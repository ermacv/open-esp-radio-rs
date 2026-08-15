//! Relocated, direct, table and external-ABI call semantics.

use super::state::StructuralTraceState;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StructuralCallControl {
    NotCall,
    Advance(usize),
    Stop,
}

fn structural_finish_call(
    values: &mut [SymbolicValue; 32],
    return_address: u32,
    call_token: u32,
    target: u32,
    pointer_context: &StructuralPointerContext,
) {
    structural_finish_call_with_result(
        values,
        return_address,
        SymbolicValue::CallResult(call_token),
    );
    if pointer_context
        .summary_hooks
        .is_some_and(|hooks| (hooks.secondary_return_target)(target))
    {
        structural_set(
            values,
            Reg::A1,
            SymbolicValue::CallResult(call_token | SECONDARY_CALL_RESULT_TOKEN_FLAG),
        );
    }
}

fn structural_finish_call_with_result(
    values: &mut [SymbolicValue; 32],
    return_address: u32,
    result: SymbolicValue,
) {
    for register in [
        Reg::RA,
        Reg::T0,
        Reg::T1,
        Reg::T2,
        Reg::A0,
        Reg::A1,
        Reg::A2,
        Reg::A3,
        Reg::A4,
        Reg::A5,
        Reg::A6,
        Reg::A7,
        Reg::T3,
        Reg::T4,
        Reg::T5,
        Reg::T6,
    ] {
        structural_set(values, register, SymbolicValue::Unknown);
    }
    structural_set(values, Reg::RA, SymbolicValue::Constant(return_address));
    structural_set(values, Reg::A0, result);
}

fn structural_prepare_opaque_call(state: &mut StructuralTraceState, return_address: u32) {
    let arguments = structural_call_arguments(
        &state.values,
        &state.stack,
        state.private_stack_may_be_modified_by_call,
    );
    state.private_stack_may_be_modified_by_call |= arguments
        .iter()
        .any(|argument| argument.private_stack_offset().is_some());
    structural_finish_call_with_result(&mut state.values, return_address, SymbolicValue::Unknown);
}

fn apply_reviewed_external_call(
    pc: u32,
    width: u8,
    instruction: Inst,
    offset: u32,
    dest: Reg,
    mut candidates: Vec<ReviewedExternalCall>,
    state: &mut StructuralTraceState,
) -> StructuralCallControl {
    if offset != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
        state.blockers.push(format!(
            "unsupported reviewed external ABI call shape at {pc:#x}: {instruction}"
        ));
        return StructuralCallControl::Stop;
    }
    let tail = dest == Reg::ZERO;
    if candidates
        .iter()
        .any(|candidate| candidate.slot_load_site.is_some() && candidate.tail != tail)
    {
        state.blockers.push(format!(
            "reviewed external ABI call shape changed at {pc:#x}: {instruction}"
        ));
        return StructuralCallControl::Stop;
    }
    for candidate in &mut candidates {
        if candidate.slot_load_site.is_none() {
            candidate.tail = tail;
        }
    }
    for load_site in candidates
        .iter()
        .filter_map(|candidate| candidate.slot_load_site)
        .collect::<BTreeSet<_>>()
    {
        let prefix = format!("unregistered-external-abi-slot at {load_site:#x}:");
        if let Some(index) = state
            .reference_blockers
            .iter()
            .position(|blocker| blocker.starts_with(&prefix))
        {
            state.reference_blockers.remove(index);
        }
    }
    let argument_count = candidates
        .iter()
        .map(|candidate| candidate.argument_types.len())
        .max()
        .unwrap_or(0);
    let call_arguments = structural_call_arguments(
        &state.values,
        &state.stack,
        state.private_stack_may_be_modified_by_call,
    );
    let mut arguments = call_arguments
        .iter()
        .take(argument_count)
        .cloned()
        .collect::<Vec<_>>()
        .into_boxed_slice();
    state.private_stack_may_be_modified_by_call |= arguments
        .iter()
        .any(|argument| argument.private_stack_offset().is_some());
    let execution_model = match candidates.as_slice() {
        [candidate] => candidate.execution_model.as_ref(),
        _ => None,
    };
    let mut secondary_result = None;
    let result = match execution_model.map(|model| model.return_model) {
        Some(ExternalReturnModel::Void) => SymbolicValue::Unknown,
        Some(ExternalReturnModel::Constant(value)) => SymbolicValue::Constant(value),
        Some(ExternalReturnModel::SymbolicU32) => {
            SymbolicValue::ExternalResult(state.next_external_call_token)
        }
        Some(ExternalReturnModel::SymbolicU64) => {
            secondary_result = Some(SymbolicValue::ExternalResultHigh(
                state.next_external_call_token,
            ));
            SymbolicValue::ExternalResult(state.next_external_call_token)
        }
        Some(ExternalReturnModel::AllocatedZeroed { .. }) => SymbolicValue::ExternalResult(
            state.next_external_call_token | ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG,
        ),
        Some(ExternalReturnModel::OpaquePointer) => SymbolicValue::ExternalResult(
            state.next_external_call_token | OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG,
        ),
        Some(ExternalReturnModel::Unmodeled) | None => {
            state.reference_blockers.push(format!(
                "unmodeled-reviewed-external-call at {pc:#x}: {}",
                candidates
                    .iter()
                    .map(|candidate| format!("{} ({})", candidate.id, candidate.name))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
            // Preserve identity of the opaque result so later control flow
            // can still be explored on both outcomes. The blocker above
            // remains authoritative: this token is an uninterpreted value,
            // not an executable return model and cannot make the trace exact.
            SymbolicValue::ExternalResult(state.next_external_call_token)
        }
    };
    let mut private_stack_outputs = Vec::new();
    if let Some(model) = execution_model {
        for (output_index, output) in model.outputs.iter().enumerate() {
            let ExternalOutputModel::PrivateStack {
                pointer_argument,
                width,
            } = output;
            let Ok(output_index) = u8::try_from(output_index) else {
                state.reference_blockers.push(format!(
                    "unsupported-reviewed-external-output-count at {pc:#x}: {} ({}) has more than 256 outputs",
                    candidates[0].id, candidates[0].name
                ));
                state
                    .reference_events
                    .push(DraftReferenceEvent::ReviewedExternalCall {
                        token: state.next_external_call_token,
                        site: pc,
                        candidates,
                        arguments,
                    });
                state.next_external_call_token += 1;
                return StructuralCallControl::Stop;
            };
            let Some(SymbolicValue::StackAddress(offset)) =
                arguments.get(usize::from(*pointer_argument))
            else {
                state.reference_blockers.push(format!(
                    "unsupported-reviewed-external-output-pointer at {pc:#x}: {} ({}) argument a{pointer_argument} is not private stack",
                    candidates[0].id, candidates[0].name
                ));
                state
                    .reference_events
                    .push(DraftReferenceEvent::ReviewedExternalCall {
                        token: state.next_external_call_token,
                        site: pc,
                        candidates,
                        arguments,
                    });
                state.next_external_call_token += 1;
                return StructuralCallControl::Stop;
            };
            let output = SymbolicValue::ExternalOutput {
                call_token: state.next_external_call_token,
                output_index,
            }
            .and(match width {
                8 => 0xff,
                16 => 0xffff,
                _ => u32::MAX,
            });
            std::sync::Arc::make_mut(&mut state.stack).store(*offset, *width, &output);
            private_stack_outputs.push((*offset, *width, output));
            arguments[usize::from(*pointer_argument)] = SymbolicValue::Constant(0);
        }
    }
    state
        .reference_events
        .push(DraftReferenceEvent::ReviewedExternalCall {
            token: state.next_external_call_token,
            site: pc,
            candidates,
            arguments,
        });
    for (offset, width, value) in private_stack_outputs {
        state
            .reference_events
            .push(DraftReferenceEvent::PrivateStackStore {
                offset,
                width,
                value,
            });
    }
    state.next_external_call_token += 1;
    if dest == Reg::ZERO {
        state.return_value = result;
        return StructuralCallControl::Stop;
    }
    structural_finish_call_with_result(
        &mut state.values,
        pc.wrapping_add(u32::from(width)),
        result,
    );
    if let Some(secondary_result) = secondary_result {
        structural_set(&mut state.values, Reg::A1, secondary_result);
    }
    StructuralCallControl::Advance(1)
}

pub(super) fn apply_relocated_call(
    decoded: artifact::DecodedInstruction,
    next_instruction: Option<artifact::DecodedInstruction>,
    symbol: &artifact::ArtifactSymbolDefinition,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    state: &mut StructuralTraceState,
) -> StructuralCallControl {
    let pc = decoded.address;
    let Some((name, target)) = relocated_calls.get(&StructuralCallSite::new(symbol, pc as u32))
    else {
        return StructuralCallControl::NotCall;
    };

    state.blockers.push(format!(
        "call/jump instruction at {pc:#x}: relocated call to {name}"
    ));
    // A projected origin R_RISCV_CALL may name either the original
    // AUIPC+JALR pair or the authoritative linker's relaxed JAL. Both forms
    // carry the same origin call identity, but consume different instruction
    // counts and install different return PCs.
    let (dest, return_pc, instruction_count) = if let Inst::Jal { dest, .. } = decoded.instruction {
        (dest, (pc as u32).wrapping_add(u32::from(decoded.width)), 1)
    } else {
        let Some(jalr) = next_instruction else {
            state.reference_blockers.push(format!(
                "malformed-call-relocation at {pc:#x}: {name} has no following JALR"
            ));
            return StructuralCallControl::Stop;
        };
        if jalr.address != pc.wrapping_add(4) {
            state.reference_blockers.push(format!(
                "malformed-call-relocation at {pc:#x}: {name} is not a two-instruction call"
            ));
            return StructuralCallControl::Stop;
        }
        let Inst::Jalr { dest, .. } = jalr.instruction else {
            state.reference_blockers.push(format!(
                "malformed-call-relocation at {pc:#x}: {name} is not followed by JALR"
            ));
            return StructuralCallControl::Stop;
        };
        (dest, (pc as u32).wrapping_add(8), 2)
    };

    let intrinsic_event_start = state.reference_events.len();
    let standard_memory_function = pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.standard_memory_function)(name));
    if target.is_none()
        && let Some(function) = standard_memory_function
        && let Some(result) = inline_standard_memory_intrinsic(
            function,
            &core::array::from_fn(|index| {
                if index < RV32_REGISTER_ARGUMENT_COUNT {
                    state.values[10 + index].clone()
                } else {
                    SymbolicValue::Unknown
                }
            }),
            symbol,
            std::sync::Arc::make_mut(&mut state.stack),
            &mut state.reference_events,
            &mut state.next_memory_read_token,
        )
    {
        state.locate_reference_events_since(pc as u32, intrinsic_event_start);
        if !matches!(dest, Reg::ZERO | Reg::RA) {
            state.reference_blockers.push(format!(
                "unsupported-memory-intrinsic-link-register at {pc:#x}: {name} uses {dest}"
            ));
            return StructuralCallControl::Stop;
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                state
                    .reference_blockers
                    .push(format!("standard-memory-intrinsic at {pc:#x}: {error}"));
                return StructuralCallControl::Stop;
            }
        };
        let removed = state.blockers.pop();
        debug_assert!(removed.is_some_and(|blocker| blocker.starts_with("call/jump instruction")));
        state.blockers.push(format!(
            "{REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER} at {pc:#x}: {name}"
        ));
        if dest == Reg::ZERO {
            state.return_value = result;
            return StructuralCallControl::Stop;
        }
        structural_finish_call_with_result(&mut state.values, return_pc, result);
        return StructuralCallControl::Advance(instruction_count);
    }

    if let Some(&argument_count) = pointer_context.diagnostic_calls.get(name) {
        if dest != Reg::RA {
            state.reference_blockers.push(format!(
                "unsupported-diagnostic-call-link-register at {pc:#x}: {name} uses {dest}"
            ));
            return StructuralCallControl::Stop;
        }
        let arguments = (0..usize::from(argument_count))
            .map(|index| state.values[10 + index].clone())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        state
            .reference_events
            .push(DraftReferenceEvent::DiagnosticCall {
                site: pc as u32,
                function: name.clone(),
                argument_count,
                arguments,
            });
        structural_finish_call_with_result(&mut state.values, return_pc, SymbolicValue::Unknown);
        return StructuralCallControl::Advance(instruction_count);
    }

    if let Some(function) = pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.direct_external_semantic)(name))
        && !matches!(function.return_model, ExternalReturnModel::Unmodeled)
    {
        if dest != Reg::RA {
            state.reference_blockers.push(format!(
                "unsupported-modeled-direct-call-link-register at {pc:#x}: {name} uses {dest}"
            ));
            return StructuralCallControl::Stop;
        }
        let result = match function.return_model {
            ExternalReturnModel::Void => SymbolicValue::Unknown,
            ExternalReturnModel::Constant(value) => SymbolicValue::Constant(value),
            ExternalReturnModel::SymbolicU32 => {
                SymbolicValue::ExternalResult(state.next_external_call_token)
            }
            ExternalReturnModel::SymbolicU64 => {
                state.reference_blockers.push(format!(
                    "unsupported-modeled-direct-wide-return at {pc:#x}: {name}"
                ));
                return StructuralCallControl::Stop;
            }
            ExternalReturnModel::AllocatedZeroed { .. } => SymbolicValue::ExternalResult(
                state.next_external_call_token | ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG,
            ),
            ExternalReturnModel::OpaquePointer => SymbolicValue::ExternalResult(
                state.next_external_call_token | OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG,
            ),
            ExternalReturnModel::Unmodeled => unreachable!(),
        };
        let arguments = (0..usize::from(function.argument_count))
            .map(|index| state.values[10 + index].clone())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let removed = state.blockers.pop();
        debug_assert!(removed.is_some_and(|blocker| blocker.starts_with("call/jump instruction")));
        state
            .reference_events
            .push(DraftReferenceEvent::ModeledDirectCall {
                token: state.next_external_call_token,
                site: pc as u32,
                function: crate::ModeledDirectCall {
                    id: function.id.to_owned(),
                    name: function.c_name.to_owned(),
                    argument_count: function.argument_count,
                    return_model: function.return_model,
                    operation: function.semantic.operation.to_owned(),
                    return_type: function.semantic.return_type.to_owned(),
                    replacement_hint: function.semantic.replacement.map(str::to_owned),
                    evidence: function.evidence.to_owned(),
                },
                arguments,
            });
        state.next_external_call_token += 1;
        structural_finish_call_with_result(&mut state.values, return_pc, result);
        return StructuralCallControl::Advance(instruction_count);
    }

    let Some(target) = *target else {
        state
            .reference_blockers
            .push(format!("unresolved-call-relocation at {pc:#x}: {name}"));
        if dest == Reg::RA {
            structural_prepare_opaque_call(state, return_pc);
            return StructuralCallControl::Advance(instruction_count);
        }
        return StructuralCallControl::Stop;
    };
    let arguments = structural_call_arguments(
        &state.values,
        &state.stack,
        state.private_stack_may_be_modified_by_call,
    );
    state.private_stack_may_be_modified_by_call |= arguments
        .iter()
        .any(|argument| argument.private_stack_offset().is_some());
    if dest == Reg::ZERO {
        let call_token = state.next_call_token;
        state.reference_events.push(DraftReferenceEvent::TailCall {
            token: call_token,
            site: pc as u32,
            target,
            arguments,
        });
        state.return_value = SymbolicValue::CallResult(call_token);
        return StructuralCallControl::Stop;
    }
    if dest == Reg::RA {
        let call_token = state.next_call_token;
        state.next_call_token += 1;
        state.reference_events.push(DraftReferenceEvent::Call {
            token: call_token,
            site: pc as u32,
            target,
            arguments,
        });
        structural_finish_call(
            &mut state.values,
            return_pc,
            call_token,
            target,
            pointer_context,
        );
    } else {
        state.reference_blockers.push(format!(
            "unsupported-call-link-register at {pc:#x}: {name} uses {dest}"
        ));
    }
    StructuralCallControl::Advance(instruction_count)
}

pub(super) fn apply_call_instruction(
    decoded: artifact::DecodedInstruction,
    symbol: &artifact::ArtifactSymbolDefinition,
    pointer_context: &StructuralPointerContext,
    state: &mut StructuralTraceState,
) -> StructuralCallControl {
    let pc = decoded.address;
    let width = decoded.width;
    let instruction = decoded.instruction;
    match instruction {
        Inst::Jal { offset, dest } => {
            let target = (pc as u32).wrapping_add(offset.as_u32());
            let symbol_start = symbol.address as u32;
            let symbol_end = symbol_start.wrapping_add(symbol.bytes.len() as u32);
            if dest == Reg::ZERO && target >= symbol_start && target < symbol_end {
                return StructuralCallControl::NotCall;
            }
            state
                .blockers
                .push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            if target < symbol_start || target >= symbol_end {
                let arguments = structural_call_arguments(
                    &state.values,
                    &state.stack,
                    state.private_stack_may_be_modified_by_call,
                );
                state.private_stack_may_be_modified_by_call |= arguments
                    .iter()
                    .any(|argument| argument.private_stack_offset().is_some());
                if dest == Reg::ZERO {
                    let call_token = state.next_call_token;
                    state.reference_events.push(DraftReferenceEvent::TailCall {
                        token: call_token,
                        site: pc as u32,
                        target,
                        arguments,
                    });
                    state.return_value = SymbolicValue::CallResult(call_token);
                    return StructuralCallControl::Stop;
                }
                if dest == Reg::RA {
                    let call_token = state.next_call_token;
                    state.next_call_token += 1;
                    state.reference_events.push(DraftReferenceEvent::Call {
                        token: call_token,
                        site: pc as u32,
                        target,
                        arguments,
                    });
                    structural_finish_call(
                        &mut state.values,
                        (pc as u32).wrapping_add(u32::from(width)),
                        call_token,
                        target,
                        pointer_context,
                    );
                }
            }
            StructuralCallControl::Advance(1)
        }
        Inst::Jalr { offset, base, dest }
            if matches!(
                &state.values[usize::from(base.0)],
                SymbolicValue::FunctionPointer { .. }
            ) =>
        {
            let SymbolicValue::FunctionPointer { table, target } =
                state.values[usize::from(base.0)].clone()
            else {
                unreachable!()
            };
            state
                .blockers
                .push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                state.reference_blockers.push(format!(
                    "unsupported function-table call shape at {pc:#x}: {}::{target:#010x}",
                    table.id()
                ));
                return StructuralCallControl::Stop;
            }
            let arguments = structural_call_arguments(
                &state.values,
                &state.stack,
                state.private_stack_may_be_modified_by_call,
            );
            state.private_stack_may_be_modified_by_call |= arguments
                .iter()
                .any(|argument| argument.private_stack_offset().is_some());
            let call_token = state.next_call_token;
            if dest == Reg::ZERO {
                state.reference_events.push(DraftReferenceEvent::TailCall {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                state.return_value = SymbolicValue::CallResult(call_token);
                return StructuralCallControl::Stop;
            }
            state.next_call_token += 1;
            state.reference_events.push(DraftReferenceEvent::Call {
                token: call_token,
                site: pc as u32,
                target,
                arguments,
            });
            structural_finish_call(
                &mut state.values,
                (pc as u32).wrapping_add(u32::from(width)),
                call_token,
                target,
                pointer_context,
            );
            StructuralCallControl::Advance(1)
        }
        Inst::Jalr { offset, base, dest }
            if matches!(
                &state.values[usize::from(base.0)],
                SymbolicValue::ReviewedExternalFunction { .. }
            ) =>
        {
            let SymbolicValue::ReviewedExternalFunction {
                contract,
                offset: slot,
            } = state.values[usize::from(base.0)].clone()
            else {
                unreachable!()
            };
            let candidates = pointer_context
                .reviewed_external_slots
                .get(&(contract, slot))
                .expect("reviewed external slot pointer requires registered ABI candidates")
                .clone();
            apply_reviewed_external_call(
                pc as u32,
                width,
                instruction,
                offset.as_u32(),
                dest,
                candidates,
                state,
            )
        }
        Inst::Jalr { offset, dest, .. }
            if pointer_context
                .reviewed_external_calls
                .contains_key(&StructuralCallSite::new(symbol, pc as u32)) =>
        {
            let candidates = pointer_context
                .reviewed_external_calls
                .get(&StructuralCallSite::new(symbol, pc as u32))
                .expect("reviewed call site was matched")
                .clone();
            apply_reviewed_external_call(
                pc as u32,
                width,
                instruction,
                offset.as_u32(),
                dest,
                candidates,
                state,
            )
        }
        Inst::Jalr { offset, base, dest }
            if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 =>
        {
            state.return_value = state.values[usize::from(Reg::A0.0)].clone();
            StructuralCallControl::Stop
        }
        Inst::Jalr { offset, dest, .. } => {
            state
                .blockers
                .push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            if dest == Reg::RA {
                state.reference_blockers.push(format!(
                    "unresolved-indirect-call at {pc:#x}: {instruction}"
                ));
                structural_prepare_opaque_call(state, (pc as u32).wrapping_add(u32::from(width)));
                StructuralCallControl::Advance(1)
            } else {
                state.reference_blockers.push(format!(
                    "unresolved-indirect-control-flow at {pc:#x}: {instruction}; offset={:+#x}",
                    offset.as_i32(),
                ));
                StructuralCallControl::Stop
            }
        }
        _ => StructuralCallControl::NotCall,
    }
}
