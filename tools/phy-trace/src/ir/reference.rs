//! Fully resolved reference IR accepted by Rust code generation.

use std::collections::BTreeSet;

use super::{
    BranchCondition, DraftReferenceEvent, DraftReferenceFlow, DraftReferenceTerminator,
    FunctionAnalysis, IndexedMmioGuard, IndexedMmioRegister, MemoryAccess, ObservableEvent,
    SymbolicValue, collect_value_inputs,
};
use crate::external_abi;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedReferenceEvent {
    Observable(ObservableEvent),
    IndexedMmio {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        value: Option<SymbolicValue>,
    },
    DelayMicros {
        micros: SymbolicValue,
    },
    Memory {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        region: String,
        value: Option<SymbolicValue>,
    },
    ExternalCall {
        token: u32,
        table: external_abi::Table,
        function: external_abi::Function,
        arguments: Box<[SymbolicValue; 8]>,
    },
    DiagnosticCall {
        function: String,
        argument_count: u8,
        arguments: Box<[SymbolicValue; 8]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue; 8]>,
        flow: Box<ResolvedReferenceFlow>,
        result_modeled: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedReferenceTerminator {
    Return(SymbolicValue),
    Branch {
        condition: BranchCondition,
        taken: Box<ResolvedReferenceFlow>,
        not_taken: Box<ResolvedReferenceFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedReferenceFlow {
    pub(crate) events: Vec<ResolvedReferenceEvent>,
    pub(crate) terminator: ResolvedReferenceTerminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedReferenceBody {
    Linear {
        events: Vec<ResolvedReferenceEvent>,
        return_value: SymbolicValue,
    },
    Flow(ResolvedReferenceFlow),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedReferenceProgram {
    pub(crate) symbol: String,
    pub(crate) dependencies: Vec<String>,
    pub(crate) body: ResolvedReferenceBody,
    pub(crate) exit_a0_modeled: bool,
}

fn collect_resolved_flow_inputs(flow: &ResolvedReferenceFlow, output: &mut BTreeSet<u8>) {
    for event in &flow.events {
        match event {
            ResolvedReferenceEvent::Observable(ObservableEvent::Memory {
                value: Some(value),
                ..
            }) => collect_value_inputs(value, output),
            ResolvedReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => {
                collect_value_inputs(address, output);
                if let Some(guard) = guard {
                    collect_value_inputs(&guard.selector, output);
                }
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::Memory { address, value, .. } => {
                collect_value_inputs(address, output);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::DelayMicros { micros } => collect_value_inputs(micros, output),
            ResolvedReferenceEvent::ExternalCall {
                table,
                function,
                arguments,
                ..
            } => {
                let argument_count = external_abi::function(*table, *function).argument_count;
                for value in arguments.iter().take(usize::from(argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => {
                for value in arguments.iter().take(usize::from(*argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            ResolvedReferenceEvent::ComposedCall {
                arguments, flow, ..
            } => {
                for index in resolved_reference_flow_input_indices(flow) {
                    collect_value_inputs(&arguments[usize::from(index)], output);
                }
            }
            ResolvedReferenceEvent::Observable(_) => {}
        }
    }
    match &flow.terminator {
        ResolvedReferenceTerminator::Return(value) => collect_value_inputs(value, output),
        ResolvedReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_resolved_flow_inputs(taken, output);
            collect_resolved_flow_inputs(not_taken, output);
        }
    }
}

pub(crate) fn resolved_reference_flow_input_indices(flow: &ResolvedReferenceFlow) -> BTreeSet<u8> {
    let mut output = BTreeSet::new();
    collect_resolved_flow_inputs(flow, &mut output);
    output
}

impl ResolvedReferenceEvent {
    fn from_draft(event: &DraftReferenceEvent) -> Result<Self, String> {
        Ok(match event {
            DraftReferenceEvent::Observable(event) => Self::Observable(event.clone()),
            DraftReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => Self::IndexedMmio {
                access: *access,
                width: *width,
                address: address.clone(),
                registers: registers.clone(),
                guard: guard.clone(),
                value: value.clone(),
            },
            DraftReferenceEvent::DelayMicros { micros } => Self::DelayMicros {
                micros: micros.clone(),
            },
            DraftReferenceEvent::Memory {
                access,
                width,
                address,
                region,
                value,
            } => Self::Memory {
                access: *access,
                width: *width,
                address: address.clone(),
                region: region.clone(),
                value: value.clone(),
            },
            DraftReferenceEvent::ExternalCall {
                token,
                table,
                function,
                arguments,
            } => Self::ExternalCall {
                token: *token,
                table: *table,
                function: *function,
                arguments: arguments.clone(),
            },
            DraftReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => Self::DiagnosticCall {
                function: function.clone(),
                argument_count: *argument_count,
                arguments: arguments.clone(),
            },
            DraftReferenceEvent::ComposedCall {
                token,
                symbol,
                arguments,
                flow,
                result_modeled,
            } => Self::ComposedCall {
                token: *token,
                symbol: symbol.clone(),
                arguments: arguments.clone(),
                flow: Box::new(ResolvedReferenceFlow::from_draft(flow)?),
                result_modeled: *result_modeled,
            },
            DraftReferenceEvent::Call { site, target, .. } => {
                return Err(format!(
                    "unresolved call at {site:#010x} to {target:#010x} escaped reference resolution"
                ));
            }
            DraftReferenceEvent::TailCall { site, target, .. } => {
                return Err(format!(
                    "unresolved tail call at {site:#010x} to {target:#010x} escaped reference resolution"
                ));
            }
            DraftReferenceEvent::BranchDecision { condition, .. } => {
                return Err(format!(
                    "branch decision at {:#010x} escaped structured reference flow",
                    condition.site
                ));
            }
        })
    }
}

impl ResolvedReferenceFlow {
    fn from_draft(flow: &DraftReferenceFlow) -> Result<Self, String> {
        let events = flow
            .events
            .iter()
            .map(ResolvedReferenceEvent::from_draft)
            .collect::<Result<Vec<_>, _>>()?;
        let terminator = match &flow.terminator {
            DraftReferenceTerminator::Return(value) => {
                ResolvedReferenceTerminator::Return(value.clone())
            }
            DraftReferenceTerminator::Branch {
                condition,
                taken,
                not_taken,
            } => ResolvedReferenceTerminator::Branch {
                condition: condition.clone(),
                taken: Box::new(Self::from_draft(taken)?),
                not_taken: Box::new(Self::from_draft(not_taken)?),
            },
        };
        Ok(Self { events, terminator })
    }
}

impl TryFrom<&FunctionAnalysis> for ResolvedReferenceProgram {
    type Error = String;

    fn try_from(trace: &FunctionAnalysis) -> Result<Self, Self::Error> {
        if !trace.is_reference_eligible() {
            let mut reasons = trace.blockers.clone();
            reasons.extend(trace.reference_blockers.iter().cloned());
            reasons.extend(
                trace
                    .events
                    .iter()
                    .filter_map(ObservableEvent::unmapped_address)
                    .map(|address| format!("unmapped-register {address:#010x}")),
            );
            if trace.unresolved_branch.is_some() && reasons.is_empty() {
                reasons.push("unresolved symbolic branch".to_owned());
            }
            if reasons.is_empty() {
                reasons.push("reference event validation failed".to_owned());
            }
            return Err(format!(
                "{} is not eligible for reference generation: {}",
                trace.symbol,
                reasons.join("; ")
            ));
        }

        let body = if let Some(flow) = &trace.reference_flow {
            ResolvedReferenceBody::Flow(ResolvedReferenceFlow::from_draft(flow)?)
        } else {
            ResolvedReferenceBody::Linear {
                events: trace
                    .reference_events
                    .iter()
                    .map(ResolvedReferenceEvent::from_draft)
                    .collect::<Result<Vec<_>, _>>()?,
                return_value: trace.return_value.clone(),
            }
        };
        Ok(Self {
            symbol: trace.symbol.clone(),
            dependencies: trace.reference_dependencies.clone(),
            body,
            exit_a0_modeled: trace.reference_exit_a0_modeled(),
        })
    }
}
