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
    InterfaceAnalysisGap, InterfaceArgumentValue, InterfaceCallCandidate, InterfaceCallKind,
    InterfaceDiscoveryLimits, InterfaceGapReason, InterfaceLoad, InterfacePointer,
    InterfaceRegisterValue, InterfaceRoot, InterfaceSlotAssignment, InterfaceSlotSelector,
    InterfaceSymbolAddressing,
};
use state::*;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InterfaceDiscovery {
    pub limits: InterfaceDiscoveryLimits,
    pub gaps: Vec<InterfaceAnalysisGap>,
    pub calls: Vec<InterfaceCallCandidate>,
    pub assignments: Vec<InterfaceSlotAssignment>,
    pub decode_blockers: Vec<artifact::UnsupportedInstruction>,
}

impl std::ops::Deref for InterfaceDiscovery {
    type Target = [InterfaceCallCandidate];

    fn deref(&self) -> &Self::Target {
        &self.calls
    }
}

/// Discover indirect calls with recoverable straight-line/merged pointer
/// provenance. Results are candidates, not semantic or completeness claims.
pub fn discover_interface_calls(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Result<InterfaceDiscovery> {
    discover_interface_calls_with_data_symbols(symbol, &[])
}

/// Discover indirect calls and statically initialized pointer-cell stores in
/// an authoritative linked image.
///
/// Linked instructions contain final numeric addresses even when retained ELF
/// relocations are ambiguous local aliases. `data_symbols` is therefore used
/// to attach every containing data definition as a candidate. Original numeric
/// bases and load/store offsets remain intact; missing ranges are explicit
/// unknowns. Neither a containing symbol nor a numeric store proves a pointer
/// type, initialization, interface layout or execution of the producer.
pub fn discover_interface_calls_with_data_symbols(
    symbol: &artifact::ArtifactSymbolDefinition,
    data_symbols: &[artifact::ArtifactDataSymbolDefinition],
) -> Result<InterfaceDiscovery> {
    discover_interface_calls_with_limits(symbol, data_symbols, InterfaceDiscoveryLimits::default())
}

/// Discover candidates under explicit propagation limits. A limit retains all
/// accumulated states and records the unprocessed frontier as typed gaps.
pub fn discover_interface_calls_with_limits(
    symbol: &artifact::ArtifactSymbolDefinition,
    data_symbols: &[artifact::ArtifactDataSymbolDefinition],
    limits: InterfaceDiscoveryLimits,
) -> Result<InterfaceDiscovery> {
    let instructions = artifact::decode_symbol_for_analysis(symbol)?;
    if instructions.is_empty() {
        return Ok(InterfaceDiscovery {
            limits,
            ..InterfaceDiscovery::default()
        });
    }
    let instruction_indices = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.address() as u32, index))
        .collect::<BTreeMap<_, _>>();
    let mut states = BTreeMap::from([(0usize, initial_state(&symbol.identity))]);
    let mut queue = VecDeque::from([0usize]);
    let mut decode_blockers = BTreeMap::new();

    let mut gaps = BTreeSet::new();
    let mut processed = 0usize;
    while let Some(index) = queue.front().copied() {
        let observed = states[&index]
            .iter()
            .map(|value| value.atoms().len())
            .max()
            .unwrap_or(0);
        let limit = if observed > limits.max_value_alternatives {
            Some(InterfaceGapReason::ValueAlternativeLimit {
                limit: limits.max_value_alternatives,
                observed,
            })
        } else if processed >= limits.max_state_updates {
            Some(InterfaceGapReason::StateUpdateLimit {
                limit: limits.max_state_updates,
                processed,
            })
        } else {
            None
        };
        if let Some(reason) = limit {
            for pending in queue.iter().copied().collect::<BTreeSet<_>>() {
                gaps.insert(capture_gap(
                    symbol,
                    instructions[pending].address() as u32,
                    reason.clone(),
                    &states[&pending],
                ));
            }
            break;
        }
        queue.pop_front();
        processed += 1;
        let decoded_or_blocker = instructions[index];
        let Some(decoded) = decoded_or_blocker.supported() else {
            let artifact::AnalysisInstruction::Unsupported(blocker) = decoded_or_blocker else {
                unreachable!();
            };
            decode_blockers.insert(blocker.address, blocker);
            gaps.insert(capture_gap(
                symbol,
                blocker.address as u32,
                InterfaceGapReason::UnsupportedInstruction,
                &states[&index],
            ));
            let mut values = states[&index].clone();
            if let Some(destination) = blocker.integer_destination {
                values[usize::from(destination)] = Value::Unknown;
            }
            values[0] = Value::Constant(0);
            if blocker.linear_control_flow && index + 1 < instructions.len() {
                enqueue_state(index + 1, &values, &mut states, &mut queue);
            }
            continue;
        };
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
                let value = left.add(right);
                set(&mut values, dest, value);
                successors.push(index + 1);
            }
            Inst::Slli { imm, dest, src1 } | Inst::SlliW { imm, dest, src1 } => {
                let value = values[usize::from(src1.0)].clone().shift_left(imm.as_u32());
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
                    Some(Some((_, value))) => append_load(value, pc, 0, width, data_symbols),
                    Some(None) => Value::Unknown,
                    None => append_load(
                        values[usize::from(base.0)].clone(),
                        pc,
                        offset.as_i32(),
                        width,
                        data_symbols,
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
                if let Some(register) = clear_destination(instruction, &mut values) {
                    gaps.insert(capture_gap(
                        symbol,
                        pc,
                        InterfaceGapReason::UnmodeledValueTransform {
                            registers: vec![register.0],
                        },
                        &states[&index],
                    ));
                }
                successors.push(index + 1);
            }
        }

        if !matches!(
            instruction,
            Inst::Jal { .. } | Inst::Jalr { .. } | Inst::Ecall
        ) {
            let lost = states[&index]
                .iter()
                .zip(&values)
                .enumerate()
                .filter_map(|(register, (before, after))| {
                    (*before != Value::Unknown && *after == Value::Unknown)
                        .then_some(register as u8)
                })
                .collect::<Vec<_>>();
            if !lost.is_empty() {
                gaps.insert(capture_gap(
                    symbol,
                    pc,
                    InterfaceGapReason::UnmodeledValueTransform { registers: lost },
                    &states[&index],
                ));
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
    let mut assignments = BTreeSet::new();
    for (index, values) in states {
        let Some(decoded) = instructions[index].supported() else {
            continue;
        };
        if let Inst::Sw { offset, src, base } = decoded.instruction {
            let locations = values[usize::from(base.0)]
                .atoms()
                .iter()
                .filter_map(|value| value.as_data_store_pointer(offset.as_i32(), 32, data_symbols))
                .collect::<Vec<_>>();
            let targets = values[usize::from(src.0)]
                .atoms()
                .iter()
                .filter_map(|value| value.as_data_pointer(data_symbols))
                .collect::<Vec<_>>();
            if locations.len() != values[usize::from(base.0)].atoms().len() {
                gaps.insert(capture_gap(
                    symbol,
                    decoded.address as u32,
                    InterfaceGapReason::UnresolvedAssignmentLocation,
                    &values,
                ));
            }
            if targets.len() != values[usize::from(src.0)].atoms().len() {
                gaps.insert(capture_gap(
                    symbol,
                    decoded.address as u32,
                    InterfaceGapReason::UnresolvedAssignmentValue,
                    &values,
                ));
            }
            for location in &locations {
                for target in &targets {
                    assignments.insert(InterfaceSlotAssignment {
                        owner: symbol.identity.clone(),
                        member: symbol.member.clone(),
                        function: symbol.name.clone(),
                        function_address: symbol.address as u32,
                        site: decoded.address as u32,
                        root: location.root.clone(),
                        container_loads: location.loads.clone(),
                        offset: location.post_offset,
                        width: 32,
                        target: target.root.clone(),
                        target_loads: target.loads.clone(),
                        target_offset: target.post_offset,
                    });
                }
            }
        }
        let Inst::Jalr { offset, base, dest } = decoded.instruction else {
            continue;
        };
        if dest == Reg::ZERO && base == Reg::RA && offset.as_i32() == 0 {
            continue;
        }
        let kind = if dest == Reg::RA {
            InterfaceCallKind::Call
        } else if dest == Reg::ZERO {
            InterfaceCallKind::TailJump
        } else {
            InterfaceCallKind::LinkedJump(dest.0)
        };
        if values[usize::from(base.0)]
            .atoms()
            .iter()
            .any(|value| value.as_pointers().is_empty())
        {
            gaps.insert(capture_gap(
                symbol,
                decoded.address as u32,
                InterfaceGapReason::UnresolvedCallTarget,
                &values,
            ));
        }
        for target in values[usize::from(base.0)].as_pointers() {
            calls.insert(InterfaceCallCandidate {
                owner: symbol.identity.clone(),
                member: symbol.member.clone(),
                function: symbol.name.clone(),
                function_address: symbol.address as u32,
                site: decoded.address as u32,
                kind,
                target,
                jalr_offset: offset.as_i32(),
                arguments: (0..RV32_REGISTER_ARGUMENT_COUNT)
                    .map(|argument| values[10 + argument].as_argument())
                    .collect(),
            });
        }
    }

    Ok(InterfaceDiscovery {
        limits,
        gaps: gaps.into_iter().collect(),
        calls: calls.into_iter().collect(),
        assignments: assignments.into_iter().collect(),
        decode_blockers: decode_blockers.into_values().collect(),
    })
}

fn capture_gap(
    symbol: &artifact::ArtifactSymbolDefinition,
    site: u32,
    reason: InterfaceGapReason,
    values: &RegisterState,
) -> InterfaceAnalysisGap {
    InterfaceAnalysisGap {
        owner: symbol.identity.clone(),
        member: symbol.member.clone(),
        function: symbol.name.clone(),
        site,
        reason,
        registers: values
            .iter()
            .enumerate()
            .map(|(register, value)| InterfaceRegisterValue {
                register: register as u8,
                value: value.as_argument(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
