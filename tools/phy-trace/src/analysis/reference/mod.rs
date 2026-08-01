//! Reference CFG construction and call composition.

mod flow;
mod resolver;
use flow::{
    ReferenceCalleeContext, compose_calls_in_reference_flow, explore_reference_flow,
    resolve_reference_callee, trace_into_reference_flow,
};
pub(crate) use resolver::ReferenceResolver;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use super::direct::{
    StructuralCallSite, StructuralRelocatedCalls, trace_binary_symbol,
    trace_binary_symbol_with_branches,
};
use crate::{
    DraftReferenceEvent, DraftReferenceFlow, DraftReferenceTerminator, FunctionAnalysis,
    IndexedMmioGuard, MemoryAccess, MmioRegisterMap, ObservableEvent, Result, SymbolicValue,
    artifact, execution, external_abi, reference_event_is_mmio_read,
    reference_flow_calls_are_valid,
};

pub(crate) fn inline_reference_summary(
    prefix: &[DraftReferenceEvent],
    callee: &FunctionAnalysis,
    arguments: &[SymbolicValue; 8],
) -> std::result::Result<(Vec<DraftReferenceEvent>, SymbolicValue), String> {
    if callee.reference_flow.is_some() {
        return Err(format!(
            "callee {} contains symbolic control flow and must be represented as a scoped call before flattening",
            callee.symbol
        ));
    }
    let mut output = prefix.to_vec();
    let mut next_read_token = prefix
        .iter()
        .filter(|event| reference_event_is_mmio_read(event))
        .count() as u32;
    let mut next_memory_read_token = prefix
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
    let mut next_external_token = prefix
        .iter()
        .filter(|event| matches!(event, DraftReferenceEvent::ExternalCall { .. }))
        .count() as u32;
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();

    for event in &callee.reference_events {
        let event = match event {
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                ..
            }) => {
                read_tokens.push(next_read_token);
                next_read_token += 1;
                event.clone()
            }
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width,
                address,
                register,
                value: Some(value),
            }) => {
                let value = value.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !value.is_resolved() {
                    return Err(format!(
                        "callee {} has a write that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                DraftReferenceEvent::Observable(ObservableEvent::Memory {
                    access: MemoryAccess::Write,
                    width: *width,
                    address: *address,
                    register: register.clone(),
                    value: Some(value),
                })
            }
            DraftReferenceEvent::IndexedMmio {
                access,
                width,
                address,
                registers,
                guard,
                value,
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: guard.selector.substitute(
                                arguments,
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                let value = value
                    .as_ref()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .transpose()?;
                if *access == MemoryAccess::Read {
                    read_tokens.push(next_read_token);
                    next_read_token += 1;
                }
                DraftReferenceEvent::IndexedMmio {
                    access: *access,
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    value,
                }
            }
            DraftReferenceEvent::DelayMicros { micros } => {
                let micros = micros.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !micros.is_resolved() {
                    return Err(format!(
                        "callee {} has an unresolved delay after argument substitution",
                        callee.symbol
                    ));
                }
                DraftReferenceEvent::DelayMicros { micros }
            }
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width,
                address,
                region,
                value: None,
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !address.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory-read address that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                memory_read_tokens.push(next_memory_read_token);
                next_memory_read_token += 1;
                DraftReferenceEvent::Memory {
                    access: MemoryAccess::Read,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: None,
                }
            }
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width,
                address,
                region,
                value: Some(value),
            } => {
                let address = address.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                let value = value.substitute(
                    arguments,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                )?;
                if !address.is_resolved() || !value.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory write that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                DraftReferenceEvent::Memory {
                    access: MemoryAccess::Write,
                    width: *width,
                    address,
                    region: region.clone(),
                    value: Some(value),
                }
            }
            DraftReferenceEvent::ExternalCall {
                table,
                function,
                arguments: external_arguments,
                ..
            } => {
                let mapped_arguments = external_arguments
                    .iter()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal external argument count changed".to_owned())?;
                let token = next_external_token;
                next_external_token += 1;
                external_tokens.push(token);
                DraftReferenceEvent::ExternalCall {
                    token,
                    table: *table,
                    function: *function,
                    arguments: Box::new(mapped_arguments),
                }
            }
            DraftReferenceEvent::DiagnosticCall {
                function,
                argument_count,
                arguments: diagnostic_arguments,
            } => {
                let mapped_arguments = diagnostic_arguments
                    .iter()
                    .map(|value| {
                        value.substitute(
                            arguments,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal diagnostic argument count changed".to_owned())?;
                DraftReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments: Box::new(mapped_arguments),
                }
            }
            DraftReferenceEvent::TailCall { site, target, .. } => {
                return Err(format!(
                    "callee {} still contains an unresolved call at {site:#010x} to {target:#010x}",
                    callee.symbol
                ));
            }
            DraftReferenceEvent::Call {
                token,
                site,
                target,
                ..
            } => {
                return Err(format!(
                    "callee {} still contains unresolved call {token} at {site:#010x} to {target:#010x}",
                    callee.symbol
                ));
            }
            _ => event.clone(),
        };
        output.push(event);
    }
    let return_value = callee.return_value.substitute(
        arguments,
        &read_tokens,
        &memory_read_tokens,
        &external_tokens,
    )?;
    Ok((output, return_value))
}

pub(crate) fn resolve_reference_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    symbols_by_address: &BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[SymbolicValue; 8]>,
    svd: &MmioRegisterMap,
    visiting: &mut BTreeSet<u32>,
) -> Result<FunctionAnalysis> {
    let mut trace = trace_binary_symbol(
        symbol,
        svd,
        relocated_calls,
        external_pointer_cells,
        specialized_arguments,
    )?;
    let typed_calls = trace
        .reference_events
        .iter()
        .filter(|event| {
            matches!(
                event,
                DraftReferenceEvent::TailCall { .. }
                    | DraftReferenceEvent::Call { .. }
                    | DraftReferenceEvent::DiagnosticCall { .. }
            )
        })
        .count();
    if trace.unresolved_branch.is_some() {
        match explore_reference_flow(
            symbol,
            svd,
            relocated_calls,
            external_pointer_cells,
            specialized_arguments,
        )
        .and_then(|flow| {
            compose_calls_in_reference_flow(
                flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    external_pointer_cells,
                    svd,
                },
                visiting,
                &mut trace.reference_dependencies,
            )
        }) {
            Ok(flow) if reference_flow_calls_are_valid(&flow) => {
                trace.events.clear();
                trace.reference_events.clear();
                trace.blockers.clear();
                trace.reference_flow = Some(flow);
                trace.unresolved_branch = None;
            }
            Ok(_) => trace.reference_blockers.push(
                "symbolic-cfg: composed call result is used without a modeled callee `a0`"
                    .to_owned(),
            ),
            Err(error) => trace
                .reference_blockers
                .push(format!("symbolic-cfg: {error}")),
        }
        return Ok(trace);
    }
    if typed_calls == 0 {
        return Ok(trace);
    }

    let call_blockers = trace
        .blockers
        .iter()
        .filter(|blocker| blocker.starts_with("call/jump instruction"))
        .count();
    if typed_calls != call_blockers {
        trace.reference_blockers.push(format!(
            "unsupported-call-shape: typed-calls={typed_calls} call-blockers={call_blockers}"
        ));
        return Ok(trace);
    }

    let source_events = std::mem::take(&mut trace.reference_events);
    let mut output = Vec::new();
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();
    let mut call_results = BTreeMap::<u32, SymbolicValue>::new();
    let mut tail_return = None;
    for (index, event) in source_events.iter().enumerate() {
        let result = match event {
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
            DraftReferenceEvent::DelayMicros { micros } => micros
                .rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
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
                )?;
                if !address.is_resolved() {
                    return Err("memory-read address after a call remains unresolved".to_owned());
                }
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
                )?;
                let value = value.rewrite_call_context(
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &call_results,
                )?;
                if !address.is_resolved() || !value.is_resolved() {
                    return Err("memory write after a call remains unresolved".to_owned());
                }
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
            DraftReferenceEvent::ExternalCall {
                token,
                table,
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
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal external argument count changed".to_owned())?;
                let mapped_token = output
                    .iter()
                    .filter(|event| matches!(event, DraftReferenceEvent::ExternalCall { .. }))
                    .count() as u32;
                external_tokens.push(mapped_token);
                output.push(DraftReferenceEvent::ExternalCall {
                    token: mapped_token,
                    table: *table,
                    function: *function,
                    arguments: Box::new(arguments),
                });
                if usize::try_from(*token).ok() != Some(external_tokens.len() - 1) {
                    return Err(format!(
                        "external call token {token} is not ordered in the source trace"
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
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .try_into()
                    .map_err(|_| "internal diagnostic argument count changed".to_owned())?;
                output.push(DraftReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments: Box::new(arguments),
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
                                )
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?
                            .try_into()
                            .map_err(|_| "internal call argument count changed".to_owned())?;
                        let (callee_name, callee_trace) = resolve_reference_callee(
                            *target,
                            *site,
                            &arguments,
                            &ReferenceCalleeContext {
                                symbols_by_address,
                                relocated_calls,
                                external_pointer_cells,
                                svd,
                            },
                            visiting,
                        )?;
                        let requires_scoped_call = callee_trace.reference_flow.is_some()
                            || callee_trace.reference_events.iter().any(|event| {
                                matches!(event, DraftReferenceEvent::ComposedCall { .. })
                            });
                        let callee_dependencies = callee_trace.reference_dependencies.clone();
                        trace.reference_dependencies.push(callee_name.clone());
                        trace.reference_dependencies.extend(callee_dependencies);
                        if requires_scoped_call {
                            let result_modeled = callee_trace.reference_exit_a0_modeled();
                            let mapped_token = output
                                .iter()
                                .filter(|event| {
                                    matches!(event, DraftReferenceEvent::ComposedCall { .. })
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
                            let (events, return_value) =
                                inline_reference_summary(&output, &callee_trace, &arguments)?;
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
