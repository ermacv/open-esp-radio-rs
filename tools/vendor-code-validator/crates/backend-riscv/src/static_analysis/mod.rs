//! Fail-closed structural tracing of RV32 functions.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use crate::{
    BitSource, BranchCondition, BranchOperation, DEFERRED_CALLER_MEMORY_REGION,
    DirectSemanticFunctionSpec, DraftReferenceEvent, ExpressionOperation, ExternalReturnModel,
    ExternalTableRef, FunctionAnalysis, FunctionTableRef, IndexedMmioDomain, IndexedMmioRegister,
    MemoryAccess, MmioRegisterMap, ObservableEvent, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, Result, Rv32CallArguments, SECONDARY_CALL_RESULT_TOKEN_FLAG,
    SymbolicValue, artifact, collect_evaluable_input_bits, encode_fence_set, evaluate_for_input,
    indexed_mmio_domain,
};

mod alu;
mod context;
mod memory;
mod memory_access;
mod poll;
mod stack;
mod state;

use alu::apply_alu_instruction;
pub use context::{StructuralCallSite, StructuralPointerContext, StructuralRelocatedCalls};
use memory::*;
use memory_access::apply_memory_instruction;
use poll::*;
pub use stack::SymbolicStack;
use stack::structural_call_arguments;
use state::StructuralTraceState;

const REFERENCE_ONLY_POLL_BLOCKER: &str = "reference-modeled MMIO polling loop";
const REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER: &str = "reference-modeled standard memory intrinsic";
const MAX_INLINE_MEMORY_INTRINSIC_BYTES: u32 = 256;
// Constant-propagated counted loops are fully unrolled so every memory effect
// remains visible to reference generation. A reviewed calibration-record
// transfer has a proven 508-byte inner loop, so the former 256-visit ceiling
// rejected it even though both the pointer and terminal address were concrete.
// This remains a hard fail-closed bound: an unresolved or non-terminating loop
// still exhausts the budget instead of becoming a reference program.
const MAX_STRUCTURAL_INSTRUCTION_VISITS: u16 = 1_024;

#[derive(Debug)]
pub struct RiscvSummaryHooks {
    pub secondary_return_target: fn(u32) -> bool,
    pub direct_semantic:
        fn(&artifact::ArtifactSymbolDefinition) -> Option<&'static DirectSemanticFunctionSpec>,
    pub reference_intrinsic: fn(
        &artifact::ArtifactSymbolDefinition,
        &MmioRegisterMap,
        &StructuralPointerContext,
    ) -> Option<FunctionAnalysis>,
    pub standard_memory_intrinsic: fn(
        &artifact::ArtifactSymbolDefinition,
        &Rv32CallArguments,
    ) -> Option<std::result::Result<FunctionAnalysis, String>>,
    pub wide_signed_divide: fn(
        &artifact::ArtifactSymbolDefinition,
        &Rv32CallArguments,
    ) -> Option<(SymbolicValue, SymbolicValue)>,
}

pub struct RiscvHarnessSpec {
    pub contracts: &'static crate::HarnessContractSpec,
    pub summaries: &'static RiscvSummaryHooks,
}

pub fn is_reference_only_blocker(blocker: &str) -> bool {
    blocker.starts_with(REFERENCE_ONLY_POLL_BLOCKER)
        || blocker.starts_with(REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER)
}

fn structural_set(values: &mut [SymbolicValue; 32], register: Reg, value: SymbolicValue) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = value;
    }
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

pub fn trace_binary_symbol(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
) -> Result<FunctionAnalysis> {
    trace_binary_symbol_with_branches(
        symbol,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        &BTreeMap::new(),
    )
}

pub fn trace_binary_symbol_with_branches(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    forced_branches: &BTreeMap<u32, bool>,
) -> Result<FunctionAnalysis> {
    let mut state = StructuralTraceState::new(specialized_arguments);

    let instructions = artifact::decode_symbol(symbol)?;
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut instruction_index = 0usize;
    let mut instruction_visits = BTreeMap::<u32, u16>::new();
    // Reference-flow exploration forces one outcome per unresolved branch
    // site. A loop-invariant branch inside a concrete counted loop therefore
    // has one semantic decision even though the instruction executes many
    // times. Keep only its first event; otherwise flow construction would
    // incorrectly require both outcomes again inside the already selected
    // arm.
    let mut emitted_forced_branch_decisions = BTreeSet::<u32>::new();
    let mut checkpoints = BTreeMap::<u32, StructuralCheckpoint>::new();
    while let Some(decoded) = instructions.get(instruction_index).copied() {
        let pc = decoded.address;
        let width = decoded.width;
        let instruction = decoded.instruction;
        let visits = instruction_visits.entry(pc as u32).or_default();
        if *visits >= MAX_STRUCTURAL_INSTRUCTION_VISITS {
            state.blockers.push(format!(
                "control-flow loop bounded unrolling exceeds {MAX_STRUCTURAL_INSTRUCTION_VISITS} visits at {pc:#x}: {instruction}"
            ));
            break;
        }
        *visits += 1;
        checkpoints.insert(pc as u32, state.checkpoint());
        if let Some((name, target)) =
            relocated_calls.get(&StructuralCallSite::new(symbol, pc as u32))
        {
            state.blockers.push(format!(
                "call/jump instruction at {pc:#x}: relocated call to {name}"
            ));
            let Some(jalr) = instructions.get(instruction_index + 1).copied() else {
                state.reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} has no following JALR"
                ));
                break;
            };
            if jalr.address != pc.wrapping_add(4) {
                state.reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not a two-instruction call"
                ));
                break;
            }
            let Inst::Jalr { dest, .. } = jalr.instruction else {
                state.reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not followed by JALR"
                ));
                break;
            };
            if target.is_none()
                && let Some(result) = inline_standard_memory_intrinsic(
                    name,
                    &core::array::from_fn(|index| {
                        if index < RV32_REGISTER_ARGUMENT_COUNT {
                            state.values[10 + index].clone()
                        } else {
                            SymbolicValue::Unknown
                        }
                    }),
                    symbol,
                    &mut state.stack,
                    &mut state.reference_events,
                    &mut state.next_memory_read_token,
                )
            {
                if !matches!(dest, Reg::ZERO | Reg::RA) {
                    state.reference_blockers.push(format!(
                        "unsupported-memory-intrinsic-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        state
                            .reference_blockers
                            .push(format!("standard-memory-intrinsic at {pc:#x}: {error}"));
                        break;
                    }
                };
                let removed = state.blockers.pop();
                debug_assert!(
                    removed.is_some_and(|blocker| { blocker.starts_with("call/jump instruction") })
                );
                state.blockers.push(format!(
                    "{REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER} at {pc:#x}: {name}"
                ));
                if dest == Reg::ZERO {
                    state.return_value = result;
                    break;
                }
                structural_finish_call_with_result(
                    &mut state.values,
                    (pc as u32).wrapping_add(8),
                    result,
                );
                state.values[0] = SymbolicValue::Constant(0);
                instruction_index += 2;
                continue;
            }
            if target.is_none()
                && let Some(&argument_count) = pointer_context.diagnostic_calls.get(name)
            {
                if dest != Reg::RA {
                    state.reference_blockers.push(format!(
                        "unsupported-diagnostic-call-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let arguments = (0..usize::from(argument_count))
                    .map(|index| state.values[10 + index].clone())
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                state
                    .reference_events
                    .push(DraftReferenceEvent::DiagnosticCall {
                        function: name.clone(),
                        argument_count,
                        arguments,
                    });
                structural_finish_call_with_result(
                    &mut state.values,
                    (pc as u32).wrapping_add(8),
                    SymbolicValue::Unknown,
                );
                state.values[0] = SymbolicValue::Constant(0);
                instruction_index += 2;
                continue;
            }
            let Some(target) = *target else {
                state
                    .reference_blockers
                    .push(format!("unresolved-call-relocation at {pc:#x}: {name}"));
                break;
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
                break;
            } else if dest == Reg::RA {
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
                    (pc as u32).wrapping_add(8),
                    call_token,
                    target,
                    pointer_context,
                );
            } else {
                state.reference_blockers.push(format!(
                    "unsupported-call-link-register at {pc:#x}: {name} uses {dest}"
                ));
            }
            state.values[0] = SymbolicValue::Constant(0);
            instruction_index += 2;
            continue;
        }
        if apply_alu_instruction(
            decoded,
            symbol,
            &mut state.values,
            &mut state.reference_blockers,
        ) {
            state.values[0] = SymbolicValue::Constant(0);
            instruction_index += 1;
            continue;
        }
        if apply_memory_instruction(decoded, symbol, pointer_context, svd, &mut state) {
            state.values[0] = SymbolicValue::Constant(0);
            instruction_index += 1;
            continue;
        }
        match instruction {
            Inst::Beq { offset, src1, src2 }
            | Inst::Bne { offset, src1, src2 }
            | Inst::Blt { offset, src1, src2 }
            | Inst::Bge { offset, src1, src2 }
            | Inst::Bltu { offset, src1, src2 }
            | Inst::Bgeu { offset, src1, src2 } => {
                let left_value = state.values[usize::from(src1.0)].clone();
                let right_value = state.values[usize::from(src2.0)].clone();
                let left = left_value.as_constant();
                let right = right_value.as_constant();
                let taken = if let Some((left, right)) = left.zip(right) {
                    match instruction {
                        Inst::Beq { .. } => left == right,
                        Inst::Bne { .. } => left != right,
                        Inst::Blt { .. } => (left as i32) < (right as i32),
                        Inst::Bge { .. } => (left as i32) >= (right as i32),
                        Inst::Bltu { .. } => left < right,
                        Inst::Bgeu { .. } => left >= right,
                        _ => unreachable!(),
                    }
                } else {
                    let operation = match instruction {
                        Inst::Beq { .. } => BranchOperation::Equal,
                        Inst::Bne { .. } => BranchOperation::NotEqual,
                        Inst::Blt { .. } => BranchOperation::LessSigned,
                        Inst::Bge { .. } => BranchOperation::GreaterEqualSigned,
                        Inst::Bltu { .. } => BranchOperation::LessUnsigned,
                        Inst::Bgeu { .. } => BranchOperation::GreaterEqualUnsigned,
                        _ => unreachable!(),
                    };
                    let condition = BranchCondition {
                        site: pc as u32,
                        operation,
                        left: left_value,
                        right: right_value,
                    };
                    if !condition.left.is_resolved() || !condition.right.is_resolved() {
                        state.blockers.push(format!(
                            "unresolved input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        break;
                    }
                    let branch_target = (pc as u32).wrapping_add(offset.as_u32());
                    if branch_target < pc as u32
                        && let Some(loop_start_index) =
                            instruction_indices.get(&branch_target).copied()
                        && let Some(checkpoint) = checkpoints.get(&branch_target)
                        && let Some(poll) = recognize_structural_poll_loop(
                            &instructions,
                            loop_start_index,
                            instruction_index,
                            &condition,
                            checkpoint,
                            &state.events,
                            &state.reference_events,
                            &state.blockers,
                            &state.reference_blockers,
                            state.next_mmio_read_token,
                            state.next_memory_read_token,
                            state.next_call_token,
                            state.next_external_call_token,
                            &state.stack,
                            svd,
                        )
                    {
                        state.restore_checkpoint(poll.checkpoint);
                        for value in &mut state.values {
                            if symbolic_value_depends_on_mmio_read(value, poll.read_token) {
                                *value = SymbolicValue::Unknown;
                            }
                        }
                        state.reference_events.push(poll.event);
                        state.blockers.push(format!(
                            "{REFERENCE_ONLY_POLL_BLOCKER} at {pc:#x}: {instruction}"
                        ));
                        let fallthrough = (pc as u32).wrapping_add(u32::from(width));
                        let Some(fallthrough_index) =
                            instruction_indices.get(&fallthrough).copied()
                        else {
                            state.reference_blockers.push(format!(
                                "invalid polling-loop fallthrough at {pc:#x}: {instruction}"
                            ));
                            break;
                        };
                        instruction_index = fallthrough_index;
                        state.values[0] = SymbolicValue::Constant(0);
                        continue;
                    }
                    let Some(taken) = forced_branches.get(&(pc as u32)).copied() else {
                        state.blockers.push(format!(
                            "input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        state.unresolved_branch = Some(condition);
                        break;
                    };
                    if emitted_forced_branch_decisions.insert(pc as u32) {
                        state
                            .reference_events
                            .push(DraftReferenceEvent::BranchDecision { condition, taken });
                    }
                    taken
                };
                let target = if taken {
                    (pc as u32).wrapping_add(offset.as_u32())
                } else {
                    (pc as u32).wrapping_add(u32::from(width))
                };
                let Some(target_index) = instruction_indices.get(&target).copied() else {
                    state.blockers.push(format!(
                        "invalid conditional target at {pc:#x}: {instruction}"
                    ));
                    break;
                };
                instruction_index = target_index;
                state.values[0] = SymbolicValue::Constant(0);
                continue;
            }
            Inst::Jal { offset, dest } => {
                let target = (pc as u32).wrapping_add(offset.as_u32());
                let symbol_start = symbol.address as u32;
                let symbol_end = symbol_start.wrapping_add(symbol.bytes.len() as u32);
                if dest == Reg::ZERO && target >= symbol_start && target < symbol_end {
                    let Some(target_index) = instruction_indices.get(&target).copied() else {
                        state.blockers.push(format!(
                            "invalid local jump target at {pc:#x}: {instruction}"
                        ));
                        break;
                    };
                    instruction_index = target_index;
                    state.values[0] = SymbolicValue::Constant(0);
                    continue;
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
                        break;
                    } else if dest == Reg::RA {
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
                    break;
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
                    break;
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
            }
            Inst::Jalr { offset, base, dest }
                if matches!(
                    &state.values[usize::from(base.0)],
                    SymbolicValue::ExternalFunction { .. }
                ) =>
            {
                let SymbolicValue::ExternalFunction { table, function } =
                    state.values[usize::from(base.0)].clone()
                else {
                    unreachable!()
                };
                let slot = function.spec();
                if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                    state.blockers.push(format!(
                        "unsupported external ABI call shape at {pc:#x}: {instruction}"
                    ));
                    break;
                }
                let mut arguments = (0..usize::from(slot.argument_count))
                    .map(|index| state.values[10 + index].clone())
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                let mut private_stack_output = None;
                let result = match slot.return_model {
                    ExternalReturnModel::Constant(value) => SymbolicValue::Constant(value),
                    ExternalReturnModel::SymbolicU32 => {
                        SymbolicValue::ExternalResult(state.next_external_call_token)
                    }
                    ExternalReturnModel::PrivateStackOutputU8 { pointer_argument } => {
                        let Some(SymbolicValue::StackAddress(offset)) =
                            arguments.get(usize::from(pointer_argument))
                        else {
                            state.blockers.push(format!(
                                "call/jump instruction at {pc:#x}: external ABI {}::{}",
                                table.spec().id,
                                slot.c_name
                            ));
                            state.reference_blockers.push(format!(
                                "unsupported-external-output-pointer at {pc:#x}: {}::{} argument a{pointer_argument} is not private stack",
                                table.spec().id,
                                slot.c_name
                            ));
                            break;
                        };
                        let output =
                            SymbolicValue::ExternalResult(state.next_external_call_token).and(0xff);
                        state.stack.store(*offset, 8, &output);
                        private_stack_output = Some((*offset, output));
                        // The validated private pointer has already been
                        // consumed by the internal stack effect. Do not let a
                        // callee-local address escape into call composition or
                        // generated behavior.
                        arguments[usize::from(pointer_argument)] = SymbolicValue::Constant(0);
                        // The C callback returns an int, but this model only
                        // claims its output-byte effect. Any later use of a0
                        // therefore remains fail-closed.
                        SymbolicValue::Unknown
                    }
                    ExternalReturnModel::Unmodeled => {
                        state.reference_blockers.push(format!(
                            "unmodeled-external-semantics at {pc:#x}: {}::{} ({})",
                            table.spec().id,
                            slot.c_name,
                            slot.semantic.operation,
                        ));
                        // Preserve opaque return-value data flow for manual IR
                        // without claiming the call's effects are modeled.
                        SymbolicValue::ExternalResult(state.next_external_call_token)
                    }
                };
                state
                    .reference_events
                    .push(DraftReferenceEvent::ExternalCall {
                        token: state.next_external_call_token,
                        table,
                        function,
                        arguments,
                    });
                if let Some((offset, value)) = private_stack_output {
                    state
                        .reference_events
                        .push(DraftReferenceEvent::PrivateStackStore {
                            offset,
                            width: 8,
                            value,
                        });
                }
                state.next_external_call_token += 1;
                if dest == Reg::ZERO {
                    state.return_value = result;
                    break;
                }
                structural_finish_call_with_result(
                    &mut state.values,
                    (pc as u32).wrapping_add(u32::from(width)),
                    result,
                );
            }
            Inst::Jalr { offset, base, dest }
                if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 =>
            {
                state.return_value = state.values[usize::from(Reg::A0.0)].clone();
                break;
            }
            Inst::Jalr { .. } => {
                state
                    .blockers
                    .push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            }
            Inst::Fence { fence } => {
                let event = ObservableEvent::Fence {
                    fm: fence.fm,
                    predecessor: encode_fence_set(fence.pred),
                    successor: encode_fence_set(fence.succ),
                };
                state.events.push(event.clone());
                state
                    .reference_events
                    .push(DraftReferenceEvent::Observable(event));
            }
            Inst::Ecall
            | Inst::Ebreak
            | Inst::LrW { .. }
            | Inst::ScW { .. }
            | Inst::AmoW { .. } => {
                state.blockers.push(format!(
                    "unsupported execution edge at {pc:#x}: {instruction}"
                ));
            }
            _ => {
                state
                    .blockers
                    .push(format!("unsupported instruction at {pc:#x}: {instruction}"));
            }
        }
        state.values[0] = SymbolicValue::Constant(0);
        instruction_index += 1;
    }

    Ok(state.finish(symbol))
}
