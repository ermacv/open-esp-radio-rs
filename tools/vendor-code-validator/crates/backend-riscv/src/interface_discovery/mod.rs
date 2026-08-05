//! Architecture-level discovery of indirect-call interfaces.
//!
//! This pass deliberately records only recoverable pointer provenance. It
//! does not assign names, types, table bounds or platform semantics to slots.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rv_asm::{Inst, Reg};

use crate::{RV32_REGISTER_ARGUMENT_COUNT, Result, artifact};

mod model;
mod state;
pub use model::{
    InterfaceArgumentValue, InterfaceCallCandidate, InterfaceCallKind, InterfaceLoad,
    InterfacePointer, InterfaceRoot, InterfaceSymbolAddressing,
};
use state::*;

/// Discover indirect calls with recoverable straight-line/merged pointer
/// provenance. Results are candidates, not semantic or completeness claims.
pub fn discover_interface_calls(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Result<Vec<InterfaceCallCandidate>> {
    let instructions = artifact::decode_symbol(symbol)?;
    if instructions.is_empty() {
        return Ok(Vec::new());
    }
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut states = BTreeMap::from([(0usize, initial_state())]);
    let mut queue = VecDeque::from([0usize]);

    while let Some(index) = queue.pop_front() {
        let decoded = instructions[index];
        let pc = decoded.address as u32;
        let instruction = decoded.instruction;
        let mut values = states[&index].clone();
        let mut successors = Vec::new();

        match instruction {
            Inst::Lui { uimm, dest } => {
                let value = if !symbol.addresses_resolved {
                    symbol
                        .relocation(pc, artifact::RelocationKind::Hi20)
                        .map_or_else(
                            || Value::Constant(uimm.as_u32()),
                            |relocation| relocated_root(symbol, relocation),
                        )
                } else {
                    Value::Constant(uimm.as_u32())
                };
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Auipc { uimm, dest } => {
                let value = if !symbol.addresses_resolved {
                    [
                        artifact::RelocationKind::PcRelHi20,
                        artifact::RelocationKind::GotHi20,
                    ]
                    .into_iter()
                    .find_map(|kind| symbol.relocation(pc, kind))
                    .map_or_else(
                        || Value::Constant(pc.wrapping_add(uimm.as_u32())),
                        |relocation| relocated_root(symbol, relocation),
                    )
                } else {
                    Value::Constant(pc.wrapping_add(uimm.as_u32()))
                };
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Addi { imm, dest, src1 } | Inst::AddiW { imm, dest, src1 } => {
                let value = match low_relocation_value(symbol, pc, &values[usize::from(src1.0)]) {
                    Some(Some((relocation, value)))
                        if relocation.kind != artifact::RelocationKind::GotPcRelLo12I =>
                    {
                        value
                    }
                    Some(_) => Value::Unknown,
                    None => values[usize::from(src1.0)]
                        .clone()
                        .add_constant(imm.as_i32()),
                };
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Add { dest, src1, src2 } => {
                let left = values[usize::from(src1.0)].clone();
                let right = values[usize::from(src2.0)].clone();
                let value = match (left, right) {
                    (Value::Pointer(pointer), Value::Constant(offset))
                    | (Value::Constant(offset), Value::Pointer(pointer)) => {
                        Value::Pointer(pointer).add_constant(offset as i32)
                    }
                    (Value::Constant(left), Value::Constant(right)) => {
                        Value::Constant(left.wrapping_add(right))
                    }
                    _ => Value::Unknown,
                };
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Lb { offset, dest, base }
            | Inst::Lbu { offset, dest, base }
            | Inst::Lh { offset, dest, base }
            | Inst::Lhu { offset, dest, base }
            | Inst::Lw { offset, dest, base }
            | Inst::Lwu { offset, dest, base }
            | Inst::Ld { offset, dest, base } => {
                let width = match instruction {
                    Inst::Lb { .. } | Inst::Lbu { .. } => 8,
                    Inst::Lh { .. } | Inst::Lhu { .. } => 16,
                    Inst::Lw { .. } | Inst::Lwu { .. } => 32,
                    Inst::Ld { .. } => 64,
                    _ => unreachable!(),
                };
                let relocated = low_relocation_value(symbol, pc, &values[usize::from(base.0)]);
                let value = match relocated {
                    Some(Some((relocation, value)))
                        if relocation.kind == artifact::RelocationKind::GotPcRelLo12I =>
                    {
                        value
                    }
                    Some(Some((_, value))) => append_load(value, pc, 0, width),
                    Some(None) => Value::Unknown,
                    None => append_load(
                        values[usize::from(base.0)].clone(),
                        pc,
                        offset.as_i32(),
                        width,
                    ),
                };
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Jalr { dest, .. } => {
                if dest != Reg::ZERO {
                    clear_call_clobbers(&mut values);
                    successors.push(index + 1);
                }
            }
            Inst::Jal { offset, dest } => {
                if dest == Reg::ZERO {
                    if let Some(target) = branch_target(&instruction_indices, pc, offset.as_i32()) {
                        successors.push(target);
                    }
                } else {
                    clear_call_clobbers(&mut values);
                    successors.push(index + 1);
                }
            }
            Inst::Beq { offset, .. }
            | Inst::Bne { offset, .. }
            | Inst::Blt { offset, .. }
            | Inst::Bge { offset, .. }
            | Inst::Bltu { offset, .. }
            | Inst::Bgeu { offset, .. } => {
                if let Some(target) = branch_target(&instruction_indices, pc, offset.as_i32()) {
                    successors.push(target);
                }
                successors.push(index + 1);
            }
            Inst::Ebreak => {}
            Inst::Ecall => {
                clear_call_clobbers(&mut values);
                successors.push(index + 1);
            }
            Inst::Sb { .. }
            | Inst::Sh { .. }
            | Inst::Sw { .. }
            | Inst::Sd { .. }
            | Inst::Fence { .. } => successors.push(index + 1),
            _ => {
                clear_destination(instruction, &mut values);
                successors.push(index + 1);
            }
        }

        values[0] = Value::Constant(0);
        for successor in successors {
            if successor < instructions.len() {
                enqueue_state(successor, &values, &mut states, &mut queue);
            }
        }
    }

    let mut calls = BTreeSet::new();
    for (index, values) in states {
        let decoded = instructions[index];
        let Inst::Jalr { offset, base, dest } = decoded.instruction else {
            continue;
        };
        if dest == Reg::ZERO && base == Reg::RA && offset.as_i32() == 0 {
            continue;
        }
        let Value::Pointer(target) = &values[usize::from(base.0)] else {
            continue;
        };
        let kind = if dest == Reg::RA {
            InterfaceCallKind::Call
        } else if dest == Reg::ZERO {
            InterfaceCallKind::TailJump
        } else {
            InterfaceCallKind::LinkedJump(dest.0)
        };
        calls.insert(InterfaceCallCandidate {
            member: symbol.member.clone(),
            function: symbol.name.clone(),
            function_address: symbol.address as u32,
            site: decoded.address as u32,
            kind,
            target: target.clone(),
            jalr_offset: offset.as_i32(),
            arguments: (0..RV32_REGISTER_ARGUMENT_COUNT)
                .map(|argument| values[10 + argument].as_argument())
                .collect(),
        });
    }

    Ok(calls.into_iter().collect())
}

#[cfg(test)]
mod tests;
