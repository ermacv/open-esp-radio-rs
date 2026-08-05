//! Reference CFG construction and call composition.

mod flow;
mod resolver;
use flow::{
    ReferenceCalleeContext, compose_calls_in_reference_flow, explore_reference_flow,
    resolve_reference_callee, trace_into_reference_flow,
};
pub use resolver::{ReferenceResolver, ReferenceSymbolKey};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use super::static_analysis::{
    StructuralCallSite, StructuralPointerContext, StructuralRelocatedCalls, SymbolicStack,
    is_reference_only_blocker, trace_binary_symbol, trace_binary_symbol_with_branches,
};
use crate::{
    DEFERRED_CALLER_MEMORY_REGION, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, FunctionAnalysis, IndexedMmioGuard, MemoryAccess, MmioRegisterMap,
    ObservableEvent, RV32_MODELED_ARGUMENT_COUNT, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, Result, Rv32CallArguments, SECONDARY_CALL_RESULT_TOKEN_FLAG,
    SymbolicValue, artifact, execution, reference_event_is_mmio_read,
    reference_flow_calls_are_valid,
};

fn validate_deferred_memory_address(
    region: &str,
    address: &SymbolicValue,
) -> std::result::Result<(), String> {
    if region == DEFERRED_CALLER_MEMORY_REGION && !address.caller_memory_address() {
        return Err(format!(
            "deferred memory address {} did not resolve to affine caller-owned RAM",
            address.canonical()
        ));
    }
    Ok(())
}

pub fn inline_reference_summary(
    prefix: &[DraftReferenceEvent],
    callee: &FunctionAnalysis,
    arguments: &Rv32CallArguments,
    mut private_stack: Option<&mut SymbolicStack>,
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
    let mut next_private_stack_read_token = prefix
        .iter()
        .filter(|event| matches!(event, DraftReferenceEvent::PrivateStackLoad { .. }))
        .count() as u32;
    let mut read_tokens = Vec::new();
    let mut memory_read_tokens = Vec::new();
    let mut external_tokens = Vec::new();
    let mut private_stack_reads = BTreeMap::new();

    let substitute = |value: &SymbolicValue,
                      read_tokens: &[u32],
                      memory_read_tokens: &[u32],
                      external_tokens: &[u32],
                      private_stack_reads: &BTreeMap<u32, SymbolicValue>| {
        value
            .substitute(arguments, read_tokens, memory_read_tokens, external_tokens)?
            .rewrite_private_stack_context(private_stack_reads)
    };

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
                let value = substitute(
                    value,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
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
                let address = substitute(
                    address,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: substitute(
                                &guard.selector,
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                                &private_stack_reads,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                let value = value
                    .as_ref()
                    .map(|value| {
                        substitute(
                            value,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &private_stack_reads,
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
            DraftReferenceEvent::PollMmio {
                width,
                address,
                registers,
                guard,
                mask,
                expected,
            } => {
                let address = substitute(
                    address,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
                )?;
                let guard = guard
                    .as_ref()
                    .map(|guard| -> std::result::Result<IndexedMmioGuard, String> {
                        Ok(IndexedMmioGuard {
                            selector: substitute(
                                &guard.selector,
                                &read_tokens,
                                &memory_read_tokens,
                                &external_tokens,
                                &private_stack_reads,
                            )?,
                            maximum: guard.maximum,
                        })
                    })
                    .transpose()?;
                DraftReferenceEvent::PollMmio {
                    width: *width,
                    address,
                    registers: registers.clone(),
                    guard,
                    mask: *mask,
                    expected: *expected,
                }
            }
            DraftReferenceEvent::DelayMicros { micros } => {
                let micros = substitute(
                    micros,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
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
                let address = substitute(
                    address,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
                )?;
                if let Some(offset) = address.private_stack_offset() {
                    let token = next_private_stack_read_token;
                    next_private_stack_read_token += 1;
                    memory_read_tokens.push(crate::PRIVATE_STACK_READ_TOKEN_FLAG | token);
                    let value = private_stack
                        .as_deref()
                        .and_then(|stack| stack.load(offset, *width, false))
                        .ok_or_else(|| {
                            format!(
                                "callee {} reads uninitialized caller private stack at {offset:+#x}",
                                callee.symbol
                            )
                        })?;
                    private_stack_reads.insert(token, value);
                    continue;
                }
                if !address.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory-read address that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                validate_deferred_memory_address(region, &address)?;
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
                let address = substitute(
                    address,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
                )?;
                let value = substitute(
                    value,
                    &read_tokens,
                    &memory_read_tokens,
                    &external_tokens,
                    &private_stack_reads,
                )?;
                if let Some(offset) = address.private_stack_offset() {
                    private_stack
                        .as_deref_mut()
                        .ok_or_else(|| {
                            format!(
                                "callee {} writes caller private stack without composition state",
                                callee.symbol
                            )
                        })?
                        .store(offset, *width, &value);
                    continue;
                }
                if !address.is_resolved() || !value.is_resolved() {
                    return Err(format!(
                        "callee {} has a memory write that is unresolved after argument substitution",
                        callee.symbol
                    ));
                }
                validate_deferred_memory_address(region, &address)?;
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
                        substitute(
                            value,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                let token = next_external_token;
                next_external_token += 1;
                external_tokens.push(token);
                DraftReferenceEvent::ExternalCall {
                    token,
                    table: *table,
                    function: *function,
                    arguments: mapped_arguments,
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
                        substitute(
                            value,
                            &read_tokens,
                            &memory_read_tokens,
                            &external_tokens,
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                DraftReferenceEvent::DiagnosticCall {
                    function: function.clone(),
                    argument_count: *argument_count,
                    arguments: mapped_arguments,
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
    let return_value = substitute(
        &callee.return_value,
        &read_tokens,
        &memory_read_tokens,
        &external_tokens,
        &private_stack_reads,
    )?;
    Ok((output, return_value))
}

pub fn resolve_reference_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    symbols_by_address: &BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    svd: &MmioRegisterMap,
    visiting: &mut BTreeSet<u32>,
) -> Result<FunctionAnalysis> {
    if let Some(mut trace) = pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.reference_intrinsic)(symbol, svd, pointer_context))
    {
        if let Some(flow) = trace.reference_flow.take() {
            let original_flow = flow.clone();
            match compose_calls_in_reference_flow(
                flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    pointer_context,
                    svd,
                },
                visiting,
                &mut trace.reference_dependencies,
            ) {
                Ok(flow) if reference_flow_calls_are_valid(&flow) => {
                    trace.reference_flow = Some(flow);
                }
                Ok(flow) => {
                    trace.reference_flow = Some(flow);
                    trace.reference_blockers.push(
                        "reviewed-summary: composed call result is used without a modeled callee `a0`"
                            .to_owned(),
                    );
                }
                Err(error) => {
                    trace.reference_flow = Some(original_flow);
                    trace
                        .reference_blockers
                        .push(format!("reviewed-summary: {error}"));
                }
            }
        }
        return Ok(trace);
    }
    let mut trace = trace_binary_symbol(
        symbol,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
    )?;
    trace
        .blockers
        .retain(|blocker| !is_reference_only_blocker(blocker));
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
    let has_private_stack_events = trace.reference_events.iter().any(|event| {
        matches!(
            event,
            DraftReferenceEvent::PrivateStackLoad { .. }
                | DraftReferenceEvent::PrivateStackStore { .. }
        )
    });
    if trace.unresolved_branch.is_some() {
        match explore_reference_flow(
            symbol,
            svd,
            relocated_calls,
            pointer_context,
            specialized_arguments,
        )
        .and_then(|flow| {
            compose_calls_in_reference_flow(
                flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    pointer_context,
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
    if typed_calls == 0 && !has_private_stack_events {
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
                            &private_stack_reads,
                        )
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                let mapped_token = output
                    .iter()
                    .filter(|event| matches!(event, DraftReferenceEvent::ExternalCall { .. }))
                    .count() as u32;
                external_tokens.push(mapped_token);
                output.push(DraftReferenceEvent::ExternalCall {
                    token: mapped_token,
                    table: *table,
                    function: *function,
                    arguments,
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
