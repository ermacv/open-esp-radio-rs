//! Observable traces and the current reference-control-flow representation.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    indexed_mmio::{IndexedMmioGuard, IndexedMmioRegister},
    value::{BitSource, SymbolicValue},
};

pub const DEFERRED_CALLER_MEMORY_REGION: &str = "deferred call-composed caller memory";
pub const SECONDARY_CALL_RESULT_TOKEN_FLAG: u32 = 1 << 31;
use open_radio_vendor_validator_core::{ExternalFunctionRef, ExternalTableRef};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryAccess {
    Read,
    Write,
}

pub fn parse_fence_set(value: &str) -> Option<u8> {
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
pub enum ObservableEvent {
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
pub enum DraftReferenceEvent {
    Observable(ObservableEvent),
    IndexedMmio {
        access: MemoryAccess,
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        value: Option<SymbolicValue>,
    },
    PollMmio {
        width: u8,
        address: SymbolicValue,
        registers: Vec<IndexedMmioRegister>,
        guard: Option<IndexedMmioGuard>,
        mask: u32,
        expected: u32,
    },
    BoundedPoll {
        maximum_attempts: u16,
        body: Box<DraftReferenceFlow>,
        repeat_while_mask: u32,
        repeat_while_expected: u32,
        on_exhausted: Option<Box<DraftReferenceEvent>>,
    },
    PollFlow {
        body: Box<DraftReferenceFlow>,
        exit_when_mask: u32,
        exit_when_expected: u32,
    },
    SymmetricCalibrationSearch {
        token: u32,
        attempts_per_direction: u16,
        settle_micros: u32,
        sample_shift: u8,
        sample_mask: u32,
        accepted_sample: u32,
        initial_read: Box<DraftReferenceFlow>,
        setup: Box<DraftReferenceFlow>,
        write_candidate: Box<DraftReferenceFlow>,
        sample: Box<DraftReferenceFlow>,
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
    PrivateStackLoad {
        token: u32,
        offset: i32,
        width: u8,
        signed: bool,
    },
    PrivateStackStore {
        offset: i32,
        width: u8,
        value: SymbolicValue,
    },
    ExternalCall {
        token: u32,
        table: ExternalTableRef,
        function: ExternalFunctionRef,
        arguments: Box<[SymbolicValue]>,
    },
    DiagnosticCall {
        function: String,
        argument_count: u8,
        arguments: Box<[SymbolicValue]>,
    },
    TailCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
    },
    Call {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
    },
    ComposedCall {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<DraftReferenceFlow>,
        result_modeled: bool,
    },
    ScratchCall {
        token: u32,
        site: u32,
        target: u32,
        arguments: Box<[SymbolicValue]>,
        scratch_argument: u8,
        scratch_size: u16,
    },
    ComposedCallWithScratch {
        token: u32,
        symbol: String,
        arguments: Box<[SymbolicValue]>,
        flow: Box<DraftReferenceFlow>,
        result_modeled: bool,
        scratch_argument: u8,
        scratch_size: u16,
    },
    WideSignedDivide {
        token: u32,
        dividend_low: SymbolicValue,
        dividend_high: SymbolicValue,
        divisor_low: SymbolicValue,
        divisor_high: SymbolicValue,
    },
    BranchDecision {
        condition: BranchCondition,
        taken: bool,
    },
}

pub fn reference_event_is_mmio_read(event: &DraftReferenceEvent) -> bool {
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
        SymbolicValue::InputConstant { index, .. } => {
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

pub fn reference_flow_exit_modeled(flow: &DraftReferenceFlow) -> bool {
    reference_flow_exit_modeled_with_calls(flow, BTreeMap::new())
}

pub fn reference_flow_exit_modeled_with_calls(
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
            value_is_reference_ready(&condition.left, &available)
                && value_is_reference_ready(&condition.right, &available)
                && reference_flow_exit_modeled_with_calls(taken, available.clone())
                && reference_flow_exit_modeled_with_calls(not_taken, available)
        }
    }
}

pub fn value_call_results_available(
    value: &SymbolicValue,
    available: &BTreeMap<u32, bool>,
) -> bool {
    match value {
        SymbolicValue::CallResult(token) => available.get(token).copied() == Some(true),
        SymbolicValue::Expression { left, right, .. } => {
            value_call_results_available(left, available)
                && value_call_results_available(right, available)
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            ..
        } => {
            value_call_results_available(dividend_low, available)
                && value_call_results_available(dividend_high, available)
                && value_call_results_available(divisor_low, available)
                && value_call_results_available(divisor_high, available)
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

fn value_is_reference_ready(value: &SymbolicValue, available: &BTreeMap<u32, bool>) -> bool {
    value.is_resolved() && value_call_results_available(value, available)
}

pub fn validate_reference_events(
    events: &[DraftReferenceEvent],
    available: BTreeMap<u32, bool>,
) -> Option<BTreeMap<u32, bool>> {
    validate_reference_events_detailed(events, available).ok()
}

fn reference_value_error(
    value: &SymbolicValue,
    available: &BTreeMap<u32, bool>,
) -> Option<&'static str> {
    if !value.is_resolved() {
        Some("symbolic value is unresolved")
    } else if !value_call_results_available(value, available) {
        Some("symbolic value depends on an unavailable call result")
    } else {
        None
    }
}

fn validate_reference_events_detailed(
    events: &[DraftReferenceEvent],
    mut available: BTreeMap<u32, bool>,
) -> std::result::Result<BTreeMap<u32, bool>, String> {
    for (event_index, event) in events.iter().enumerate() {
        let value_error = |role: &str, value: &SymbolicValue| {
            reference_value_error(value, &available)
                .map(|error| format!("event {event_index} {role}: {error}"))
        };
        let error = match event {
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                value: Some(value), ..
            }) => value_error("MMIO write value", value),
            DraftReferenceEvent::IndexedMmio {
                address,
                guard,
                value,
                ..
            } => value_error("indexed MMIO address", address)
                .or_else(|| {
                    guard
                        .as_ref()
                        .and_then(|guard| value_error("indexed MMIO guard", &guard.selector))
                })
                .or_else(|| {
                    value
                        .as_ref()
                        .and_then(|value| value_error("indexed MMIO write value", value))
                }),
            DraftReferenceEvent::PollMmio { address, guard, .. } => {
                value_error("MMIO poll address", address).or_else(|| {
                    guard
                        .as_ref()
                        .and_then(|guard| value_error("MMIO poll guard", &guard.selector))
                })
            }
            DraftReferenceEvent::BoundedPoll {
                maximum_attempts,
                body,
                on_exhausted,
                ..
            } => {
                if *maximum_attempts == 0 {
                    Some(format!("event {event_index} bounded poll has no attempts"))
                } else if !reference_flow_exit_modeled(body) {
                    Some(format!(
                        "event {event_index} bounded-poll body has an unresolved result"
                    ))
                } else if on_exhausted.as_deref().is_some_and(|event| {
                    !matches!(event, DraftReferenceEvent::DiagnosticCall { .. })
                }) {
                    Some(format!(
                        "event {event_index} bounded-poll exhaustion is not a diagnostic call"
                    ))
                } else if let Some(event) = on_exhausted {
                    validate_reference_events_detailed(
                        std::slice::from_ref(event.as_ref()),
                        available.clone(),
                    )
                    .err()
                    .map(|error| {
                        format!("event {event_index} bounded-poll exhaustion is invalid: {error}")
                    })
                } else {
                    None
                }
            }
            DraftReferenceEvent::PollFlow {
                body,
                exit_when_mask,
                exit_when_expected,
            } => {
                if *exit_when_expected & !*exit_when_mask != 0 {
                    Some(format!(
                        "event {event_index} poll-flow exit value is outside its mask"
                    ))
                } else if !reference_flow_exit_modeled(body) {
                    Some(format!(
                        "event {event_index} poll-flow body has an unresolved result"
                    ))
                } else {
                    None
                }
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                token,
                attempts_per_direction,
                sample_shift,
                sample_mask,
                accepted_sample,
                initial_read,
                setup,
                write_candidate,
                sample,
                ..
            } => {
                let next_token = available
                    .keys()
                    .filter(|token| **token & SECONDARY_CALL_RESULT_TOKEN_FLAG == 0)
                    .count() as u32;
                let no_inputs =
                    |flow: &DraftReferenceFlow| reference_flow_input_indices(flow).is_empty();
                let write_inputs = reference_flow_input_indices(write_candidate);
                if *token != next_token {
                    Some(format!(
                        "event {event_index} calibration token {token} is not the next token {next_token}"
                    ))
                } else if *attempts_per_direction == 0 {
                    Some(format!(
                        "event {event_index} calibration search has no attempts"
                    ))
                } else if *sample_shift >= 32 {
                    Some(format!(
                        "event {event_index} calibration sample shift is outside a 32-bit word"
                    ))
                } else if *accepted_sample & !*sample_mask != 0 {
                    Some(format!(
                        "event {event_index} accepted calibration sample is outside its mask"
                    ))
                } else if !no_inputs(initial_read) || !no_inputs(setup) || !no_inputs(sample) {
                    Some(format!(
                        "event {event_index} calibration fixed flows unexpectedly consume outer arguments"
                    ))
                } else if write_inputs != BTreeSet::from([0]) {
                    Some(format!(
                        "event {event_index} calibration writer must consume only local candidate input 0"
                    ))
                } else if !reference_flow_exit_modeled(initial_read) {
                    Some(format!(
                        "event {event_index} calibration initial read has an unresolved result"
                    ))
                } else if !reference_flow_exit_modeled(sample) {
                    Some(format!(
                        "event {event_index} calibration sample has an unresolved result"
                    ))
                } else if let Some((role, error)) = [
                    ("initial read", initial_read.as_ref()),
                    ("setup", setup.as_ref()),
                    ("candidate writer", write_candidate.as_ref()),
                    ("sample", sample.as_ref()),
                ]
                .into_iter()
                .find_map(|(role, flow)| {
                    validate_reference_flow_with_calls_detailed(flow, BTreeMap::new())
                        .err()
                        .map(|error| (role, error))
                }) {
                    Some(format!(
                        "event {event_index} calibration {role} flow is invalid: {error}"
                    ))
                } else {
                    available.insert(*token, true);
                    None
                }
            }
            DraftReferenceEvent::Memory { address, value, .. } => {
                value_error("memory address", address).or_else(|| {
                    value
                        .as_ref()
                        .and_then(|value| value_error("memory write value", value))
                })
            }
            DraftReferenceEvent::DelayMicros { micros } => value_error("delay value", micros),
            DraftReferenceEvent::ExternalCall {
                table: _,
                function,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(function.spec().argument_count))
                .enumerate()
                .find_map(|(index, value)| {
                    value_error(&format!("external-call argument {index}"), value)
                }),
            DraftReferenceEvent::DiagnosticCall {
                argument_count,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(*argument_count))
                .enumerate()
                .find_map(|(index, value)| {
                    value_error(&format!("diagnostic-call argument {index}"), value)
                }),
            DraftReferenceEvent::ComposedCall {
                token,
                arguments,
                flow,
                result_modeled,
                ..
            } => {
                let used_inputs = reference_flow_input_indices(flow);
                let next_token = available
                    .keys()
                    .filter(|token| **token & SECONDARY_CALL_RESULT_TOKEN_FLAG == 0)
                    .count() as u32;
                if *token != next_token {
                    Some(format!(
                        "event {event_index} composed-call token {token} is not the next token {}",
                        next_token
                    ))
                } else if let Some((index, error)) = used_inputs.iter().find_map(|index| {
                    reference_value_error(&arguments[usize::from(*index)], &available)
                        .map(|error| (*index, error))
                }) {
                    Some(format!(
                        "event {event_index} composed-call argument {index}: {error}"
                    ))
                } else if let Err(error) =
                    validate_reference_flow_with_calls_detailed(flow, BTreeMap::new())
                {
                    Some(format!(
                        "event {event_index} composed-call flow is invalid: {error}"
                    ))
                } else if *result_modeled != reference_flow_exit_modeled(flow) {
                    Some(format!(
                        "event {event_index} composed-call result flag is inconsistent with its flow"
                    ))
                } else {
                    available.insert(*token, *result_modeled);
                    None
                }
            }
            DraftReferenceEvent::ComposedCallWithScratch {
                token,
                arguments,
                flow,
                result_modeled,
                scratch_argument,
                scratch_size,
                ..
            } => {
                let used_inputs = reference_flow_input_indices(flow);
                let next_token = available
                    .keys()
                    .filter(|token| **token & SECONDARY_CALL_RESULT_TOKEN_FLAG == 0)
                    .count() as u32;
                if *token != next_token {
                    Some(format!(
                        "event {event_index} scratch-call token {token} is not the next token {next_token}"
                    ))
                } else if usize::from(*scratch_argument) >= arguments.len() {
                    Some(format!(
                        "event {event_index} scratch argument {scratch_argument} is outside the modeled call arguments"
                    ))
                } else if *scratch_size == 0 || *scratch_size > 256 {
                    Some(format!(
                        "event {event_index} scratch size {scratch_size} is outside 1..=256"
                    ))
                } else if !used_inputs.contains(scratch_argument) {
                    Some(format!(
                        "event {event_index} scratch argument {scratch_argument} is not consumed by the callee"
                    ))
                } else if let Some((index, error)) = used_inputs
                    .iter()
                    .filter(|index| **index != *scratch_argument)
                    .find_map(|index| {
                        reference_value_error(&arguments[usize::from(*index)], &available)
                            .map(|error| (*index, error))
                    })
                {
                    Some(format!(
                        "event {event_index} scratch-call argument {index}: {error}"
                    ))
                } else if let Err(error) =
                    validate_reference_flow_with_calls_detailed(flow, BTreeMap::new())
                {
                    Some(format!(
                        "event {event_index} scratch-call flow is invalid: {error}"
                    ))
                } else if *result_modeled != reference_flow_exit_modeled(flow) {
                    Some(format!(
                        "event {event_index} scratch-call result flag is inconsistent with its flow"
                    ))
                } else {
                    available.insert(*token, *result_modeled);
                    None
                }
            }
            DraftReferenceEvent::WideSignedDivide {
                token,
                dividend_low,
                dividend_high,
                divisor_low,
                divisor_high,
            } => {
                let next_token = available
                    .keys()
                    .filter(|token| **token & SECONDARY_CALL_RESULT_TOKEN_FLAG == 0)
                    .count() as u32;
                if *token != next_token {
                    Some(format!(
                        "event {event_index} wide-divide token {token} is not the next token {next_token}"
                    ))
                } else if let Some(error) = [dividend_low, dividend_high, divisor_low, divisor_high]
                    .into_iter()
                    .find_map(|value| reference_value_error(value, &available))
                {
                    Some(format!("event {event_index} wide-divide operand: {error}"))
                } else {
                    available.insert(*token, true);
                    available.insert(*token | SECONDARY_CALL_RESULT_TOKEN_FLAG, true);
                    None
                }
            }
            DraftReferenceEvent::Call { .. }
            | DraftReferenceEvent::TailCall { .. }
            | DraftReferenceEvent::ScratchCall { .. }
            | DraftReferenceEvent::BranchDecision { .. }
            | DraftReferenceEvent::PrivateStackLoad { .. }
            | DraftReferenceEvent::PrivateStackStore { .. } => Some(format!(
                "event {event_index} contains an unresolved call, branch, or private-stack marker"
            )),
            _ => None,
        };
        if let Some(error) = error {
            return Err(error);
        }
    }
    Ok(available)
}

pub fn reference_flow_calls_are_valid(flow: &DraftReferenceFlow) -> bool {
    validate_reference_flow_with_calls_detailed(flow, BTreeMap::new()).is_ok()
}

fn validate_reference_flow_with_calls_detailed(
    flow: &DraftReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> std::result::Result<(), String> {
    let available = validate_reference_events_detailed(&flow.events, available)?;
    match &flow.terminator {
        DraftReferenceTerminator::Return(_) => Ok(()),
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            if let Some(error) = reference_value_error(&condition.left, &available) {
                return Err(format!(
                    "branch at {:#010x} left operand: {error}",
                    condition.site
                ));
            }
            if let Some(error) = reference_value_error(&condition.right, &available) {
                return Err(format!(
                    "branch at {:#010x} right operand: {error}",
                    condition.site
                ));
            }
            validate_reference_flow_with_calls_detailed(taken, available.clone())
                .map_err(|error| format!("taken branch at {:#010x}: {error}", condition.site))?;
            validate_reference_flow_with_calls_detailed(not_taken, available)
                .map_err(|error| format!("not-taken branch at {:#010x}: {error}", condition.site))
        }
    }
}

impl ObservableEvent {
    pub fn canonical(&self) -> String {
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

    pub fn equivalent(&self, other: &Self) -> bool {
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

    pub fn unmapped_address(&self) -> Option<u32> {
        match self {
            Self::Memory {
                address, register, ..
            } if register == "UNMAPPED" => Some(*address),
            _ => None,
        }
    }

    pub fn memory_value(&self) -> Option<String> {
        match self {
            Self::Memory { value, .. } => value.as_ref().map(SymbolicValue::canonical),
            Self::Fence { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionAnalysis {
    pub symbol: String,
    pub events: Vec<ObservableEvent>,
    pub reference_events: Vec<DraftReferenceEvent>,
    pub reference_dependencies: Vec<String>,
    pub blockers: Vec<String>,
    pub reference_blockers: Vec<String>,
    pub return_value: SymbolicValue,
    pub reference_flow: Option<DraftReferenceFlow>,
    pub unresolved_branch: Option<BranchCondition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSymbolIdentity {
    pub member: Option<String>,
    pub name: String,
}

impl FunctionAnalysis {
    pub fn reference_failure_reasons(&self) -> Vec<String> {
        let mut reasons = self.blockers.clone();
        reasons.extend(self.reference_blockers.iter().cloned());
        reasons.extend(
            self.reference_unmapped_addresses()
                .into_iter()
                .map(|address| format!("unmapped-register {address:#010x}")),
        );
        if self.unresolved_branch.is_some() && reasons.is_empty() {
            reasons.push("unresolved symbolic branch".to_owned());
        }
        if reasons.is_empty() && !self.is_reference_eligible() {
            let validation_error = self.reference_flow.as_ref().map_or_else(
                || {
                    validate_reference_events_detailed(&self.reference_events, BTreeMap::new())
                        .err()
                },
                |flow| validate_reference_flow_with_calls_detailed(flow, BTreeMap::new()).err(),
            );
            reasons.push(validation_error.map_or_else(
                || "reference eligibility failed without a classified cause".to_owned(),
                |error| format!("reference-event-validation: {error}"),
            ));
        }
        reasons
    }

    pub fn reference_unmapped_addresses(&self) -> BTreeSet<u32> {
        fn collect_event(event: &DraftReferenceEvent, output: &mut BTreeSet<u32>) {
            match event {
                DraftReferenceEvent::Observable(event) => {
                    output.extend(event.unmapped_address());
                }
                DraftReferenceEvent::ComposedCall { flow, .. } => collect_flow(flow, output),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                    collect_flow(flow, output)
                }
                DraftReferenceEvent::BoundedPoll { body, .. } => collect_flow(body, output),
                DraftReferenceEvent::PollFlow { body, .. } => collect_flow(body, output),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    collect_flow(initial_read, output);
                    collect_flow(setup, output);
                    collect_flow(write_candidate, output);
                    collect_flow(sample, output);
                }
                _ => {}
            }
        }

        fn collect_flow(flow: &DraftReferenceFlow, output: &mut BTreeSet<u32>) {
            for event in &flow.events {
                collect_event(event, output);
            }
            if let DraftReferenceTerminator::Branch {
                taken, not_taken, ..
            } = &flow.terminator
            {
                collect_flow(taken, output);
                collect_flow(not_taken, output);
            }
        }

        let mut output = BTreeSet::new();
        if let Some(flow) = &self.reference_flow {
            collect_flow(flow, &mut output);
        } else {
            for event in &self.reference_events {
                collect_event(event, &mut output);
            }
        }
        output
    }

    pub fn reference_indexed_mmio_count(&self) -> usize {
        fn flow_count(flow: &DraftReferenceFlow) -> usize {
            let events = flow
                .events
                .iter()
                .map(|event| match event {
                    DraftReferenceEvent::IndexedMmio { .. }
                    | DraftReferenceEvent::PollMmio { .. } => 1,
                    DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                    DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_count(flow),
                    DraftReferenceEvent::BoundedPoll { body, .. } => flow_count(body),
                    DraftReferenceEvent::PollFlow { body, .. } => flow_count(body),
                    DraftReferenceEvent::SymmetricCalibrationSearch {
                        initial_read,
                        setup,
                        write_candidate,
                        sample,
                        ..
                    } => {
                        flow_count(initial_read)
                            + flow_count(setup)
                            + flow_count(write_candidate)
                            + flow_count(sample)
                    }
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
                        DraftReferenceEvent::IndexedMmio { .. }
                        | DraftReferenceEvent::PollMmio { .. } => 1,
                        DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                        DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                            flow_count(flow)
                        }
                        DraftReferenceEvent::BoundedPoll { body, .. } => flow_count(body),
                        DraftReferenceEvent::PollFlow { body, .. } => flow_count(body),
                        DraftReferenceEvent::SymmetricCalibrationSearch {
                            initial_read,
                            setup,
                            write_candidate,
                            sample,
                            ..
                        } => {
                            flow_count(initial_read)
                                + flow_count(setup)
                                + flow_count(write_candidate)
                                + flow_count(sample)
                        }
                        _ => 0,
                    })
                    .sum()
            },
            flow_count,
        )
    }

    pub fn is_exact(&self) -> bool {
        fn event_contains_reference_only_control_flow(event: &DraftReferenceEvent) -> bool {
            match event {
                DraftReferenceEvent::PollMmio { .. } => true,
                DraftReferenceEvent::ComposedCall { flow, .. } => {
                    flow_contains_reference_only_control_flow(flow)
                }
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                    flow_contains_reference_only_control_flow(flow)
                }
                DraftReferenceEvent::BoundedPoll { .. } => true,
                DraftReferenceEvent::PollFlow { .. } => true,
                DraftReferenceEvent::SymmetricCalibrationSearch { .. } => true,
                _ => false,
            }
        }

        fn flow_contains_reference_only_control_flow(flow: &DraftReferenceFlow) -> bool {
            flow.events
                .iter()
                .any(event_contains_reference_only_control_flow)
                || match &flow.terminator {
                    DraftReferenceTerminator::Return(_) => false,
                    DraftReferenceTerminator::Branch {
                        taken, not_taken, ..
                    } => {
                        flow_contains_reference_only_control_flow(taken)
                            || flow_contains_reference_only_control_flow(not_taken)
                    }
                }
        }

        self.reference_flow.is_none()
            && self.blockers.is_empty()
            && !self
                .reference_events
                .iter()
                .any(event_contains_reference_only_control_flow)
            && self
                .events
                .iter()
                .all(|event| event.unmapped_address().is_none())
    }

    pub fn is_reference_eligible(&self) -> bool {
        self.blockers.is_empty()
            && self.reference_blockers.is_empty()
            && self.unresolved_branch.is_none()
            && self.reference_observables_are_mapped()
            && self.reference_calls_are_valid()
    }

    pub fn reference_observables_are_mapped(&self) -> bool {
        fn flow_is_mapped(flow: &DraftReferenceFlow) -> bool {
            flow.events.iter().all(|event| match event {
                DraftReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                DraftReferenceEvent::IndexedMmio { registers, .. }
                | DraftReferenceEvent::PollMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::BoundedPoll { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::PollFlow { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    flow_is_mapped(initial_read)
                        && flow_is_mapped(setup)
                        && flow_is_mapped(write_candidate)
                        && flow_is_mapped(sample)
                }
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
                DraftReferenceEvent::IndexedMmio { registers, .. }
                | DraftReferenceEvent::PollMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::BoundedPoll { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::PollFlow { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    flow_is_mapped(initial_read)
                        && flow_is_mapped(setup)
                        && flow_is_mapped(write_candidate)
                        && flow_is_mapped(sample)
                }
                _ => true,
            }
        }

        self.reference_flow.as_ref().map_or_else(
            || self.reference_events.iter().all(reference_event_is_mapped),
            flow_is_mapped,
        )
    }

    pub fn reference_calls_are_valid(&self) -> bool {
        if let Some(flow) = &self.reference_flow {
            reference_flow_calls_are_valid(flow)
        } else {
            validate_reference_events(&self.reference_events, BTreeMap::new()).is_some()
        }
    }

    pub fn reference_exit_return_modeled(&self) -> bool {
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
