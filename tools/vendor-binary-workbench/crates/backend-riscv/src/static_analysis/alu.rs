//! Symbolic semantics for register-only RV32 integer instructions.

use super::*;

pub(super) fn apply_alu_instruction(
    decoded: artifact::DecodedInstruction,
    symbol: &artifact::ArtifactSymbolDefinition,
    values: &mut [SymbolicValue; 32],
    reference_blockers: &mut Vec<String>,
) -> bool {
    let pc = decoded.address;
    let width = decoded.width;
    let instruction = decoded.instruction;
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
                    structural_set(values, dest, SymbolicValue::Unknown);
                } else {
                    structural_set(values, dest, relocation_symbol_address(symbol, relocation));
                }
            } else {
                structural_set(values, dest, SymbolicValue::Constant(uimm.as_u32()));
            }
        }
        Inst::Auipc { uimm, dest } => {
            structural_set(
                values,
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
            structural_set(values, dest, value);
        }
        Inst::Andi { imm, dest, src1 } => {
            let value = values[usize::from(src1.0)]
                .clone()
                .and(artifact::andi_immediate(imm, width));
            structural_set(values, dest, value);
        }
        Inst::Ori { imm, dest, src1 } => {
            let value = values[usize::from(src1.0)].clone().or(imm.as_u32());
            structural_set(values, dest, value);
        }
        Inst::Xori { imm, dest, src1 } => {
            let value = values[usize::from(src1.0)].clone().xor(imm.as_u32());
            structural_set(values, dest, value);
        }
        Inst::Slli { imm, dest, src1 } => {
            let value = values[usize::from(src1.0)].clone().shift_left(imm.as_u32());
            structural_set(values, dest, value);
        }
        Inst::Srli { imm, dest, src1 } => {
            let value = values[usize::from(src1.0)]
                .clone()
                .shift_right(imm.as_u32());
            structural_set(values, dest, value);
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
                |value| SymbolicValue::Constant((value as i32).wrapping_shr(imm.as_u32()) as u32),
            );
            structural_set(values, dest, value);
        }
        Inst::Sltiu { imm, dest, src1 } if imm.as_u32() == 1 => {
            let value = values[usize::from(src1.0)].clone().seqz();
            structural_set(values, dest, value);
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
            structural_set(values, dest, value);
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
                (Some(left), Some(right)) if operation == ExpressionOperation::LessThanSigned => {
                    SymbolicValue::Constant(u32::from((left as i32) < (right as i32)))
                }
                (Some(left), Some(right)) => SymbolicValue::Constant(u32::from(left < right)),
                _ => SymbolicValue::expression(operation, left, right),
            };
            structural_set(values, dest, value);
        }
        Inst::And { dest, src1, src2 } => {
            let value = values[usize::from(src1.0)]
                .clone()
                .symbolic_bitand(values[usize::from(src2.0)].clone());
            structural_set(values, dest, value);
        }
        Inst::Or { dest, src1, src2 } => {
            let value = values[usize::from(src1.0)]
                .clone()
                .symbolic_bitor(values[usize::from(src2.0)].clone());
            structural_set(values, dest, value);
        }
        Inst::Xor { dest, src1, src2 } => {
            let value = values[usize::from(src1.0)]
                .clone()
                .symbolic_bitxor(values[usize::from(src2.0)].clone());
            structural_set(values, dest, value);
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
            structural_set(values, dest, value);
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
            structural_set(values, dest, value);
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
            structural_set(values, dest, value);
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
            structural_set(values, dest, value);
        }
        Inst::Mulh { dest, .. } | Inst::Mulhsu { dest, .. } | Inst::Mulhu { dest, .. } => {
            structural_set(values, dest, SymbolicValue::Unknown);
        }
        _ => return false,
    }
    true
}
