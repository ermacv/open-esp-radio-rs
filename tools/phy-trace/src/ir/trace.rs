//! Observable traces and the current reference-control-flow representation.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    indexed_mmio::{IndexedMmioGuard, IndexedMmioRegister},
    value::{BitSource, SymbolicValue},
};
use crate::external_abi;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryAccess {
    Read,
    Write,
}

pub(crate) fn encode_fence_set(set: rv_asm::FenceSet) -> u8 {
    u8::from(set.device_input) << 3
        | u8::from(set.device_output) << 2
        | u8::from(set.memory_read) << 1
        | u8::from(set.memory_write)
}

#[cfg(test)]
pub(crate) fn parse_fence_set(value: &str) -> Option<u8> {
    let mut encoded = 0_u8;
    for character in value.chars() {
        encoded |= match character.to_ascii_lowercase() {
            'i' => 1 << 3,
            'o' => 1 << 2,
            'r' => 1 << 1,
            'w' => 1,
            _ => return None,
        };
    }
    Some(encoded)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ObservableEvent {
    Memory {
        access: MemoryAccess,
        width: u8,
        address: u32,
        register: String,
        value: Option<SymbolicValue>,
    },
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DraftReferenceEvent {
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
    TailCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue; 8]>,
    },
    Call {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue; 8]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue; 8]>,
        flow: Box<DraftReferenceFlow>,
        result_modeled: bool,
    },
    BranchDecision {
        condition: BranchCondition,
        taken: bool,
    },
}

pub(crate) fn reference_event_is_mmio_read(event: &DraftReferenceEvent) -> bool {
    matches!(
        event,
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access: MemoryAccess::Read,
            ..
        }) | DraftReferenceEvent::IndexedMmio {
            access: MemoryAccess::Read,
            ..
        }
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BranchOperation {
    Equal,
    NotEqual,
    LessSigned,
    GreaterEqualSigned,
    LessUnsigned,
    GreaterEqualUnsigned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BranchCondition {
    pub(crate) site: u32,
    pub(crate) operation: BranchOperation,
    pub(crate) left: SymbolicValue,
    pub(crate) right: SymbolicValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DraftReferenceTerminator {
    Return(SymbolicValue),
    Branch {
        condition: BranchCondition,
        taken: Box<DraftReferenceFlow>,
        not_taken: Box<DraftReferenceFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DraftReferenceFlow {
    pub(crate) events: Vec<DraftReferenceEvent>,
    pub(crate) terminator: DraftReferenceTerminator,
}

pub(crate) fn collect_value_inputs(value: &SymbolicValue, output: &mut BTreeSet<u8>) {
    match value {
        SymbolicValue::InputConstant { index, .. } => {
            output.insert(*index);
        }
        SymbolicValue::Expression { left, right, .. } => {
            collect_value_inputs(left, output);
            collect_value_inputs(right, output);
        }
        SymbolicValue::Bits(bits) => {
            output.extend(bits.iter().filter_map(|source| match source {
                BitSource::Input { index, .. } => Some(*index),
                _ => None,
            }));
        }
        _ => {}
    }
}

pub(crate) fn collect_reference_flow_inputs(flow: &DraftReferenceFlow, output: &mut BTreeSet<u8>) {
    for event in &flow.events {
        match event {
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                value: Some(value), ..
            }) => collect_value_inputs(value, output),
            DraftReferenceEvent::IndexedMmio {
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
            DraftReferenceEvent::Memory { address, value, .. } => {
                collect_value_inputs(address, output);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            DraftReferenceEvent::DelayMicros { micros } => collect_value_inputs(micros, output),
            DraftReferenceEvent::ExternalCall {
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
            DraftReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => {
                for value in arguments.iter().take(usize::from(*argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            DraftReferenceEvent::ComposedCall {
                arguments, flow, ..
            } => {
                for index in reference_flow_input_indices(flow) {
                    collect_value_inputs(&arguments[usize::from(index)], output);
                }
            }
            _ => {}
        }
    }
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => collect_value_inputs(value, output),
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            collect_value_inputs(&condition.left, output);
            collect_value_inputs(&condition.right, output);
            collect_reference_flow_inputs(taken, output);
            collect_reference_flow_inputs(not_taken, output);
        }
    }
}

pub(crate) fn reference_flow_input_indices(flow: &DraftReferenceFlow) -> BTreeSet<u8> {
    let mut output = BTreeSet::new();
    collect_reference_flow_inputs(flow, &mut output);
    output
}

pub(crate) fn reference_flow_exit_modeled(flow: &DraftReferenceFlow) -> bool {
    reference_flow_exit_modeled_with_calls(flow, BTreeMap::new())
}

pub(crate) fn reference_flow_exit_modeled_with_calls(
    flow: &DraftReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> bool {
    let Some(available) = validate_reference_events(&flow.events, available) else {
        return false;
    };
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => {
            value.is_resolved() && value_call_results_available(value, &available)
        }
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && reference_flow_exit_modeled_with_calls(taken, available.clone())
                && reference_flow_exit_modeled_with_calls(not_taken, available)
        }
    }
}

pub(crate) fn value_call_results_available(
    value: &SymbolicValue,
    available: &BTreeMap<u32, bool>,
) -> bool {
    match value {
        SymbolicValue::CallResult(token) => available.get(token).copied() == Some(true),
        SymbolicValue::Expression { left, right, .. } => {
            value_call_results_available(left, available)
                && value_call_results_available(right, available)
        }
        SymbolicValue::Bits(bits) => bits.iter().all(|source| match source {
            BitSource::CallResult { call_token, .. } => {
                available.get(call_token).copied() == Some(true)
            }
            _ => true,
        }),
        _ => true,
    }
}

pub(crate) fn validate_reference_events(
    events: &[DraftReferenceEvent],
    mut available: BTreeMap<u32, bool>,
) -> Option<BTreeMap<u32, bool>> {
    for event in events {
        let values_are_available = match event {
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                value: Some(value), ..
            }) => value_call_results_available(value, &available),
            DraftReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => {
                value_call_results_available(address, &available)
                    && guard.as_ref().is_none_or(|guard| {
                        value_call_results_available(&guard.selector, &available)
                    })
                    && value
                        .as_ref()
                        .is_none_or(|value| value_call_results_available(value, &available))
            }
            DraftReferenceEvent::Memory { address, value, .. } => {
                value_call_results_available(address, &available)
                    && value
                        .as_ref()
                        .is_none_or(|value| value_call_results_available(value, &available))
            }
            DraftReferenceEvent::DelayMicros { micros } => {
                value_call_results_available(micros, &available)
            }
            DraftReferenceEvent::ExternalCall {
                table,
                function,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(
                    external_abi::function(*table, *function).argument_count,
                ))
                .all(|value| value_call_results_available(value, &available)),
            DraftReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(*argument_count))
                .all(|value| value_call_results_available(value, &available)),
            DraftReferenceEvent::ComposedCall {
                token,
                arguments,
                flow,
                result_modeled,
                ..
            } => {
                let used_inputs = reference_flow_input_indices(flow);
                if *token != available.len() as u32
                    || used_inputs.iter().any(|index| {
                        !value_call_results_available(&arguments[usize::from(*index)], &available)
                    })
                    || !reference_flow_calls_are_valid(flow)
                    || *result_modeled != reference_flow_exit_modeled(flow)
                {
                    return None;
                }
                available.insert(*token, *result_modeled);
                true
            }
            DraftReferenceEvent::Call { .. }
            | DraftReferenceEvent::TailCall { .. }
            | DraftReferenceEvent::BranchDecision { .. } => return None,
            _ => true,
        };
        if !values_are_available {
            return None;
        }
    }
    Some(available)
}

pub(crate) fn reference_flow_calls_are_valid(flow: &DraftReferenceFlow) -> bool {
    let Some(available) = validate_reference_events(&flow.events, BTreeMap::new()) else {
        return false;
    };
    match &flow.terminator {
        DraftReferenceTerminator::Return(_) => true,
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && validate_reference_flow_with_calls(taken, available.clone())
                && validate_reference_flow_with_calls(not_taken, available)
        }
    }
}

pub(crate) fn validate_reference_flow_with_calls(
    flow: &DraftReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> bool {
    let Some(available) = validate_reference_events(&flow.events, available) else {
        return false;
    };
    match &flow.terminator {
        DraftReferenceTerminator::Return(_) => true,
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            value_call_results_available(&condition.left, &available)
                && value_call_results_available(&condition.right, &available)
                && validate_reference_flow_with_calls(taken, available.clone())
                && validate_reference_flow_with_calls(not_taken, available)
        }
    }
}

impl ObservableEvent {
    pub(crate) fn canonical(&self) -> String {
        match self {
            Self::Memory {
                access,
                width,
                address,
                register,
                value,
            } => {
                let access = match access {
                    MemoryAccess::Read => "R",
                    MemoryAccess::Write => "W",
                };
                let value = value
                    .as_ref()
                    .map_or_else(|| "-".to_owned(), SymbolicValue::canonical);
                format!("{access}\t{width}\t{address:#010x}\t{register}\t{value}")
            }
            Self::Fence {
                fm,
                predecessor,
                successor,
            } => format!("FENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"),
        }
    }

    pub(crate) fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Memory {
                    access: left_access,
                    width: left_width,
                    address: left_address,
                    value: left_value,
                    ..
                },
                Self::Memory {
                    access: right_access,
                    width: right_width,
                    address: right_address,
                    value: right_value,
                    ..
                },
            ) => {
                left_access == right_access
                    && left_width == right_width
                    && left_address == right_address
                    && left_value == right_value
            }
            (
                Self::Fence {
                    fm: left_fm,
                    predecessor: left_predecessor,
                    successor: left_successor,
                },
                Self::Fence {
                    fm: right_fm,
                    predecessor: right_predecessor,
                    successor: right_successor,
                },
            ) => {
                left_fm == right_fm
                    && left_predecessor == right_predecessor
                    && left_successor == right_successor
            }
            _ => false,
        }
    }

    pub(crate) fn unmapped_address(&self) -> Option<u32> {
        match self {
            Self::Memory {
                address, register, ..
            } if register == "UNMAPPED" => Some(*address),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn memory_value(&self) -> Option<String> {
        match self {
            Self::Memory { value, .. } => value.as_ref().map(SymbolicValue::canonical),
            Self::Fence { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionAnalysis {
    pub(crate) symbol: String,
    pub(crate) events: Vec<ObservableEvent>,
    pub(crate) reference_events: Vec<DraftReferenceEvent>,
    pub(crate) reference_dependencies: Vec<String>,
    pub(crate) blockers: Vec<String>,
    pub(crate) reference_blockers: Vec<String>,
    pub(crate) return_value: SymbolicValue,
    pub(crate) reference_flow: Option<DraftReferenceFlow>,
    pub(crate) unresolved_branch: Option<BranchCondition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactSymbolIdentity {
    pub(crate) member: Option<String>,
    pub(crate) name: String,
}

impl FunctionAnalysis {
    pub(crate) fn reference_indexed_mmio_count(&self) -> usize {
        fn flow_count(flow: &DraftReferenceFlow) -> usize {
            let events = flow
                .events
                .iter()
                .map(|event| match event {
                    DraftReferenceEvent::IndexedMmio { .. } => 1,
                    DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                    _ => 0,
                })
                .sum::<usize>();
            events
                + match &flow.terminator {
                    DraftReferenceTerminator::Return(_) => 0,
                    DraftReferenceTerminator::Branch {
                        taken, not_taken, ..
                    } => flow_count(taken) + flow_count(not_taken),
                }
        }

        self.reference_flow.as_ref().map_or_else(
            || {
                self.reference_events
                    .iter()
                    .map(|event| match event {
                        DraftReferenceEvent::IndexedMmio { .. } => 1,
                        DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                        _ => 0,
                    })
                    .sum()
            },
            flow_count,
        )
    }

    pub(crate) fn is_exact(&self) -> bool {
        self.reference_flow.is_none()
            && self.blockers.is_empty()
            && self
                .events
                .iter()
                .all(|event| event.unmapped_address().is_none())
    }

    pub(crate) fn is_reference_eligible(&self) -> bool {
        self.blockers.is_empty()
            && self.reference_blockers.is_empty()
            && self.unresolved_branch.is_none()
            && self.reference_observables_are_mapped()
            && self.reference_calls_are_valid()
    }

    pub(crate) fn reference_observables_are_mapped(&self) -> bool {
        fn flow_is_mapped(flow: &DraftReferenceFlow) -> bool {
            flow.events.iter().all(|event| match event {
                DraftReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                DraftReferenceEvent::IndexedMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                _ => true,
            }) && match &flow.terminator {
                DraftReferenceTerminator::Return(_) => true,
                DraftReferenceTerminator::Branch {
                    taken, not_taken, ..
                } => flow_is_mapped(taken) && flow_is_mapped(not_taken),
            }
        }

        fn reference_event_is_mapped(event: &DraftReferenceEvent) -> bool {
            match event {
                DraftReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                DraftReferenceEvent::IndexedMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                _ => true,
            }
        }

        self.reference_flow.as_ref().map_or_else(
            || self.reference_events.iter().all(reference_event_is_mapped),
            flow_is_mapped,
        )
    }

    pub(crate) fn reference_calls_are_valid(&self) -> bool {
        if let Some(flow) = &self.reference_flow {
            reference_flow_calls_are_valid(flow)
        } else {
            validate_reference_events(&self.reference_events, BTreeMap::new()).is_some()
        }
    }

    pub(crate) fn reference_exit_a0_modeled(&self) -> bool {
        self.reference_flow.as_ref().map_or_else(
            || {
                validate_reference_events(&self.reference_events, BTreeMap::new()).is_some_and(
                    |available| {
                        self.return_value.is_resolved()
                            && value_call_results_available(&self.return_value, &available)
                    },
                )
            },
            reference_flow_exit_modeled,
        )
    }
}
