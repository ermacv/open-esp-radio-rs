//! Fail-closed structural tracing of RV32 functions.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use crate::{
    BitSource, BranchCondition, BranchOperation, DraftReferenceEvent, ExpressionOperation,
    FunctionAnalysis, IndexedMmioDomain, MemoryAccess, MmioRegisterMap, ObservableEvent, Result,
    SymbolicValue, artifact, encode_fence_set, external_abi, indexed_mmio_domain,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum StructuralAddress {
    Absolute(u32),
    PrivateStack(i32),
    ExternalTableSlot(external_abi::Table, i32),
    CallerMemory(SymbolicValue),
    SymbolMemory(SymbolicValue),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct StructuralCallSite {
    member: Option<String>,
    symbol: String,
    address: u32,
}

impl StructuralCallSite {
    pub(crate) fn new(owner: &artifact::ArtifactSymbolDefinition, address: u32) -> Self {
        Self {
            member: owner.member.clone(),
            symbol: owner.name.clone(),
            address,
        }
    }
}

pub(crate) type StructuralRelocatedCalls = BTreeMap<StructuralCallSite, (String, Option<u32>)>;

fn structural_effective_address(
    values: &[SymbolicValue; 32],
    base: Reg,
    offset: i32,
) -> Option<StructuralAddress> {
    let base = &values[usize::from(base.0)];
    match base {
        SymbolicValue::Constant(base) => Some(StructuralAddress::Absolute(
            base.wrapping_add(offset as u32),
        )),
        SymbolicValue::StackAddress(base) => {
            Some(StructuralAddress::PrivateStack(base.wrapping_add(offset)))
        }
        SymbolicValue::ExternalTable(table) => {
            Some(StructuralAddress::ExternalTableSlot(*table, offset))
        }
        SymbolicValue::SymbolAddress {
            lo_addend: Some(_), ..
        } => Some(StructuralAddress::SymbolMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ if base.caller_memory_address() => Some(StructuralAddress::CallerMemory(
            base.clone().add_constant(offset as u32),
        )),
        _ => None,
    }
}

fn structural_indexed_mmio_address(
    values: &[SymbolicValue; 32],
    base: Reg,
    offset: i32,
    svd: &MmioRegisterMap,
) -> Option<(SymbolicValue, IndexedMmioDomain)> {
    let address = values[usize::from(base.0)]
        .clone()
        .add_constant(offset as u32);
    let domain = indexed_mmio_domain(&address, svd)?;
    Some((address, domain))
}

fn relocation_symbol_address(
    owner: &artifact::ArtifactSymbolDefinition,
    relocation: &artifact::SymbolRelocation,
) -> SymbolicValue {
    SymbolicValue::SymbolAddress {
        member: owner.member.clone(),
        symbol: relocation.symbol.clone(),
        hi_addend: relocation.addend,
        lo_addend: None,
        post_offset: 0,
    }
}

fn complete_low_relocation(
    owner: &artifact::ArtifactSymbolDefinition,
    pc: u32,
    kind: artifact::RelocationKind,
    base: &SymbolicValue,
    encoded_offset: i32,
) -> std::result::Result<Option<SymbolicValue>, String> {
    if owner.addresses_resolved {
        return Ok(None);
    }
    let Some(relocation) = owner.relocation(pc, kind) else {
        return Ok(None);
    };
    let expected_offset = ((relocation.addend as u32) << 20) as i32 >> 20;
    if encoded_offset != expected_offset {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} encodes {encoded_offset:+#x}, expected low addend {expected_offset:+#x}"
        ));
    }
    let SymbolicValue::SymbolAddress {
        member,
        symbol,
        hi_addend,
        lo_addend: None,
        post_offset: 0,
    } = base
    else {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} has no matching incomplete HI20 base"
        ));
    };
    if member != &owner.member || symbol != &relocation.symbol {
        return Err(format!(
            "relocation {kind:?} at {pc:#x} does not match its HI20 base: low={:?}::{}{:+#x}, high={member:?}::{symbol}{hi_addend:+#x}",
            owner.member, relocation.symbol, relocation.addend
        ));
    }
    Ok(Some(SymbolicValue::SymbolAddress {
        member: member.clone(),
        symbol: symbol.clone(),
        hi_addend: *hi_addend,
        lo_addend: Some(relocation.addend),
        post_offset: 0,
    }))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SymbolicStack {
    bytes: BTreeMap<i32, [BitSource; 8]>,
}

impl SymbolicStack {
    pub(crate) fn store(&mut self, offset: i32, width: u8, value: &SymbolicValue) {
        let bits = value.bits();
        for byte in 0..usize::from(width / 8) {
            self.bytes.insert(
                offset.wrapping_add(byte as i32),
                core::array::from_fn(|bit| bits[byte * 8 + bit]),
            );
        }
    }

    pub(crate) fn load(&self, offset: i32, width: u8, signed: bool) -> Option<SymbolicValue> {
        let width = usize::from(width);
        let mut bits = [BitSource::Constant(false); 32];
        for destination in 0..width {
            let byte = self
                .bytes
                .get(&offset.wrapping_add((destination / 8) as i32))?;
            bits[destination] = byte[destination % 8];
        }
        if signed {
            let sign = bits[width - 1];
            bits[width..].fill(sign);
        }
        Some(SymbolicValue::from_bits(bits))
    }
}

fn structural_set(values: &mut [SymbolicValue; 32], register: Reg, value: SymbolicValue) {
    if register != Reg::ZERO {
        values[usize::from(register.0)] = value;
    }
}

fn structural_finish_call(values: &mut [SymbolicValue; 32], return_address: u32, call_token: u32) {
    structural_finish_call_with_result(
        values,
        return_address,
        SymbolicValue::CallResult(call_token),
    );
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

pub(crate) fn trace_binary_symbol(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[SymbolicValue; 8]>,
) -> Result<FunctionAnalysis> {
    trace_binary_symbol_with_branches(
        symbol,
        svd,
        relocated_calls,
        external_pointer_cells,
        specialized_arguments,
        &BTreeMap::new(),
    )
}

pub(crate) fn trace_binary_symbol_with_branches(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[SymbolicValue; 8]>,
    forced_branches: &BTreeMap<u32, bool>,
) -> Result<FunctionAnalysis> {
    let mut values: [SymbolicValue; 32] = core::array::from_fn(|_| SymbolicValue::Unknown);
    values[0] = SymbolicValue::Constant(0);
    values[usize::from(Reg::SP.0)] = SymbolicValue::StackAddress(0);
    for index in 0..8 {
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
    let mut stack = SymbolicStack::default();

    let instructions = artifact::decode_symbol(symbol)?;
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut instruction_index = 0usize;
    let mut visited_instructions = BTreeSet::new();
    while let Some(decoded) = instructions.get(instruction_index).copied() {
        let pc = decoded.address;
        let width = decoded.width;
        let instruction = decoded.instruction;
        if !visited_instructions.insert(pc as u32) {
            blockers.push(format!(
                "control-flow loop revisits instruction at {pc:#x}: {instruction}"
            ));
            break;
        }
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
                && let Some(argument_count) = external_abi::diagnostic_argument_count(name)
            {
                if dest != Reg::RA {
                    reference_blockers.push(format!(
                        "unsupported-diagnostic-call-link-register at {pc:#x}: {name} uses {dest}"
                    ));
                    break;
                }
                let arguments = Box::new(core::array::from_fn(|index| {
                    if index < usize::from(argument_count) {
                        values[10 + index].clone()
                    } else {
                        SymbolicValue::Constant(0)
                    }
                }));
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
            let arguments = Box::new(core::array::from_fn(|index| values[10 + index].clone()));
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
                structural_finish_call(&mut values, (pc as u32).wrapping_add(8), call_token);
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
            Inst::Slti { dest, .. }
            | Inst::Sltiu { dest, .. }
            | Inst::Slt { dest, .. }
            | Inst::Sltu { dest, .. } => {
                structural_set(&mut values, dest, SymbolicValue::Unknown);
            }
            Inst::And { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitand(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Or { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitor(values[usize::from(src2.0)].clone());
                structural_set(&mut values, dest, value);
            }
            Inst::Xor { dest, src1, src2 } => {
                let value = values[usize::from(src1.0)]
                    .clone()
                    .bitxor(values[usize::from(src2.0)].clone());
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
                let relocated_external_table = symbol
                    .relocation(pc as u32, artifact::RelocationKind::Lo12I)
                    .and_then(|relocation| {
                        (relocation.addend == 0 && offset.as_i32() == 0)
                            .then(|| external_abi::table_for_pointer_symbol(&relocation.symbol))
                            .flatten()
                    });
                let address = if relocated_external_table.is_some() {
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
                let value = match (relocated_external_table, address) {
                    (Some(table), _) if width == 32 => SymbolicValue::ExternalTable(table),
                    (_, Some(StructuralAddress::Absolute(address)))
                        if width == 32 && external_pointer_cells.contains_key(&address) =>
                    {
                        SymbolicValue::ExternalTable(external_pointer_cells[&address])
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
                        match external_abi::slot(table, offset) {
                            Some(slot) => SymbolicValue::ExternalFunction {
                                table,
                                function: slot.function,
                            },
                            None => {
                                reference_blockers.push(format!(
                                    "unregistered-external-abi-slot at {pc:#x}: {}+{offset:#x}",
                                    external_abi::table_spec(table).id
                                ));
                                SymbolicValue::Unknown
                            }
                        }
                    }
                    (_, Some(StructuralAddress::PrivateStack(offset))) => {
                        stack.load(offset, width, signed).unwrap_or_else(|| {
                            reference_blockers.push(format!(
                                "uninitialized-private-stack-load at {pc:#x}: {instruction}"
                            ));
                            SymbolicValue::Unknown
                        })
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
                    let arguments =
                        Box::new(core::array::from_fn(|index| values[10 + index].clone()));
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
                        );
                    }
                }
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
                let slot = external_abi::function(table, function);
                if offset.as_u32() != 0 || !matches!(dest, Reg::ZERO | Reg::RA) {
                    blockers.push(format!(
                        "unsupported external ABI call shape at {pc:#x}: {instruction}"
                    ));
                    break;
                }
                let mut arguments = Box::new(core::array::from_fn(|index| {
                    if index < usize::from(slot.argument_count) {
                        values[10 + index].clone()
                    } else {
                        SymbolicValue::Constant(0)
                    }
                }));
                let result = match slot.return_model {
                    external_abi::ReturnModel::Constant(value) => SymbolicValue::Constant(value),
                    external_abi::ReturnModel::SymbolicU32 => {
                        SymbolicValue::ExternalResult(next_external_call_token)
                    }
                    external_abi::ReturnModel::PrivateStackOutputU8 { pointer_argument } => {
                        let Some(SymbolicValue::StackAddress(offset)) =
                            arguments.get(usize::from(pointer_argument))
                        else {
                            blockers.push(format!(
                                "call/jump instruction at {pc:#x}: external ABI {}::{}",
                                external_abi::table_spec(table).id,
                                slot.c_name
                            ));
                            reference_blockers.push(format!(
                                "unsupported-external-output-pointer at {pc:#x}: {}::{} argument a{pointer_argument} is not private stack",
                                external_abi::table_spec(table).id,
                                slot.c_name
                            ));
                            break;
                        };
                        let output =
                            SymbolicValue::ExternalResult(next_external_call_token).and(0xff);
                        stack.store(*offset, 8, &output);
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
