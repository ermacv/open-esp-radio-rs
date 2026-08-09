//! Recovery of compact MMIO polling loops from structural branch state.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct StructuralCheckpoint {
    pub(super) events_len: usize,
    pub(super) reference_events_len: usize,
    pub(super) blockers_len: usize,
    pub(super) reference_blockers_len: usize,
    pub(super) next_mmio_read_token: u32,
    pub(super) next_memory_read_token: u32,
    pub(super) memory_read_sources: BTreeMap<u32, MemoryObjectLocation>,
    pub(super) next_call_token: u32,
    pub(super) next_external_call_token: u32,
    pub(super) stack: SymbolicStack,
}

#[derive(Clone, Debug)]
pub(super) struct StructuralPollLoop {
    pub(super) event: DraftReferenceEvent,
    pub(super) checkpoint: StructuralCheckpoint,
    pub(super) read_token: u32,
}

pub(super) fn symbolic_value_depends_on_mmio_read(value: &SymbolicValue, read_token: u32) -> bool {
    match value {
        SymbolicValue::RegisterImage {
            read_token: token, ..
        }
        | SymbolicValue::IndexedRegisterImage {
            read_token: token, ..
        } => *token == read_token,
        SymbolicValue::Expression { left, right, .. } => {
            symbolic_value_depends_on_mmio_read(left, read_token)
                || symbolic_value_depends_on_mmio_read(right, read_token)
        }
        SymbolicValue::Bits(bits) => bits.iter().any(|source| {
            matches!(
                source,
                BitSource::Register {
                    read_token: token,
                    ..
                } | BitSource::IndexedRegister {
                    read_token: token,
                    ..
                } if *token == read_token
            )
        }),
        _ => false,
    }
}

pub(super) fn equality_constraints_for_read(
    value: &SymbolicValue,
    target: u32,
    read_token: u32,
    read_address: u32,
) -> Option<(u32, u32)> {
    let mut mask = 0_u32;
    let mut expected = 0_u32;
    for (destination, source) in value.bits().into_iter().enumerate() {
        let target_bit = target & (1 << destination) != 0;
        match source {
            BitSource::Constant(value) if value == target_bit => {}
            BitSource::Constant(_) => return None,
            BitSource::Register {
                read_token: token,
                address,
                bit,
                inverted,
            } if token == read_token && address == read_address => {
                let source_mask = 1_u32 << bit;
                let source_expected = target_bit ^ inverted;
                if mask & source_mask != 0 && (expected & source_mask != 0) != source_expected {
                    return None;
                }
                mask |= source_mask;
                if source_expected {
                    expected |= source_mask;
                }
            }
            _ => return None,
        }
    }
    (mask != 0).then_some((mask, expected))
}

pub(super) fn sign_constraint_for_read(
    value: &SymbolicValue,
    expected_sign: bool,
    read_token: u32,
    read_address: u32,
) -> Option<(u32, u32)> {
    let BitSource::Register {
        read_token: token,
        address,
        bit,
        inverted,
    } = value.bits()[31]
    else {
        return None;
    };
    if token != read_token || address != read_address {
        return None;
    }
    let mask = 1_u32 << bit;
    let source_expected = expected_sign ^ inverted;
    Some((mask, if source_expected { mask } else { 0 }))
}

pub(super) fn poll_exit_predicate(
    condition: &BranchCondition,
    read_token: u32,
    read_address: u32,
) -> Option<(u32, u32)> {
    match condition.operation {
        BranchOperation::NotEqual | BranchOperation::Equal => {
            let (value, target) = condition
                .right
                .as_constant()
                .map(|target| (&condition.left, target))
                .or_else(|| {
                    condition
                        .left
                        .as_constant()
                        .map(|target| (&condition.right, target))
                })?;
            let (mask, mut expected) =
                equality_constraints_for_read(value, target, read_token, read_address)?;
            if condition.operation == BranchOperation::Equal {
                if mask.count_ones() != 1 {
                    return None;
                }
                expected ^= mask;
            }
            Some((mask, expected))
        }
        BranchOperation::LessSigned if condition.right.as_constant() == Some(0) => {
            sign_constraint_for_read(&condition.left, false, read_token, read_address)
        }
        BranchOperation::GreaterEqualSigned if condition.right.as_constant() == Some(0) => {
            sign_constraint_for_read(&condition.left, true, read_token, read_address)
        }
        _ => None,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "poll recognition validates one complete structural checkpoint"
)]
pub(super) fn recognize_structural_poll_loop(
    instructions: &[artifact::AnalysisInstruction],
    loop_start_index: usize,
    branch_index: usize,
    condition: &BranchCondition,
    checkpoint: &StructuralCheckpoint,
    events: &[ObservableEvent],
    reference_events: &[DraftReferenceEvent],
    blockers: &[String],
    reference_blockers: &[String],
    next_mmio_read_token: u32,
    next_memory_read_token: u32,
    next_call_token: u32,
    next_external_call_token: u32,
    stack: &SymbolicStack,
    svd: &MmioMap,
) -> Option<StructuralPollLoop> {
    if blockers.len() != checkpoint.blockers_len
        || reference_blockers.len() != checkpoint.reference_blockers_len
        || next_mmio_read_token != checkpoint.next_mmio_read_token + 1
        || next_memory_read_token != checkpoint.next_memory_read_token
        || next_call_token != checkpoint.next_call_token
        || next_external_call_token != checkpoint.next_external_call_token
        || stack != &checkpoint.stack
        || events.len() != checkpoint.events_len + 1
        || reference_events.len() != checkpoint.reference_events_len + 1
    {
        return None;
    }

    let loop_instructions = &instructions[loop_start_index..=branch_index];
    if loop_instructions
        .iter()
        .any(|instruction| instruction.supported().is_none())
    {
        return None;
    }
    let load_count = loop_instructions
        .iter()
        .filter(|decoded| {
            matches!(
                decoded.supported().map(|decoded| decoded.instruction),
                Some(
                    Inst::Lb { .. }
                        | Inst::Lbu { .. }
                        | Inst::Lh { .. }
                        | Inst::Lhu { .. }
                        | Inst::Lw { .. }
                )
            )
        })
        .count();
    if load_count != 1
        || loop_instructions[..loop_instructions.len() - 1]
            .iter()
            .any(|decoded| {
                matches!(
                    decoded.supported().map(|decoded| decoded.instruction),
                    Some(
                        Inst::Sb { .. }
                            | Inst::Sh { .. }
                            | Inst::Sw { .. }
                            | Inst::Beq { .. }
                            | Inst::Bne { .. }
                            | Inst::Blt { .. }
                            | Inst::Bge { .. }
                            | Inst::Bltu { .. }
                            | Inst::Bgeu { .. }
                            | Inst::Jal { .. }
                            | Inst::Jalr { .. }
                            | Inst::Fence { .. }
                            | Inst::Ecall
                            | Inst::Ebreak
                            | Inst::LrW { .. }
                            | Inst::ScW { .. }
                            | Inst::AmoW { .. }
                    )
                )
            })
    {
        return None;
    }

    let DraftReferenceEvent::Observable(ObservableEvent::Memory {
        access: MemoryAccess::Read,
        width,
        address,
        ..
    }) = &reference_events[checkpoint.reference_events_len]
    else {
        return None;
    };
    let register = svd.register(*address)?;
    let read_token = checkpoint.next_mmio_read_token;
    let (mask, expected) = poll_exit_predicate(condition, read_token, *address)?;
    Some(StructuralPollLoop {
        event: DraftReferenceEvent::PollMmio {
            width: *width,
            address: SymbolicValue::Constant(*address),
            registers: vec![IndexedMmioRegister {
                address: *address,
                name: register.name.clone(),
            }],
            guard: None,
            mask,
            expected,
        },
        checkpoint: checkpoint.clone(),
        read_token,
    })
}
