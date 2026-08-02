//! Fail-closed structural tracing of RV32 functions.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use crate::{
    BitSource, BranchCondition, BranchOperation, DEFERRED_CALLER_MEMORY_REGION,
    DraftReferenceEvent, ExpressionOperation, ExternalReturnModel, ExternalTableRef,
    FunctionAnalysis, FunctionTableRef, IndexedMmioDomain, IndexedMmioRegister, MemoryAccess,
    MmioRegisterMap, ObservableEvent, RV32_REGISTER_ARGUMENT_COUNT, RV32_STACK_ARGUMENT_COUNT,
    Result, Rv32CallArguments, SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue, artifact,
    collect_evaluable_input_bits, encode_fence_set, evaluate_for_input, indexed_mmio_domain,
};

mod context;
mod memory;
mod poll;
mod stack;

pub use context::{StructuralCallSite, StructuralPointerContext, StructuralRelocatedCalls};
use memory::*;
use poll::*;
pub use stack::SymbolicStack;
use stack::structural_call_arguments;

const REFERENCE_ONLY_POLL_BLOCKER: &str = "reference-modeled MMIO polling loop";
const REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER: &str = "reference-modeled standard memory intrinsic";
const MAX_INLINE_MEMORY_INTRINSIC_BYTES: u32 = 256;
const MAX_STRUCTURAL_INSTRUCTION_VISITS: u16 = 256;

#[derive(Debug)]
pub struct RiscvSummaryHooks {
    pub secondary_return_target: fn(u32) -> bool,
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
    let mut values: [SymbolicValue; 32] = core::array::from_fn(|_| SymbolicValue::Unknown);
    values[0] = SymbolicValue::Constant(0);
    values[usize::from(Reg::SP.0)] = SymbolicValue::StackAddress(0);
    for index in 0..RV32_REGISTER_ARGUMENT_COUNT {
        values[10 + index] = specialized_arguments
            .and_then(|arguments| arguments[index].as_constant())
            .map_or_else(
                || SymbolicValue::input(index as u8),
                |value| SymbolicValue::InputConstant {
                    index: index as u8,
                    value,
                },
            );
    }
    let mut events = Vec::new();
    let mut reference_events = Vec::new();
    let mut blockers = Vec::new();
    let mut reference_blockers = Vec::new();
    let mut return_value = SymbolicValue::Unknown;
    let mut unresolved_branch = None;
    let mut next_mmio_read_token = 0_u32;
    let mut next_memory_read_token = 0_u32;
    let mut next_call_token = 0_u32;
    let mut next_external_call_token = 0_u32;
    let mut next_private_stack_read_token = 0_u32;
    let mut stack = SymbolicStack::default();
    for index in 0..RV32_STACK_ARGUMENT_COUNT {
        let argument_index = RV32_REGISTER_ARGUMENT_COUNT + index;
        let value = specialized_arguments
            .and_then(|arguments| arguments[argument_index].as_constant())
            .map_or_else(
                || SymbolicValue::input(argument_index as u8),
                SymbolicValue::Constant,
            );
        stack.store((index * 4) as i32, 32, &value);
    }
    let mut private_stack_may_be_modified_by_call = false;

    let instructions = artifact::decode_symbol(symbol)?;
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut instruction_index = 0usize;
    let mut instruction_visits = BTreeMap::<u32, u16>::new();
    let mut checkpoints = BTreeMap::<u32, StructuralCheckpoint>::new();
    while let Some(decoded) = instructions.get(instruction_index).copied() {
        let pc = decoded.address;
        let width = decoded.width;
        let instruction = decoded.instruction;
        let visits = instruction_visits.entry(pc as u32).or_default();
        if *visits >= MAX_STRUCTURAL_INSTRUCTION_VISITS {
            blockers.push(format!(
                "control-flow loop bounded unrolling exceeds {MAX_STRUCTURAL_INSTRUCTION_VISITS} visits at {pc:#x}: {instruction}"
            ));
            break;
        }
        *visits += 1;
        checkpoints.insert(
            pc as u32,
            StructuralCheckpoint {
                events_len: events.len(),
                reference_events_len: reference_events.len(),
                blockers_len: blockers.len(),
                reference_blockers_len: reference_blockers.len(),
                next_mmio_read_token,
                next_memory_read_token,
                next_call_token,
                next_external_call_token,
                stack: stack.clone(),
            },
        );
        if let Some((name, target)) =
            relocated_calls.get(&StructuralCallSite::new(symbol, pc as u32))
        {
            blockers.push(format!(
                "call/jump instruction at {pc:#x}: relocated call to {name}"
            ));
            let Some(jalr) = instructions.get(instruction_index + 1).copied() else {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} has no following JALR"
                ));
                break;
            };
            if jalr.address != pc.wrapping_add(4) {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not a two-instruction call"
                ));
                break;
            }
            let Inst::Jalr { dest, .. } = jalr.instruction else {
                reference_blockers.push(format!(
                    "malformed-call-relocation at {pc:#x}: {name} is not followed by JALR"
                ));
                break;
            };
            if target.is_none()
                && let Some(result) = inline_standard_memory_intrinsic(
                    name,
                    &core::array::from_fn(|index| {
                        if index < RV32_REGISTER_ARGUMENT_COUNT {
                            values[10 + index].clone()
                        } else {
                            SymbolicValue::Unknown
                        }
                    }),
                    symbol,
                    &mut stack,
                    &mut reference_events,
                    &mut next_memory_read_token,
                )
            {
                if !matches!(dest, Reg::ZERO | Reg::RA) {
                    reference_blockers.push(format!(
                        "unsupported-memory-intrinsic-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let result = match result {
                    Ok(result) => result,
                    Err(error) => {
                        reference_blockers
                            .push(format!("standard-memory-intrinsic at {pc:#x}: {error}"));
                        break;
                    }
                };
                let removed = blockers.pop();
                debug_assert!(
                    removed.is_some_and(|blocker| { blocker.starts_with("call/jump instruction") })
                );
                blockers.push(format!(
                    "{REFERENCE_ONLY_MEMORY_INTRINSIC_BLOCKER} at {pc:#x}: {name}"
                ));
                if dest == Reg::ZERO {
                    return_value = result;
                    break;
                }
                structural_finish_call_with_result(
                    &mut values,
                    (pc as u32).wrapping_add(8),
                    result,
                );
                values[0] = SymbolicValue::Constant(0);
                instruction_index += 2;
                continue;
            }
            if target.is_none()
                && let Some(&argument_count) = pointer_context.diagnostic_calls.get(name)
            {
                if dest != Reg::RA {
                    reference_blockers.push(format!(
                        "unsupported-diagnostic-call-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let arguments = (0..usize::from(argument_count))
                    .map(|index| values[10 + index].clone())
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                reference_events.push(DraftReferenceEvent::DiagnosticCall {
                    function: name.clone(),
                    argument_count,
                    arguments,
                });
                structural_finish_call_with_result(
                    &mut values,
                    (pc as u32).wrapping_add(8),
                    SymbolicValue::Unknown,
                );
                values[0] = SymbolicValue::Constant(0);
                instruction_index += 2;
                continue;
            }
            let Some(target) = *target else {
                reference_blockers.push(format!("unresolved-call-relocation at {pc:#x}: {name}"));
                break;
            };
            let arguments =
                structural_call_arguments(&values, &stack, private_stack_may_be_modified_by_call);
            private_stack_may_be_modified_by_call |= arguments
                .iter()
                .any(|argument| argument.private_stack_offset().is_some());
            if dest == Reg::ZERO {
                let call_token = next_call_token;
                reference_events.push(DraftReferenceEvent::TailCall {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                return_value = SymbolicValue::CallResult(call_token);
                break;
            } else if dest == Reg::RA {
                let call_token = next_call_token;
                next_call_token += 1;
                reference_events.push(DraftReferenceEvent::Call {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                structural_finish_call(
                    &mut values,
                    (pc as u32).wrapping_add(8),
                    call_token,
                    target,
                    pointer_context,
                );
            } else {
                reference_blockers.push(format!(
                    "unsupported-call-link-register at {pc:#x}: {name} uses {dest}"
                ));
            }
            values[0] = SymbolicValue::Constant(0);
            instruction_index += 2;
            continue;
        }
        match instruction {
            Inst::Lui { uimm, dest } => {
                if !symbol.addresses_resolved
                    && let Some(relocation) =
                        symbol.relocation(pc as u32, artifact::RelocationKind::Hi20)
                {
                    if uimm.as_u32() != 0 {
                        reference_blockers.push(format!(
                            "malformed-data-relocation at {pc:#x}: HI20 retains encoded immediate {:#x}",
                            uimm.as_u32()
                        ));
                        structural_set(&mut values, dest, SymbolicValue::Unknown);
                    } else {
                        structural_set(
                            &mut values,
                            dest,
                            relocation_symbol_address(symbol, relocation),
                        );
                    }
                } else {
                    structural_set(&mut values, dest, SymbolicValue::Constant(uimm.as_u32()));
                }
            }
            Inst::Auipc { uimm, dest } => {
                structural_set(
                    &mut values,
                    dest,
                    SymbolicValue::Constant((pc as u32).wrapping_add(uimm.as_u32())),
                );
            }
            Inst::Addi { imm, dest, src1 } => {
                let value = match complete_low_relocation(
                    symbol,
                    pc as u32,
                    artifact::RelocationKind::Lo12I,
                    &values[usize::from(src1.0)],
                    imm.as_i32(),
                ) {
                    Ok(Some(address)) => address,
                    Ok(None) => values[usize::from(src1.0)]
                        .clone()
                        .add_constant(imm.as_u32()),
                    Err(error) => {
                        reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        SymbolicValue::Unknown
                    }
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Andi { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .and(artifact::andi_immediate(imm, width));
                structural_set(&mut values, dest, value);
            }
            Inst::Ori { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().or(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Xori { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().xor(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Slli { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().shift_left(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Srli { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .shift_right(imm.as_u32());
                structural_set(&mut values, dest, value);
            }
            Inst::Srai { imm, dest, src1 } => {
                let source = values[usize::from(src1.0)].clone();
                let value = source.as_constant().map_or_else(
                    || {
                        SymbolicValue::expression(
                            ExpressionOperation::ShiftRightArithmetic,
                            source,
                            SymbolicValue::Constant(imm.as_u32()),
                        )
                    },
                    |value| {
                        SymbolicValue::Constant((value as i32).wrapping_shr(imm.as_u32()) as u32)
                    },
                );
                structural_set(&mut values, dest, value);
            }
            Inst::Sltiu { imm, dest, src1 } if imm.as_u32() == 1 => {
                let value = values[usize::from(src1.0)].clone().seqz();
                structural_set(&mut values, dest, value);
            }
            Inst::Slti { imm, dest, src1 } | Inst::Sltiu { imm, dest, src1 } => {
                let left = values[usize::from(src1.0)].clone();
                let right = SymbolicValue::Constant(imm.as_u32());
                let operation = if matches!(instruction, Inst::Slti { .. }) {
                    ExpressionOperation::LessThanSigned
                } else {
                    ExpressionOperation::LessThanUnsigned
                };
                let value = match left.as_constant() {
                    Some(left) if operation == ExpressionOperation::LessThanSigned => {
                        SymbolicValue::Constant(u32::from((left as i32) < (imm.as_u32() as i32)))
                    }
                    Some(left) => SymbolicValue::Constant(u32::from(left < imm.as_u32())),
                    None => SymbolicValue::expression(operation, left, right),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Slt { dest, src1, src2 } | Inst::Sltu { dest, src1, src2 } => {
                let left = values[usize::from(src1.0)].clone();
                let right = values[usize::from(src2.0)].clone();
                let operation = if matches!(instruction, Inst::Slt { .. }) {
                    ExpressionOperation::LessThanSigned
                } else {
                    ExpressionOperation::LessThanUnsigned
                };
                let value = match (left.as_constant(), right.as_constant()) {
                    (Some(left), Some(right))
                        if operation == ExpressionOperation::LessThanSigned =>
                    {
                        SymbolicValue::Constant(u32::from((left as i32) < (right as i32)))
                    }
                    (Some(left), Some(right)) => SymbolicValue::Constant(u32::from(left < right)),
                    _ => SymbolicValue::expression(operation, left, right),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::And { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .symbolic_bitand(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Or { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .symbolic_bitor(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Xor { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .symbolic_bitxor(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Add { dest, src1, src2 } => {
                let left = values[usize::from(src1.0)].clone();
                let right = values[usize::from(src2.0)].clone();
                let value = match (left.as_constant(), right.as_constant()) {
                    (Some(left), Some(right)) => SymbolicValue::Constant(left.wrapping_add(right)),
                    (_, Some(right)) => left.add_constant(right),
                    (Some(left), _) => right.add_constant(left),
                    _ => SymbolicValue::expression(ExpressionOperation::Add, left, right),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sub { dest, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
                let value = match (left_value.as_constant(), right_value.as_constant()) {
                    (Some(left), Some(right)) => SymbolicValue::Constant(left.wrapping_sub(right)),
                    _ => SymbolicValue::expression(
                        ExpressionOperation::Subtract,
                        left_value,
                        right_value,
                    ),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sll { dest, src1, src2 }
            | Inst::Srl { dest, src1, src2 }
            | Inst::Sra { dest, src1, src2 } => {
                let source = values[usize::from(src1.0)].clone();
                let amount = values[usize::from(src2.0)].clone();
                let constant_amount = amount.as_constant().map(|value| value & 31);
                let value = match (instruction, constant_amount) {
                    (Inst::Sll { .. }, Some(amount)) => source.shift_left(amount),
                    (Inst::Srl { .. }, Some(amount)) => source.shift_right(amount),
                    (Inst::Sra { .. }, Some(amount)) => source.as_constant().map_or_else(
                        || {
                            SymbolicValue::expression(
                                ExpressionOperation::ShiftRightArithmetic,
                                source,
                                SymbolicValue::Constant(amount),
                            )
                        },
                        |value| SymbolicValue::Constant((value as i32).wrapping_shr(amount) as u32),
                    ),
                    (Inst::Sll { .. }, None) => {
                        SymbolicValue::expression(ExpressionOperation::ShiftLeft, source, amount)
                    }
                    (Inst::Srl { .. }, None) => {
                        SymbolicValue::expression(ExpressionOperation::ShiftRight, source, amount)
                    }
                    (Inst::Sra { .. }, None) => SymbolicValue::expression(
                        ExpressionOperation::ShiftRightArithmetic,
                        source,
                        amount,
                    ),
                    _ => unreachable!(),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Mul { dest, src1, src2 }
            | Inst::Div { dest, src1, src2 }
            | Inst::Divu { dest, src1, src2 }
            | Inst::Rem { dest, src1, src2 }
            | Inst::Remu { dest, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
                let left = left_value.as_constant();
                let right = right_value.as_constant();
                let value = match (instruction, left, right) {
                    (Inst::Mul { .. }, Some(left), Some(right)) => {
                        SymbolicValue::Constant(left.wrapping_mul(right))
                    }
                    (Inst::Div { .. }, Some(left), Some(right)) => {
                        SymbolicValue::Constant(if right == 0 {
                            u32::MAX
                        } else if left == i32::MIN as u32 && right == u32::MAX {
                            i32::MIN as u32
                        } else {
                            ((left as i32) / (right as i32)) as u32
                        })
                    }
                    (Inst::Divu { .. }, Some(left), Some(right)) => {
                        SymbolicValue::Constant(left.checked_div(right).unwrap_or(u32::MAX))
                    }
                    (Inst::Rem { .. }, Some(left), Some(right)) => {
                        SymbolicValue::Constant(if right == 0 {
                            left
                        } else if left == i32::MIN as u32 && right == u32::MAX {
                            0
                        } else {
                            ((left as i32) % (right as i32)) as u32
                        })
                    }
                    (Inst::Remu { .. }, Some(left), Some(right)) => {
                        SymbolicValue::Constant(if right == 0 { left } else { left % right })
                    }
                    (Inst::Mul { .. }, _, _) => SymbolicValue::expression(
                        ExpressionOperation::Multiply,
                        left_value,
                        right_value,
                    ),
                    (Inst::Div { .. }, _, _) => SymbolicValue::expression(
                        ExpressionOperation::DivideSigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Divu { .. }, _, _) => SymbolicValue::expression(
                        ExpressionOperation::DivideUnsigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Rem { .. }, _, _) => SymbolicValue::expression(
                        ExpressionOperation::RemainderSigned,
                        left_value,
                        right_value,
                    ),
                    (Inst::Remu { .. }, _, _) => SymbolicValue::expression(
                        ExpressionOperation::RemainderUnsigned,
                        left_value,
                        right_value,
                    ),
                    _ => unreachable!(),
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Mulh { dest, .. } | Inst::Mulhsu { dest, .. } | Inst::Mulhu { dest, .. } => {
                structural_set(&mut values, dest, SymbolicValue::Unknown);
            }
            Inst::Lb { offset, dest, base }
            | Inst::Lbu { offset, dest, base }
            | Inst::Lh { offset, dest, base }
            | Inst::Lhu { offset, dest, base }
            | Inst::Lw { offset, dest, base } => {
                let width = match instruction {
                    Inst::Lb { .. } | Inst::Lbu { .. } => 8,
                    Inst::Lh { .. } | Inst::Lhu { .. } => 16,
                    _ => 32,
                };
                let signed = matches!(instruction, Inst::Lb { .. } | Inst::Lh { .. });
                let relocated_pointer = symbol
                    .relocation(pc as u32, artifact::RelocationKind::Lo12I)
                    .and_then(|relocation| {
                        (relocation.addend == 0 && offset.as_i32() == 0)
                            .then(|| {
                                pointer_context
                                    .relocated_pointer_symbols
                                    .get(&relocation.symbol)
                                    .cloned()
                            })
                            .flatten()
                    });
                let address = if relocated_pointer.is_some() {
                    None
                } else {
                    match complete_low_relocation(
                        symbol,
                        pc as u32,
                        artifact::RelocationKind::Lo12I,
                        &values[usize::from(base.0)],
                        offset.as_i32(),
                    ) {
                        Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                        Ok(None) => structural_effective_address(&values, base, offset.as_i32()),
                        Err(error) => {
                            reference_blockers
                                .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                            structural_set(&mut values, dest, SymbolicValue::Unknown);
                            values[0] = SymbolicValue::Constant(0);
                            instruction_index += 1;
                            continue;
                        }
                    }
                };
                let value = match (relocated_pointer, address) {
                    (Some(value), _) if width == 32 => value,
                    (_, Some(StructuralAddress::Absolute(address)))
                        if width == 32
                            && pointer_context
                                .external_pointer_cells
                                .contains_key(&address) =>
                    {
                        SymbolicValue::ExternalTable(
                            pointer_context.external_pointer_cells[&address],
                        )
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if width == 32
                            && pointer_context
                                .function_pointer_cells
                                .contains_key(&address) =>
                    {
                        SymbolicValue::FunctionTable(
                            pointer_context.function_pointer_cells[&address],
                        )
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if width == 32
                            && pointer_context.data_pointer_cells.contains_key(&address) =>
                    {
                        pointer_context.data_pointer_cells[&address].clone()
                    }
                    (_, Some(StructuralAddress::ExternalTableSlot(table, offset)))
                        if width == 32 =>
                    {
                        let Ok(offset) = u32::try_from(offset) else {
                            reference_blockers.push(format!(
                                "negative-external-abi-slot at {pc:#x}: {instruction}"
                            ));
                            structural_set(&mut values, dest, SymbolicValue::Unknown);
                            values[0] = SymbolicValue::Constant(0);
                            instruction_index += 1;
                            continue;
                        };
                        match table.function_at(offset) {
                            Some(function) => SymbolicValue::ExternalFunction { table, function },
                            None => {
                                reference_blockers.push(format!(
                                    "unregistered-external-abi-slot at {pc:#x}: {}+{offset:#x}",
                                    table.spec().id
                                ));
                                SymbolicValue::Unknown
                            }
                        }
                    }
                    (_, Some(StructuralAddress::FunctionTableSlot(table, offset)))
                        if width == 32 =>
                    {
                        let Ok(offset) = u32::try_from(offset) else {
                            reference_blockers.push(format!(
                                "negative-function-table-slot at {pc:#x}: {instruction}"
                            ));
                            structural_set(&mut values, dest, SymbolicValue::Unknown);
                            values[0] = SymbolicValue::Constant(0);
                            instruction_index += 1;
                            continue;
                        };
                        match pointer_context.function_table_slots.get(&(table, offset)) {
                            Some(target) => SymbolicValue::FunctionPointer {
                                table,
                                target: *target,
                            },
                            None => {
                                reference_blockers.push(format!(
                                    "unregistered-function-table-slot at {pc:#x}: {}+{offset:#x}",
                                    table.id()
                                ));
                                SymbolicValue::Unknown
                            }
                        }
                    }
                    (_, Some(StructuralAddress::PrivateStack(offset))) => {
                        if private_stack_may_be_modified_by_call {
                            let token = next_private_stack_read_token;
                            next_private_stack_read_token += 1;
                            reference_events.push(DraftReferenceEvent::PrivateStackLoad {
                                token,
                                offset,
                                width,
                                signed,
                            });
                            SymbolicValue::private_stack_read(token, width, signed)
                        } else {
                            stack.load(offset, width, signed).unwrap_or_else(|| {
                                reference_blockers.push(format!(
                                    "uninitialized-private-stack-load at {pc:#x}: {instruction}"
                                ));
                                SymbolicValue::Unknown
                            })
                        }
                    }
                    (_, Some(StructuralAddress::CallerMemory(address))) => {
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: None,
                        });
                        SymbolicValue::memory_read(read_token, width, signed)
                    }
                    (_, Some(StructuralAddress::SymbolMemory(address))) => {
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            region: address.canonical(),
                            address,
                            value: None,
                        });
                        SymbolicValue::memory_read(read_token, width, signed)
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if svd.contains_mmio(address) =>
                    {
                        let read_token = next_mmio_read_token;
                        next_mmio_read_token += 1;
                        let event = ObservableEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address,
                            register: svd.register_name(address),
                            value: None,
                        };
                        events.push(event.clone());
                        reference_events.push(DraftReferenceEvent::Observable(event));
                        SymbolicValue::register_read(read_token, address, width, signed)
                    }
                    (_, Some(StructuralAddress::Absolute(address)))
                        if symbol.memory_region(address, width).is_some() =>
                    {
                        let region = symbol.memory_region(address, width).unwrap();
                        let read_token = next_memory_read_token;
                        next_memory_read_token += 1;
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Read,
                            width,
                            address: SymbolicValue::Constant(address),
                            region: region.name.clone(),
                            value: None,
                        });
                        SymbolicValue::memory_read(read_token, width, signed)
                    }
                    _ => {
                        if let Some((address, domain)) =
                            structural_indexed_mmio_address(&values, base, offset.as_i32(), svd)
                        {
                            let read_token = next_mmio_read_token;
                            next_mmio_read_token += 1;
                            reference_events.push(DraftReferenceEvent::IndexedMmio {
                                access: MemoryAccess::Read,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: None,
                            });
                            SymbolicValue::indexed_register_read(read_token, width, signed)
                        } else if let Some((address, region)) =
                            structural_indexed_read_only_memory_address(
                                &values,
                                base,
                                offset.as_i32(),
                                width,
                                symbol,
                                &reference_events,
                            )
                        {
                            let read_token = next_memory_read_token;
                            next_memory_read_token += 1;
                            reference_events.push(DraftReferenceEvent::Memory {
                                access: MemoryAccess::Read,
                                width,
                                address,
                                region,
                                value: None,
                            });
                            SymbolicValue::memory_read(read_token, width, signed)
                        } else if values[usize::from(base.0)].is_resolved()
                            && values[usize::from(base.0)].depends_on_private_stack_read()
                        {
                            let read_token = next_memory_read_token;
                            next_memory_read_token += 1;
                            reference_events.push(DraftReferenceEvent::Memory {
                                access: MemoryAccess::Read,
                                width,
                                address: values[usize::from(base.0)]
                                    .clone()
                                    .add_constant(offset.as_u32()),
                                region: DEFERRED_CALLER_MEMORY_REGION.to_owned(),
                                value: None,
                            });
                            SymbolicValue::memory_read(read_token, width, signed)
                        } else {
                            reference_blockers.push(format!(
                                "unmodeled-memory-load at {pc:#x}: {instruction}{}; base {} = {}",
                                if symbol.addresses_resolved {
                                    ""
                                } else {
                                    " (relocatable addresses)"
                                },
                                base,
                                values[usize::from(base.0)].canonical(),
                            ));
                            SymbolicValue::Unknown
                        }
                    }
                };
                structural_set(&mut values, dest, value);
            }
            Inst::Sb { offset, src, base }
            | Inst::Sh { offset, src, base }
            | Inst::Sw { offset, src, base } => {
                let width = match instruction {
                    Inst::Sb { .. } => 8,
                    Inst::Sh { .. } => 16,
                    _ => 32,
                };
                let value = values[usize::from(src.0)].clone();
                let address = match complete_low_relocation(
                    symbol,
                    pc as u32,
                    artifact::RelocationKind::Lo12S,
                    &values[usize::from(base.0)],
                    offset.as_i32(),
                ) {
                    Ok(Some(address)) => Some(StructuralAddress::SymbolMemory(address)),
                    Ok(None) => structural_effective_address(&values, base, offset.as_i32()),
                    Err(error) => {
                        reference_blockers
                            .push(format!("malformed-data-relocation at {pc:#x}: {error}"));
                        values[0] = SymbolicValue::Constant(0);
                        instruction_index += 1;
                        continue;
                    }
                };
                match address {
                    Some(StructuralAddress::PrivateStack(offset)) => {
                        stack.store(offset, width, &value);
                        reference_events.push(DraftReferenceEvent::PrivateStackStore {
                            offset,
                            width,
                            value,
                        });
                    }
                    Some(StructuralAddress::CallerMemory(address)) => {
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            region: "caller-owned ABI argument RAM".to_owned(),
                            value: Some(value),
                        });
                    }
                    Some(StructuralAddress::SymbolMemory(address)) => {
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            region: address.canonical(),
                            address,
                            value: Some(value),
                        });
                    }
                    Some(StructuralAddress::Absolute(address)) if svd.contains_mmio(address) => {
                        if !value.is_resolved() {
                            blockers.push(format!(
                                "unresolved MMIO write value at {pc:#x}: {instruction}"
                            ));
                        }
                        let event = ObservableEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address,
                            register: svd.register_name(address),
                            value: Some(value),
                        };
                        events.push(event.clone());
                        reference_events.push(DraftReferenceEvent::Observable(event));
                    }
                    Some(StructuralAddress::Absolute(address))
                        if symbol.memory_region(address, width).is_some() =>
                    {
                        let region = symbol.memory_region(address, width).unwrap();
                        if !region.writable {
                            reference_blockers.push(format!(
                                "read-only-memory-store at {pc:#x}: {instruction} ({})",
                                region.name
                            ));
                        }
                        if !value.is_resolved() {
                            reference_blockers
                                .push(format!("unresolved-memory-write at {pc:#x}: {instruction}"));
                        }
                        reference_events.push(DraftReferenceEvent::Memory {
                            access: MemoryAccess::Write,
                            width,
                            address: SymbolicValue::Constant(address),
                            region: region.name.clone(),
                            value: Some(value),
                        });
                    }
                    _ => {
                        if let Some((address, domain)) =
                            structural_indexed_mmio_address(&values, base, offset.as_i32(), svd)
                        {
                            if !value.is_resolved() {
                                reference_blockers.push(format!(
                                    "unresolved indexed MMIO write value at {pc:#x}: {instruction}"
                                ));
                            }
                            reference_events.push(DraftReferenceEvent::IndexedMmio {
                                access: MemoryAccess::Write,
                                width,
                                address,
                                registers: domain.registers,
                                guard: domain.guard,
                                value: Some(value),
                            });
                        } else if values[usize::from(base.0)].is_resolved()
                            && values[usize::from(base.0)].depends_on_private_stack_read()
                        {
                            reference_events.push(DraftReferenceEvent::Memory {
                                access: MemoryAccess::Write,
                                width,
                                address: values[usize::from(base.0)]
                                    .clone()
                                    .add_constant(offset.as_u32()),
                                region: DEFERRED_CALLER_MEMORY_REGION.to_owned(),
                                value: Some(value),
                            });
                        } else {
                            reference_blockers.push(format!(
                                "unmodeled-memory-store at {pc:#x}: {instruction}{}; base {} = {}",
                                if symbol.addresses_resolved {
                                    ""
                                } else {
                                    " (relocatable addresses)"
                                },
                                base,
                                values[usize::from(base.0)].canonical(),
                            ));
                        }
                    }
                }
            }
            Inst::Beq { offset, src1, src2 }
            | Inst::Bne { offset, src1, src2 }
            | Inst::Blt { offset, src1, src2 }
            | Inst::Bge { offset, src1, src2 }
            | Inst::Bltu { offset, src1, src2 }
            | Inst::Bgeu { offset, src1, src2 } => {
                let left_value = values[usize::from(src1.0)].clone();
                let right_value = values[usize::from(src2.0)].clone();
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
                        blockers.push(format!(
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
                            &events,
                            &reference_events,
                            &blockers,
                            &reference_blockers,
                            next_mmio_read_token,
                            next_memory_read_token,
                            next_call_token,
                            next_external_call_token,
                            &stack,
                            svd,
                        )
                    {
                        events.truncate(poll.checkpoint.events_len);
                        reference_events.truncate(poll.checkpoint.reference_events_len);
                        blockers.truncate(poll.checkpoint.blockers_len);
                        reference_blockers.truncate(poll.checkpoint.reference_blockers_len);
                        next_mmio_read_token = poll.checkpoint.next_mmio_read_token;
                        next_memory_read_token = poll.checkpoint.next_memory_read_token;
                        next_call_token = poll.checkpoint.next_call_token;
                        next_external_call_token = poll.checkpoint.next_external_call_token;
                        stack = poll.checkpoint.stack;
                        for value in &mut values {
                            if symbolic_value_depends_on_mmio_read(value, poll.read_token) {
                                *value = SymbolicValue::Unknown;
                            }
                        }
                        reference_events.push(poll.event);
                        blockers.push(format!(
                            "{REFERENCE_ONLY_POLL_BLOCKER} at {pc:#x}: {instruction}"
                        ));
                        let fallthrough = (pc as u32).wrapping_add(u32::from(width));
                        let Some(fallthrough_index) =
                            instruction_indices.get(&fallthrough).copied()
                        else {
                            reference_blockers.push(format!(
                                "invalid polling-loop fallthrough at {pc:#x}: {instruction}"
                            ));
                            break;
                        };
                        instruction_index = fallthrough_index;
                        values[0] = SymbolicValue::Constant(0);
                        continue;
                    }
                    let Some(taken) = forced_branches.get(&(pc as u32)).copied() else {
                        blockers.push(format!(
                            "input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        unresolved_branch = Some(condition);
                        break;
                    };
                    reference_events.push(DraftReferenceEvent::BranchDecision { condition, taken });
                    taken
                };
                let target = if taken {
                    (pc as u32).wrapping_add(offset.as_u32())
                } else {
                    (pc as u32).wrapping_add(u32::from(width))
                };
                let Some(target_index) = instruction_indices.get(&target).copied() else {
                    blockers.push(format!(
                        "invalid conditional target at {pc:#x}: {instruction}"
                    ));
                    break;
                };
                instruction_index = target_index;
                values[0] = SymbolicValue::Constant(0);
                continue;
            }
            Inst::Jal { offset, dest } => {
                let target = (pc as u32).wrapping_add(offset.as_u32());
                let symbol_start = symbol.address as u32;
                let symbol_end = symbol_start.wrapping_add(symbol.bytes.len() as u32);
                if dest == Reg::ZERO && target >= symbol_start && target < symbol_end {
                    let Some(target_index) = instruction_indices.get(&target).copied() else {
                        blockers.push(format!(
                            "invalid local jump target at {pc:#x}: {instruction}"
                        ));
                        break;
                    };
                    instruction_index = target_index;
                    values[0] = SymbolicValue::Constant(0);
                    continue;
                }
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
                if target < symbol_start || target >= symbol_end {
                    let arguments = structural_call_arguments(
                        &values,
                        &stack,
                        private_stack_may_be_modified_by_call,
                    );
                    private_stack_may_be_modified_by_call |= arguments
                        .iter()
                        .any(|argument| argument.private_stack_offset().is_some());
                    if dest == Reg::ZERO {
                        let call_token = next_call_token;
                        reference_events.push(DraftReferenceEvent::TailCall {
                            token: call_token,
                            site: pc as u32,
                            target,
                            arguments,
                        });
                        return_value = SymbolicValue::CallResult(call_token);
                        break;
                    } else if dest == Reg::RA {
                        let call_token = next_call_token;
                        next_call_token += 1;
                        reference_events.push(DraftReferenceEvent::Call {
                            token: call_token,
                            site: pc as u32,
                            target,
                            arguments,
                        });
                        structural_finish_call(
                            &mut values,
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
                    &values[usize::from(base.0)],
                    SymbolicValue::FunctionPointer { .. }
                ) =>
            {
                let SymbolicValue::FunctionPointer { table, target } =
                    values[usize::from(base.0)].clone()
                else {
                    unreachable!()
                };
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
                if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                    reference_blockers.push(format!(
                        "unsupported function-table call shape at {pc:#x}: {}::{target:#010x}",
                        table.id()
                    ));
                    break;
                }
                let arguments = structural_call_arguments(
                    &values,
                    &stack,
                    private_stack_may_be_modified_by_call,
                );
                private_stack_may_be_modified_by_call |= arguments
                    .iter()
                    .any(|argument| argument.private_stack_offset().is_some());
                let call_token = next_call_token;
                if dest == Reg::ZERO {
                    reference_events.push(DraftReferenceEvent::TailCall {
                        token: call_token,
                        site: pc as u32,
                        target,
                        arguments,
                    });
                    return_value = SymbolicValue::CallResult(call_token);
                    break;
                }
                next_call_token += 1;
                reference_events.push(DraftReferenceEvent::Call {
                    token: call_token,
                    site: pc as u32,
                    target,
                    arguments,
                });
                structural_finish_call(
                    &mut values,
                    (pc as u32).wrapping_add(u32::from(width)),
                    call_token,
                    target,
                    pointer_context,
                );
            }
            Inst::Jalr { offset, base, dest }
                if matches!(
                    &values[usize::from(base.0)],
                    SymbolicValue::ExternalFunction { .. }
                ) =>
            {
                let SymbolicValue::ExternalFunction { table, function } =
                    values[usize::from(base.0)].clone()
                else {
                    unreachable!()
                };
                let slot = function.spec();
                if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                    blockers.push(format!(
                        "unsupported external ABI call shape at {pc:#x}: {instruction}"
                    ));
                    break;
                }
                let mut arguments = (0..usize::from(slot.argument_count))
                    .map(|index| values[10 + index].clone())
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                let mut private_stack_output = None;
                let result = match slot.return_model {
                    ExternalReturnModel::Constant(value) => SymbolicValue::Constant(value),
                    ExternalReturnModel::SymbolicU32 => {
                        SymbolicValue::ExternalResult(next_external_call_token)
                    }
                    ExternalReturnModel::PrivateStackOutputU8 { pointer_argument } => {
                        let Some(SymbolicValue::StackAddress(offset)) =
                            arguments.get(usize::from(pointer_argument))
                        else {
                            blockers.push(format!(
                                "call/jump instruction at {pc:#x}: external ABI {}::{}",
                                table.spec().id,
                                slot.c_name
                            ));
                            reference_blockers.push(format!(
                                "unsupported-external-output-pointer at {pc:#x}: {}::{} argument a{pointer_argument} is not private stack",
                                table.spec().id,
                                slot.c_name
                            ));
                            break;
                        };
                        let output =
                            SymbolicValue::ExternalResult(next_external_call_token).and(0xff);
                        stack.store(*offset, 8, &output);
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
                };
                reference_events.push(DraftReferenceEvent::ExternalCall {
                    token: next_external_call_token,
                    table,
                    function,
                    arguments,
                });
                if let Some((offset, value)) = private_stack_output {
                    reference_events.push(DraftReferenceEvent::PrivateStackStore {
                        offset,
                        width: 8,
                        value,
                    });
                }
                next_external_call_token += 1;
                if dest == Reg::ZERO {
                    return_value = result;
                    break;
                }
                structural_finish_call_with_result(
                    &mut values,
                    (pc as u32).wrapping_add(u32::from(width)),
                    result,
                );
            }
            Inst::Jalr { offset, base, dest }
                if dest == Reg::ZERO && base == Reg::RA && offset.as_u32() == 0 =>
            {
                return_value = values[usize::from(Reg::A0.0)].clone();
                break;
            }
            Inst::Jalr { .. } => {
                blockers.push(format!("call/jump instruction at {pc:#x}: {instruction}"));
            }
            Inst::Fence { fence } => {
                let event = ObservableEvent::Fence {
                    fm: fence.fm,
                    predecessor: encode_fence_set(fence.pred),
                    successor: encode_fence_set(fence.succ),
                };
                events.push(event.clone());
                reference_events.push(DraftReferenceEvent::Observable(event));
            }
            Inst::Ecall
            | Inst::Ebreak
            | Inst::LrW { .. }
            | Inst::ScW { .. }
            | Inst::AmoW { .. } => {
                blockers.push(format!(
                    "unsupported execution edge at {pc:#x}: {instruction}"
                ));
            }
            _ => {
                blockers.push(format!("unsupported instruction at {pc:#x}: {instruction}"));
            }
        }
        values[0] = SymbolicValue::Constant(0);
        instruction_index += 1;
    }

    let private_stack_crosses_call_boundary = reference_events.iter().any(|event| match event {
        DraftReferenceEvent::Call { arguments, .. }
        | DraftReferenceEvent::TailCall { arguments, .. } => arguments
            .iter()
            .any(|argument| argument.private_stack_offset().is_some()),
        _ => false,
    });
    if !private_stack_crosses_call_boundary
        && !reference_events
            .iter()
            .any(|event| matches!(event, DraftReferenceEvent::PrivateStackLoad { .. }))
    {
        reference_events
            .retain(|event| !matches!(event, DraftReferenceEvent::PrivateStackStore { .. }));
    }

    Ok(FunctionAnalysis {
        symbol: symbol.name.clone(),
        events,
        reference_events,
        reference_dependencies: Vec::new(),
        blockers,
        reference_blockers,
        return_value,
        reference_flow: None,
        unresolved_branch,
    })
}
