//! Reviewed production-candidate plan between recovered reference IR and Rust lowerers.
//!
//! A [`DriverPlan`] is deliberately narrower than the executable reference
//! model. It contains only effects that have an explicit Effect Contract and
//! only MMIO that resolves to a generated-PAC binding. Unsupported vendor
//! behavior is rejected before either production-oriented lowerer runs.

mod pac_binding;
mod pac_lowerer;
#[cfg(test)]
mod tests;
mod transition_lowerer;

use std::collections::BTreeSet;

pub use pac_binding::*;
pub use pac_lowerer::{PacLeafOutput, lower_pac_leaf};
pub use transition_lowerer::{TransitionSkeletonOutput, lower_transition_skeleton};

use crate::{
    EffectDisposition, EffectPolicy, EffectSelector, MemoryAccess, ObservableEvent,
    ResolvedReferenceBody, ResolvedReferenceEvent, ResolvedReferenceFlow, ResolvedReferenceProgram,
    ResolvedReferenceTerminator, Result, SymbolicValue, evaluate_for_input,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedPacBinding {
    pub selector: u32,
    pub binding: PacRegisterBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverAction {
    Mmio {
        access: MemoryAccess,
        binding: PacRegisterBinding,
        value: Option<SymbolicValue>,
        disposition: EffectDisposition,
    },
    IndexedMmio {
        access: MemoryAccess,
        width: u8,
        input_index: u8,
        bindings: Vec<IndexedPacBinding>,
        value: Option<SymbolicValue>,
        disposition: EffectDisposition,
    },
    Delay {
        micros: SymbolicValue,
        disposition: EffectDisposition,
    },
}

impl DriverAction {
    pub fn selectors(&self) -> Vec<EffectSelector> {
        match self {
            Self::Mmio {
                access, binding, ..
            } => vec![match access {
                MemoryAccess::Read => EffectSelector::MmioRead {
                    width: binding.width,
                    address: binding.address,
                },
                MemoryAccess::Write => EffectSelector::MmioWrite {
                    width: binding.width,
                    address: binding.address,
                },
            }],
            Self::IndexedMmio {
                access,
                width,
                bindings,
                ..
            } => bindings
                .iter()
                .map(|candidate| match access {
                    MemoryAccess::Read => EffectSelector::MmioRead {
                        width: *width,
                        address: candidate.binding.address,
                    },
                    MemoryAccess::Write => EffectSelector::MmioWrite {
                        width: *width,
                        address: candidate.binding.address,
                    },
                })
                .collect(),
            Self::Delay { .. } => vec![EffectSelector::Delay],
        }
    }

    pub fn disposition(&self) -> &EffectDisposition {
        match self {
            Self::Mmio { disposition, .. }
            | Self::IndexedMmio { disposition, .. }
            | Self::Delay { disposition, .. } => disposition,
        }
    }
}

fn plan_indexed_mmio_action(
    event: &ResolvedReferenceEvent,
    policy: &EffectPolicy,
    bindings: &PacBindingIndex,
    used_rules: &mut BTreeSet<EffectSelector>,
) -> Result<DriverAction> {
    let ResolvedReferenceEvent::IndexedMmio {
        access,
        width,
        address,
        registers,
        guard,
        value,
    } = event
    else {
        unreachable!("indexed-MMIO planner requires an indexed-MMIO event")
    };
    let guard = guard
        .as_ref()
        .ok_or("production DriverPlan requires a bounded indexed-MMIO selector")?;
    let input_index = guard
        .selector
        .direct_input_index()
        .ok_or("indexed-MMIO guard is not one direct ABI input")?;
    if guard.maximum >= 32 {
        return Err("indexed-MMIO DriverPlan domain exceeds 32 registers".into());
    }

    let register_names = registers
        .iter()
        .map(|register| (register.address, register.name.as_str()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut selected_addresses = BTreeSet::new();
    let mut pac_bindings = Vec::new();
    let mut common_disposition = None;
    for selector in 0..=guard.maximum {
        let register_address =
            evaluate_for_input(address, input_index, selector).ok_or_else(|| {
                format!("cannot evaluate indexed-MMIO address for selector {selector}")
            })?;
        let register_name = register_names.get(&register_address).ok_or_else(|| {
            format!(
                "indexed-MMIO selector {selector} resolves outside its SVD domain: {register_address:#010x}"
            )
        })?;
        if !selected_addresses.insert(register_address) {
            return Err(
                format!("indexed-MMIO selectors alias register {register_address:#010x}").into(),
            );
        }
        let effect_selector = match *access {
            MemoryAccess::Read => EffectSelector::MmioRead {
                width: *width,
                address: register_address,
            },
            MemoryAccess::Write => EffectSelector::MmioWrite {
                width: *width,
                address: register_address,
            },
        };
        let disposition = policy.disposition(&effect_selector).ok_or_else(|| {
            format!(
                "unclassified vendor effect in indexed DriverPlan: {}",
                effect_selector.canonical()
            )
        })?;
        if disposition == &EffectDisposition::Forbidden {
            return Err(format!(
                "forbidden vendor effect in indexed DriverPlan: {}",
                effect_selector.canonical()
            )
            .into());
        }
        match &common_disposition {
            Some(common) if common != disposition => {
                return Err("indexed-MMIO PAC bank has mixed effect dispositions".into());
            }
            Some(_) => {}
            None => common_disposition = Some(disposition.clone()),
        }
        let binding = bindings
            .register(register_address, *width, register_name)?
            .clone();
        match *access {
            MemoryAccess::Read if !binding.access.readable() => {
                return Err(format!("PAC register {register_name} is not readable").into());
            }
            MemoryAccess::Write if !binding.access.writable() => {
                return Err(format!("PAC register {register_name} is not writable").into());
            }
            _ => {}
        }
        used_rules.insert(effect_selector);
        pac_bindings.push(IndexedPacBinding { selector, binding });
    }
    let domain_addresses = register_names.keys().copied().collect::<BTreeSet<_>>();
    if selected_addresses != domain_addresses {
        return Err("indexed-MMIO guard does not cover its complete SVD domain".into());
    }

    Ok(DriverAction::IndexedMmio {
        access: *access,
        width: *width,
        input_index,
        bindings: pac_bindings,
        value: value.clone(),
        disposition: common_disposition.expect("non-empty guarded domain"),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverTerminator {
    Return(SymbolicValue),
    Branch {
        condition: crate::BranchCondition,
        taken: Box<DriverFlow>,
        not_taken: Box<DriverFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverFlow {
    pub actions: Vec<DriverAction>,
    pub terminator: DriverTerminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriverPlan {
    pub symbol: String,
    pub dependencies: Vec<String>,
    pub flow: DriverFlow,
    pub exit_return_modeled: bool,
}

fn plan_action(
    event: &ResolvedReferenceEvent,
    policy: &EffectPolicy,
    bindings: &PacBindingIndex,
    used_rules: &mut BTreeSet<EffectSelector>,
) -> Result<DriverAction> {
    let (selector, action) = match event {
        ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        }) => {
            let selector = match access {
                MemoryAccess::Read => EffectSelector::MmioRead {
                    width: *width,
                    address: *address,
                },
                MemoryAccess::Write => EffectSelector::MmioWrite {
                    width: *width,
                    address: *address,
                },
            };
            let disposition = policy.disposition(&selector).ok_or_else(|| {
                format!(
                    "unclassified vendor effect in driver plan: {}",
                    selector.canonical()
                )
            })?;
            if disposition == &EffectDisposition::Forbidden {
                return Err(format!(
                    "forbidden vendor effect in driver plan: {}",
                    selector.canonical()
                )
                .into());
            }
            let binding = bindings.register(*address, *width, register)?.clone();
            match access {
                MemoryAccess::Read if !binding.access.readable() => {
                    return Err(format!("PAC register {register} is not readable").into());
                }
                MemoryAccess::Write if !binding.access.writable() => {
                    return Err(format!("PAC register {register} is not writable").into());
                }
                _ => {}
            }
            (
                selector,
                DriverAction::Mmio {
                    access: *access,
                    binding,
                    value: value.clone(),
                    disposition: disposition.clone(),
                },
            )
        }
        ResolvedReferenceEvent::DelayMicros { micros } => {
            let selector = EffectSelector::Delay;
            let disposition = policy.disposition(&selector).ok_or_else(|| {
                format!(
                    "unclassified vendor effect in driver plan: {}",
                    selector.canonical()
                )
            })?;
            if disposition == &EffectDisposition::Forbidden {
                return Err("forbidden vendor delay in driver plan".into());
            }
            (
                selector,
                DriverAction::Delay {
                    micros: micros.clone(),
                    disposition: disposition.clone(),
                },
            )
        }
        ResolvedReferenceEvent::Observable(ObservableEvent::Fence { .. }) => {
            return Err("memory fence has no production DriverPlan classification".into());
        }
        ResolvedReferenceEvent::IndexedMmio { .. } => {
            return plan_indexed_mmio_action(event, policy, bindings, used_rules);
        }
        ResolvedReferenceEvent::PollMmio { .. }
        | ResolvedReferenceEvent::BoundedPoll { .. }
        | ResolvedReferenceEvent::PollFlow { .. }
        | ResolvedReferenceEvent::SymmetricCalibrationSearch { .. }
        | ResolvedReferenceEvent::Memory { .. }
        | ResolvedReferenceEvent::WordToBytesMemoryLoop { .. }
        | ResolvedReferenceEvent::BytesToWordMemoryLoop { .. }
        | ResolvedReferenceEvent::ExternalCall { .. }
        | ResolvedReferenceEvent::ModeledDirectCall { .. }
        | ResolvedReferenceEvent::DiagnosticCall { .. }
        | ResolvedReferenceEvent::ComposedCall { .. }
        | ResolvedReferenceEvent::ComposedCallWithScratch { .. }
        | ResolvedReferenceEvent::WideSignedDivide { .. } => {
            return Err(format!(
                "resolved reference event has no production DriverPlan lowering: {event:?}"
            )
            .into());
        }
    };
    used_rules.insert(selector);
    Ok(action)
}

fn plan_flow(
    flow: &ResolvedReferenceFlow,
    policy: &EffectPolicy,
    bindings: &PacBindingIndex,
    used_rules: &mut BTreeSet<EffectSelector>,
) -> Result<DriverFlow> {
    let actions = flow
        .events
        .iter()
        .map(|event| plan_action(event, policy, bindings, used_rules))
        .collect::<Result<Vec<_>>>()?;
    let terminator = match &flow.terminator {
        ResolvedReferenceTerminator::Return(value) => DriverTerminator::Return(value.clone()),
        ResolvedReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => DriverTerminator::Branch {
            condition: condition.clone(),
            taken: Box::new(plan_flow(taken, policy, bindings, used_rules)?),
            not_taken: Box::new(plan_flow(not_taken, policy, bindings, used_rules)?),
        },
    };
    Ok(DriverFlow {
        actions,
        terminator,
    })
}

impl DriverPlan {
    pub fn from_resolved(
        program: &ResolvedReferenceProgram,
        policy: &EffectPolicy,
        bindings: &PacBindingIndex,
    ) -> Result<Self> {
        let mut used_rules = BTreeSet::new();
        let flow = match &program.body {
            ResolvedReferenceBody::Linear {
                events,
                return_value,
            } => plan_flow(
                &ResolvedReferenceFlow {
                    events: events.clone(),
                    terminator: ResolvedReferenceTerminator::Return(return_value.clone()),
                },
                policy,
                bindings,
                &mut used_rules,
            )?,
            ResolvedReferenceBody::Flow(flow) => {
                plan_flow(flow, policy, bindings, &mut used_rules)?
            }
        };
        for (selector, disposition) in policy.rules() {
            if disposition != &EffectDisposition::Forbidden && !used_rules.contains(selector) {
                return Err(format!(
                    "declared DriverPlan effect rule was not exercised: {}",
                    selector.canonical()
                )
                .into());
            }
        }
        Ok(Self {
            symbol: program.symbol.clone(),
            dependencies: program.dependencies.clone(),
            flow,
            exit_return_modeled: program.exit_return_modeled,
        })
    }

    pub fn canonical(&self) -> String {
        fn append_flow(output: &mut String, flow: &DriverFlow, indent: &str) {
            for action in &flow.actions {
                output.push_str(indent);
                output.push_str("action ");
                let selectors = action.selectors();
                output.push_str(
                    &selectors
                        .iter()
                        .map(EffectSelector::canonical)
                        .collect::<Vec<_>>()
                        .join(","),
                );
                output.push(' ');
                output.push_str(&action.disposition().canonical());
                match action {
                    DriverAction::Mmio { binding, .. } => {
                        output.push_str(" pac=");
                        output.push_str(&binding.peripheral_type);
                        output.push_str("::");
                        output.push_str(&binding.method_path("self"));
                    }
                    DriverAction::IndexedMmio {
                        input_index,
                        bindings,
                        ..
                    } => {
                        output.push_str(&format!(" selector=arg{input_index} pac-bank="));
                        output.push_str(
                            &bindings
                                .iter()
                                .map(|candidate| {
                                    format!(
                                        "{}:{}::{}",
                                        candidate.selector,
                                        candidate.binding.peripheral_type,
                                        candidate.binding.method_path("self")
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("|"),
                        );
                    }
                    DriverAction::Delay { .. } => {}
                }
                output.push('\n');
            }
            match &flow.terminator {
                DriverTerminator::Return(value) => {
                    output.push_str(indent);
                    output.push_str("return ");
                    output.push_str(&value.canonical());
                    output.push('\n');
                }
                DriverTerminator::Branch {
                    condition,
                    taken,
                    not_taken,
                } => {
                    output.push_str(indent);
                    output.push_str("branch ");
                    output.push_str(&format!("{:?}", condition.operation));
                    output.push('\n');
                    append_flow(output, taken, &format!("{indent}  taken "));
                    append_flow(output, not_taken, &format!("{indent}  not-taken "));
                }
            }
        }

        let mut output = format!(
            "driver-plan 1\nsymbol {}\nexit-return-modeled {}\n",
            self.symbol, self.exit_return_modeled
        );
        for dependency in &self.dependencies {
            output.push_str("dependency ");
            output.push_str(dependency);
            output.push('\n');
        }
        append_flow(&mut output, &self.flow, "");
        output
    }
}
