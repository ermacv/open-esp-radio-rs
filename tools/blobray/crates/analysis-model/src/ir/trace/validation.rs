//! Fail-closed reference event and control-flow validation.

use super::*;

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
        DraftReferenceTerminator::FailStop {
            argument_count,
            arguments,
            ..
        } => arguments
            .iter()
            .take(usize::from(*argument_count))
            .all(|value| value_is_reference_ready(value, &available)),
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
    value.tree().all(|value| match value {
        SymbolicValue::CallResult(token) => available.get(token).copied() == Some(true),
        SymbolicValue::Bits(bits) => bits.iter().all(|source| match source {
            BitSource::CallResult { call_token, .. } => {
                available.get(call_token).copied() == Some(true)
            }
            _ => true,
        }),
        _ => true,
    })
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
    if value.private_stack_offset().is_some() {
        Some("private-stack pointer requires an explicit external input-memory model")
    } else if !value.is_resolved() {
        Some("symbolic value is unresolved")
    } else if !value_call_results_available(value, available) {
        Some("symbolic value depends on an unavailable call result")
    } else {
        None
    }
}

pub(super) fn validate_reference_events_detailed(
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
            DraftReferenceEvent::ReviewedExternalCall {
                site,
                candidates,
                arguments,
                ..
            } => {
                let name = candidates
                    .first()
                    .map_or("<unknown-reviewed-call>", |candidate| {
                        candidate.name.as_str()
                    });
                arguments.iter().enumerate().find_map(|(index, value)| {
                    value_error(
                        &format!("external call `{name}` at {site:#010x} argument {index}"),
                        value,
                    )
                })
            }
            DraftReferenceEvent::ModeledDirectCall {
                site,
                function,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(function.argument_count))
                .enumerate()
                .find_map(|(index, value)| {
                    value_error(
                        &format!(
                            "modeled direct call `{}` at {site:#010x} argument {index}",
                            function.name
                        ),
                        value,
                    )
                }),
            DraftReferenceEvent::DiagnosticCall {
                site,
                function,
                argument_count,
                arguments,
                ..
            } => arguments
                .iter()
                .take(usize::from(*argument_count))
                .enumerate()
                .find_map(|(index, value)| {
                    value_error(
                        &format!("diagnostic call `{function}` at {site:#010x} argument {index}"),
                        value,
                    )
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
    reference_flow_call_validation_error(flow).is_none()
}

/// Explain the first unavailable call result in a structured reference flow.
///
/// The boolean predicate remains convenient for gates, but callers producing
/// evidence must not collapse an exact failing branch/event into a generic
/// "callee a0" message.
pub fn reference_flow_call_validation_error(flow: &DraftReferenceFlow) -> Option<String> {
    validate_reference_flow_with_calls_detailed(flow, BTreeMap::new()).err()
}

pub(super) fn validate_reference_flow_with_calls_detailed(
    flow: &DraftReferenceFlow,
    available: BTreeMap<u32, bool>,
) -> std::result::Result<(), String> {
    let available = validate_reference_events_detailed(&flow.events, available)?;
    match &flow.terminator {
        DraftReferenceTerminator::Return(_) => Ok(()),
        DraftReferenceTerminator::FailStop {
            site,
            argument_count,
            arguments,
            ..
        } => {
            for (index, argument) in arguments
                .iter()
                .take(usize::from(*argument_count))
                .enumerate()
            {
                if let Some(error) = reference_value_error(argument, &available) {
                    return Err(format!(
                        "fail-stop at {site:#010x} argument {index}: {error}"
                    ));
                }
            }
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use crate::{FloatingPointOperation, FloatingRoundingMode};

    use super::*;

    #[test]
    fn floating_value_rejects_an_unavailable_nested_call_result() {
        let value = SymbolicValue::FloatingPoint {
            operation: FloatingPointOperation::SignedWordToSingle,
            rounding: FloatingRoundingMode::TowardZero,
            operands: vec![SymbolicValue::CallResult(4)].into_boxed_slice(),
        };

        assert!(!value_call_results_available(&value, &BTreeMap::new()));
        assert!(value_call_results_available(
            &value,
            &BTreeMap::from([(4, true)])
        ));
    }

    #[test]
    fn call_validation_reports_the_exact_branch_using_an_unmodeled_result() {
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ComposedCall {
                token: 0,
                symbol: "opaque_callback".to_owned(),
                arguments: Box::new([]),
                flow: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
                }),
                result_modeled: false,
            }],
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x4000_1234,
                    operation: BranchOperation::NotEqual,
                    left: SymbolicValue::CallResult(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
            },
        };

        assert_eq!(
            reference_flow_call_validation_error(&flow).as_deref(),
            Some(
                "branch at 0x40001234 left operand: symbolic value depends on an unavailable call result"
            )
        );
    }

    #[test]
    fn external_call_requires_a_model_before_consuming_private_stack_memory() {
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ReviewedExternalCall {
                token: 0,
                site: 0x4000_5678,
                candidates: vec![ReviewedExternalCall {
                    id: "queue-send".to_owned(),
                    contract: "test-rtos@+0x10".to_owned(),
                    name: "queue_send".to_owned(),
                    argument_types: vec!["opaque-handle".to_owned(), "const-ptr".to_owned()],
                    return_type: "i32".to_owned(),
                    variadic: false,
                    semantic_operation: Some("rtos.queue.send".to_owned()),
                    replacement_hint: None,
                    execution_model: None,
                    tail: false,
                    evidence: ReviewedExternalCallEvidence::ObservedCallSite,
                    slot_load_site: Some(0x4000_5670),
                }],
                arguments: vec![SymbolicValue::Constant(1), SymbolicValue::StackAddress(-16)]
                    .into_boxed_slice(),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
        };

        assert_eq!(
            reference_flow_call_validation_error(&flow).as_deref(),
            Some(
                "event 0 external call `queue_send` at 0x40005678 argument 1: private-stack pointer requires an explicit external input-memory model"
            )
        );
    }
}
