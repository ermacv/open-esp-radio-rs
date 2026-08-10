//! Draft reference control-flow data and input queries.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchOperation {
    Equal,
    NotEqual,
    LessSigned,
    GreaterEqualSigned,
    LessUnsigned,
    GreaterEqualUnsigned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchCondition {
    pub site: u32,
    pub operation: BranchOperation,
    pub left: SymbolicValue,
    pub right: SymbolicValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftReferenceTerminator {
    Return(SymbolicValue),
    Branch {
        condition: BranchCondition,
        taken: Box<DraftReferenceFlow>,
        not_taken: Box<DraftReferenceFlow>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DraftReferenceFlow {
    pub events: Vec<DraftReferenceEvent>,
    pub terminator: DraftReferenceTerminator,
}

pub fn collect_value_inputs(value: &SymbolicValue, output: &mut BTreeSet<u8>) {
    match value {
        SymbolicValue::Input { index } | SymbolicValue::InputConstant { index, .. } => {
            output.insert(*index);
        }
        SymbolicValue::Expression { left, right, .. } => {
            collect_value_inputs(left, output);
            collect_value_inputs(right, output);
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            collect_value_inputs(dividend_low, output);
            collect_value_inputs(dividend_high, output);
            collect_value_inputs(divisor_low, output);
            collect_value_inputs(divisor_high, output);
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

pub fn collect_reference_flow_inputs(flow: &DraftReferenceFlow, output: &mut BTreeSet<u8>) {
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
            DraftReferenceEvent::PollMmio { address, guard, .. } => {
                collect_value_inputs(address, output);
                if let Some(guard) = guard {
                    collect_value_inputs(&guard.selector, output);
                }
            }
            DraftReferenceEvent::BoundedPoll {
                body, on_exhausted, ..
            } => {
                collect_reference_flow_inputs(body, output);
                if let Some(event) = on_exhausted {
                    collect_reference_event_inputs(event, output);
                }
            }
            DraftReferenceEvent::PollFlow { body, .. } => {
                collect_reference_flow_inputs(body, output);
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                sample,
                ..
            } => {
                collect_reference_flow_inputs(initial_read, output);
                collect_reference_flow_inputs(setup, output);
                collect_reference_flow_inputs(sample, output);
            }
            DraftReferenceEvent::Memory { address, value, .. } => {
                collect_value_inputs(address, output);
                if let Some(value) = value {
                    collect_value_inputs(value, output);
                }
            }
            DraftReferenceEvent::DelayMicros { micros } => collect_value_inputs(micros, output),
            DraftReferenceEvent::ExternalCall {
                table: _,
                function,
                arguments,
                ..
            } => {
                let argument_count = function.spec().argument_count;
                for value in arguments.iter().take(usize::from(argument_count)) {
                    collect_value_inputs(value, output);
                }
            }
            DraftReferenceEvent::ModeledDirectCall {
                function,
                arguments,
                ..
            } => {
                for value in arguments.iter().take(usize::from(function.argument_count)) {
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
            DraftReferenceEvent::ComposedCallWithScratch {
                arguments,
                flow,
                scratch_argument,
                ..
            } => {
                for index in reference_flow_input_indices(flow) {
                    if index != *scratch_argument {
                        collect_value_inputs(&arguments[usize::from(index)], output);
                    }
                }
            }
            DraftReferenceEvent::WideSignedDivide {
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
                ..
            } => {
                collect_value_inputs(dividend_low, output);
                collect_value_inputs(dividend_high, output);
                collect_value_inputs(divisor_low, output);
                collect_value_inputs(divisor_high, output);
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

fn collect_reference_event_inputs(event: &DraftReferenceEvent, output: &mut BTreeSet<u8>) {
    let flow = DraftReferenceFlow {
        events: vec![event.clone()],
        terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
    };
    collect_reference_flow_inputs(&flow, output);
}

pub fn reference_flow_input_indices(flow: &DraftReferenceFlow) -> BTreeSet<u8> {
    let mut output = BTreeSet::new();
    collect_reference_flow_inputs(flow, &mut output);
    output
}
