//! Flat reference-summary substitution and token remapping.

use super::*;

pub(super) fn validate_deferred_memory_address(
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
        .filter(|event| {
            matches!(
                event,
                DraftReferenceEvent::ExternalCall { .. }
                    | DraftReferenceEvent::ModeledDirectCall { .. }
            )
        })
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
                site,
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
                    site: *site,
                    table: *table,
                    function: *function,
                    arguments: mapped_arguments,
                }
            }
            DraftReferenceEvent::ModeledDirectCall {
                site,
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
                DraftReferenceEvent::ModeledDirectCall {
                    token,
                    site: *site,
                    function: function.clone(),
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
