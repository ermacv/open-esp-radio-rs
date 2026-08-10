//! Straight-line call composition and caller-context rewriting.

use super::inline::validate_deferred_memory_address;
use super::*;

pub(super) fn flatten_reference_trace(
    mut trace: FunctionAnalysis,
    symbols_by_address: &BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    svd: &MmioMap,
    visiting: &mut BTreeSet<u32>,
    budget: StructuralTraceBudget,
) -> Result<FunctionAnalysis> {
    let source_events = std::mem::take(&mut trace.reference_events);
    let mut output = Vec::new();
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();
    let mut call_results = BTreeMap::<u32, SymbolicValue>::new();
    let mut private_stack_reads = BTreeMap::<u32, SymbolicValue>::new();
    let mut private_stack = SymbolicStack::default();
    for index in 0..RV32_STACK_ARGUMENT_COUNT {
        let argument_index = RV32_REGISTER_ARGUMENT_COUNT + index;
        let value = specialized_arguments
            .and_then(|arguments| arguments[argument_index].as_constant())
            .map_or_else(
                || SymbolicValue::input(argument_index as u8),
                SymbolicValue::Constant,
            );
        private_stack.store((index * 4) as i32, 32, &value);
    }
    let mut tail_return = None;
    for (index, event) in source_events.iter().enumerate() {
        let result = match event {
            DraftReferenceEvent::PrivateStackStore {
                offset,
                width,
                value,
            } => value
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )
                .map(|value| private_stack.store(*offset, *width, &value)),
            DraftReferenceEvent::PrivateStackLoad {
                token,
                offset,
                width,
                signed,
            } => private_stack
                .load(*offset, *width, *signed)
                .ok_or_else(|| {
                    format!(
                        "private-stack read {token} at {offset:+#x} is not definitely initialized"
                    )
                })
                .and_then(|value| {
                    if private_stack_reads.insert(*token, value).is_some() {
                        Err(format!("private-stack read token {token} is duplicated"))
                    } else {
                        Ok(())
                    }
                }),
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                ..
            }) => {
                let token = output
                    .iter()
                    .filter(|event| reference_event_is_mmio_read(event))
                    .count() as u32;
                read_tokens.push(token);
                output.push(event.clone());
                Ok(())
            }
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width,
                address,
                register,
                value: Some(value),
            }) => value
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )
                .and_then(|value| {
                    value.is_resolved().then_some(value).ok_or_else(|| {
                        format!("MMIO write after a call remains unresolved at {address:#010x}")
                    })
                })
                .map(|value| {
                    output.push(DraftReferenceEvent::Observable(ObservableEvent::Memory {
                        access: MemoryAccess::Write,
                        width: *width,
                        address: *address,
                        register: register.clone(),
                        value: Some(value),
                    }));
                }),
            DraftReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: guard.selector.rewrite_call_context(
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                                &call_results,
                                &private_stack_reads,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                let value = value
                    .as_ref()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                            &private_stack_reads,
                        )
                    })
                    .transpose()?;
                if *access == MemoryAccess::Read {
                    let token = output
                        .iter()
                        .filter(|event| reference_event_is_mmio_read(event))
                        .count() as u32;
                    read_tokens.push(token);
                }
                output.push(DraftReferenceEvent::IndexedMmio {
                    access: *access,
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    value,
                });
                Ok(())
            })(),
            DraftReferenceEvent::PollMmio {
                width,
                address,
                registers,
                guard,
                mask,
                expected,
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: guard.selector.rewrite_call_context(
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                                &call_results,
                                &private_stack_reads,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                output.push(DraftReferenceEvent::PollMmio {
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    mask: *mask,
                    expected: *expected,
                });
                Ok(())
            })(),
            DraftReferenceEvent::DelayMicros { micros } => micros
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )
                .and_then(|micros| {
                    micros
                        .is_resolved()
                        .then_some(micros)
                        .ok_or_else(|| "delay after a call remains unresolved".to_owned())
                })
                .map(|micros| output.push(DraftReferenceEvent::DelayMicros { micros })),
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width,
                address,
                region,
                value: None,
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )?;
                if !address.is_resolved() {
                    return Err("memory-read address after a call remains unresolved".to_owned());
                }
                validate_deferred_memory_address(region, &address)?;
                let token = output
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            DraftReferenceEvent::Memory {
                                access: MemoryAccess::Read,
                                ..
                            }
                        )
                    })
                    .count() as u32;
                memory_read_tokens.push(token);
                output.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Read,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: None,
                });
                Ok(())
            })(),
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width,
                address,
                region,
                value: Some(value),
            } => (|| -> std::result::Result<(), String> {
                let address = address.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )?;
                let value = value.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                    &private_stack_reads,
                )?;
                if !address.is_resolved() || !value.is_resolved() {
                    return Err("memory write after a call remains unresolved".to_owned());
                }
                validate_deferred_memory_address(region, &address)?;
                output.push(DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: Some(value),
                });
                Ok(())
            })(),
            DraftReferenceEvent::Observable(ObservableEvent::Fence { .. }) => {
                output.push(event.clone());
                Ok(())
            }
            DraftReferenceEvent::ReviewedExternalCall {
                token,
                site,
                candidates,
                arguments,
            } => (|| -> std::result::Result<(), String> {
                let arguments = arguments
                    .iter()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                let mapped_token = output
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            DraftReferenceEvent::ReviewedExternalCall { .. }
                                | DraftReferenceEvent::ModeledDirectCall { .. }
                        )
                    })
                    .count() as u32;
                external_tokens.push(mapped_token);
                output.push(DraftReferenceEvent::ReviewedExternalCall {
                    token: mapped_token,
                    site: *site,
                    candidates: candidates.clone(),
                    arguments,
                });
                if usize::try_from(*token).ok() != Some(external_tokens.len() - 1) {
                    return Err(format!(
                        "external call token {token} is not ordered in the source trace"
                    ));
                }
                Ok(())
            })(),
            DraftReferenceEvent::ModeledDirectCall {
                token,
                site,
                function,
                arguments,
            } => (|| -> std::result::Result<(), String> {
                let arguments = arguments
                    .iter()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                let mapped_token = output
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            DraftReferenceEvent::ReviewedExternalCall { .. }
                                | DraftReferenceEvent::ModeledDirectCall { .. }
                        )
                    })
                    .count() as u32;
                external_tokens.push(mapped_token);
                output.push(DraftReferenceEvent::ModeledDirectCall {
                    token: mapped_token,
                    site: *site,
                    function: function.clone(),
                    arguments,
                });
                if usize::try_from(*token).ok() != Some(external_tokens.len() - 1) {
                    return Err(format!(
                        "modeled direct call token {token} is not ordered in the source trace"
                    ));
                }
                Ok(())
            })(),
            DraftReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments,
            } => (|| -> std::result::Result<(), String> {
                let arguments = arguments
                    .iter()
                    .map(|value| {
                        value.rewrite_call_context(
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &call_results,
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                output.push(DraftReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments,
                });
                Ok(())
            })(),
            DraftReferenceEvent::Call {
                token: source_call_token,
                site,
                target,
                arguments,
            }
            | DraftReferenceEvent::TailCall {
                token: source_call_token,
                site,
                target,
                arguments,
            } => {
                let is_tail = matches!(event, DraftReferenceEvent::TailCall { .. });
                if is_tail && index + 1 != source_events.len() {
                    Err(format!(
                        "tail-call-not-terminal at {site:#010x} to {target:#010x}"
                    ))
                } else {
                    (|| -> std::result::Result<(), String> {
                        let arguments = arguments
                            .iter()
                            .map(|value| {
                                value.rewrite_call_context(
                                    &read_tokens,
                                    &memory_read_tokens,
                                    &external_tokens,
                                    &call_results,
                                    &private_stack_reads,
                                )
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?
                            .try_into()
                            .map_err(|_| "internal call argument count changed".to_owned())?;
                        if let Some(callee) = symbols_by_address.get(target)
                            && pointer_context.summary_hooks.is_some_and(|hooks| {
                                (hooks.wide_signed_divide)(callee, &arguments).is_some()
                            })
                        {
                            let mapped_token = output
                                .iter()
                                .filter(|event| {
                                    matches!(
                                        event,
                                        DraftReferenceEvent::ComposedCall { .. }
                                            | DraftReferenceEvent::WideSignedDivide { .. }
                                    )
                                })
                                .count() as u32;
                            output.push(DraftReferenceEvent::WideSignedDivide {
                                token: mapped_token,
                                dividend_low: arguments[0].clone(),
                                dividend_high: arguments[1].clone(),
                                divisor_low: arguments[2].clone(),
                                divisor_high: arguments[3].clone(),
                            });
                            trace.reference_dependencies.push(callee.name.clone());
                            let return_value = SymbolicValue::CallResult(mapped_token);
                            if is_tail {
                                tail_return = Some(return_value);
                            } else {
                                // The summary expressions refer directly to the call's
                                // rewritten operands. Later values should instead use the
                                // ordered generated result so the operation is evaluated once.
                                call_results.insert(
                                    *source_call_token,
                                    SymbolicValue::CallResult(mapped_token),
                                );
                                call_results.insert(
                                    *source_call_token | SECONDARY_CALL_RESULT_TOKEN_FLAG,
                                    SymbolicValue::CallResult(
                                        mapped_token | SECONDARY_CALL_RESULT_TOKEN_FLAG,
                                    ),
                                );
                            }
                            return Ok(());
                        }
                        let (callee_name, callee_trace) = resolve_reference_callee(
                            *target,
                            *site,
                            &arguments,
                            &ReferenceCalleeContext {
                                symbols_by_address,
                                relocated_calls,
                                pointer_context,
                                svd,
                                budget,
                            },
                            visiting,
                        )?;
                        let requires_scoped_call = callee_trace.reference_flow.is_some()
                            || callee_trace.reference_events.iter().any(|event| {
                                matches!(
                                    event,
                                    DraftReferenceEvent::ComposedCall { .. }
                                        | DraftReferenceEvent::WideSignedDivide { .. }
                                )
                            });
                        let callee_dependencies = callee_trace.reference_dependencies.clone();
                        trace.reference_dependencies.push(callee_name.clone());
                        trace.reference_dependencies.extend(callee_dependencies);
                        if requires_scoped_call {
                            if arguments
                                .iter()
                                .any(|argument| argument.private_stack_offset().is_some())
                            {
                                return Err(format!(
                                    "callee {callee_name} has symbolic control flow over caller private stack; branch-aware memory composition is required"
                                ));
                            }
                            let result_modeled = callee_trace.reference_exit_return_modeled();
                            let mapped_token = output
                                .iter()
                                .filter(|event| {
                                    matches!(
                                        event,
                                        DraftReferenceEvent::ComposedCall { .. }
                                            | DraftReferenceEvent::WideSignedDivide { .. }
                                    )
                                })
                                .count() as u32;
                            output.push(DraftReferenceEvent::ComposedCall {
                                token: mapped_token,
                                symbol: callee_name,
                                arguments: Box::new(arguments),
                                flow: Box::new(trace_into_reference_flow(callee_trace)),
                                result_modeled,
                            });
                            let return_value = if result_modeled {
                                SymbolicValue::CallResult(mapped_token)
                            } else {
                                SymbolicValue::Unknown
                            };
                            if is_tail {
                                tail_return = Some(return_value);
                            } else {
                                call_results.insert(*source_call_token, return_value);
                            }
                        } else {
                            let (events, return_value) = inline_reference_summary(
                                &output,
                                &callee_trace,
                                &arguments,
                                Some(&mut private_stack),
                            )?;
                            output = events;
                            if is_tail {
                                tail_return = Some(return_value);
                            } else {
                                call_results.insert(*source_call_token, return_value);
                            }
                        }
                        Ok(())
                    })()
                }
            }
            _ => Err("internal reference event has an invalid value shape".to_owned()),
        };
        if let Err(error) = result {
            trace.reference_events = source_events.clone();
            trace
                .reference_blockers
                .push(format!("call-summary-flattening: {error}"));
            return Ok(trace);
        }
    }

    trace.return_value = if let Some(value) = tail_return {
        value
    } else {
        match trace.return_value.rewrite_call_context(
            &read_tokens,
            &memory_read_tokens,
            &external_tokens,
            &call_results,
            &private_stack_reads,
        ) {
            Ok(value) => value,
            Err(error) => {
                trace.reference_events = source_events.clone();
                trace
                    .reference_blockers
                    .push(format!("call-return-flattening: {error}"));
                return Ok(trace);
            }
        }
    };
    trace.reference_events = output;
    trace
        .blockers
        .retain(|blocker| !blocker.starts_with("call/jump instruction"));
    Ok(trace)
}
