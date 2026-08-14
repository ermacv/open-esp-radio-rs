//! Fail-closed structural tracing of RV32 functions.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use crate::{
    ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG, BitSource, BranchCondition, BranchOperation,
    DEFERRED_CALLER_MEMORY_REGION, DirectSemanticFunctionSpec, DraftReferenceEvent,
    ExpressionOperation, ExternalOutputModel, ExternalReturnModel, FunctionAnalysis,
    FunctionTableRef, IndexedMmioDomain, IndexedMmioRegister, LocatedObservableEvent,
    LocatedReferenceEvent, MemoryAccess, MemoryObjectLocation, MemoryObjectRoot, MmioMap,
    OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG, ObservableEvent, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, Result, ReviewedExternalCall, Rv32CallArguments,
    SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue, artifact, collect_evaluable_input_bits,
    encode_fence_set, evaluate_for_input, indexed_mmio_domain,
};

mod alu;
mod calls;
mod context;
mod memory;
mod memory_access;
mod poll;
mod stack;
mod state;

use alu::apply_alu_instruction;
use calls::{StructuralCallControl, apply_call_instruction, apply_relocated_call};
pub use context::{
    StructuralCallSite, StructuralPointerContext, StructuralProjectedRelocation,
    StructuralRelocatedCalls,
};
use memory::*;
use memory_access::{apply_floating_memory_instruction, apply_memory_instruction};
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

/// Resource limits for one structural trace.
///
/// Ordinary reference generation uses the unbounded policy because its
/// callers select individual reviewed functions. Artifact-wide inventories
/// use an explicit bounded policy so malformed or unexpectedly large vendor
/// functions become fail-closed blockers instead of exhausting host memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructuralTraceBudget {
    pub max_instruction_steps: usize,
    pub max_events: usize,
}

impl StructuralTraceBudget {
    pub const UNBOUNDED: Self = Self {
        max_instruction_steps: usize::MAX,
        max_events: usize::MAX,
    };
}

#[derive(Debug)]
pub struct RiscvSummaryHooks {
    pub secondary_return_target: fn(u32) -> bool,
    pub direct_semantic:
        fn(&artifact::ArtifactSymbolDefinition) -> Option<&'static DirectSemanticFunctionSpec>,
    pub direct_external_semantic: fn(&str) -> Option<&'static DirectSemanticFunctionSpec>,
    pub reference_intrinsic: fn(
        &artifact::ArtifactSymbolDefinition,
        &MmioMap,
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
    pub contracts: &'static crate::KnowledgeContractSpec,
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

fn apply_floating_data_instruction(
    blocker: artifact::UnsupportedInstruction,
    state: &mut StructuralTraceState,
) -> bool {
    let Some(instruction) = artifact::decode_floating_data_instruction(blocker) else {
        return false;
    };
    match instruction.operation {
        artifact::FloatingDataOperation::MoveFromInteger => {
            state.floating_values[usize::from(instruction.destination)] =
                state.values[usize::from(instruction.source1)].clone();
        }
        artifact::FloatingDataOperation::MoveToInteger => {
            if instruction.destination != 0 {
                state.values[usize::from(instruction.destination)] =
                    state.floating_values[usize::from(instruction.source1)].clone();
            }
        }
        operation => {
            if matches!(
                operation,
                artifact::FloatingDataOperation::CompareLessOrEqual
                    | artifact::FloatingDataOperation::CompareLess
                    | artifact::FloatingDataOperation::CompareEqual
            ) {
                let left = state.floating_values[usize::from(instruction.source1)].as_constant();
                let right = state.floating_values[usize::from(instruction.source2)].as_constant();
                let value = left
                    .zip(right)
                    .map(|(left, right)| {
                        let left = f32::from_bits(left);
                        let right = f32::from_bits(right);
                        u32::from(match operation {
                            artifact::FloatingDataOperation::CompareLessOrEqual => left <= right,
                            artifact::FloatingDataOperation::CompareLess => left < right,
                            artifact::FloatingDataOperation::CompareEqual => left == right,
                            _ => unreachable!(),
                        })
                    })
                    .map_or(SymbolicValue::Unknown, SymbolicValue::Constant);
                structural_set(&mut state.values, Reg(instruction.destination), value);
                return true;
            }
            let magnitude = state.floating_values[usize::from(instruction.source1)]
                .clone()
                .and(0x7fff_ffff);
            let source_sign = state.floating_values[usize::from(instruction.source2)]
                .clone()
                .and(0x8000_0000);
            let sign = match operation {
                artifact::FloatingDataOperation::SignCopy => source_sign,
                artifact::FloatingDataOperation::SignNegate => source_sign.xor(0x8000_0000),
                artifact::FloatingDataOperation::SignXor => state.floating_values
                    [usize::from(instruction.source1)]
                .clone()
                .and(0x8000_0000)
                .symbolic_bitxor(source_sign),
                _ => unreachable!(),
            };
            state.floating_values[usize::from(instruction.destination)] =
                magnitude.symbolic_bitor(sign);
        }
    }
    true
}

pub fn trace_binary_symbol(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
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

pub fn trace_binary_symbol_bounded(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    budget: StructuralTraceBudget,
) -> Result<FunctionAnalysis> {
    trace_binary_symbol_with_branches_bounded(
        symbol,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        &BTreeMap::new(),
        budget,
    )
}

pub fn trace_binary_symbol_with_branches(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    forced_branches: &BTreeMap<u32, bool>,
) -> Result<FunctionAnalysis> {
    trace_binary_symbol_with_branches_bounded(
        symbol,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        forced_branches,
        StructuralTraceBudget::UNBOUNDED,
    )
}

pub fn trace_binary_symbol_with_branches_bounded(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    forced_branches: &BTreeMap<u32, bool>,
    budget: StructuralTraceBudget,
) -> Result<FunctionAnalysis> {
    let program = StructuralProgram::decode(symbol)?;
    trace_structural_program_with_branches_bounded(
        symbol,
        &program,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        forced_branches,
        budget,
    )
}

/// Decoded, indexed function body reusable across forced branch scenarios.
///
/// Artifact-wide explorers replay the same function for multiple branch
/// decisions. Decoding and rebuilding control-flow indexes for every replay
/// is pure duplicate work and does not add evidence.
pub struct StructuralProgram {
    instructions: Vec<artifact::AnalysisInstruction>,
    instruction_indices: BTreeMap<u32, usize>,
    loop_checkpoint_addresses: BTreeSet<u32>,
}

impl StructuralProgram {
    pub fn decode(symbol: &artifact::ArtifactSymbolDefinition) -> Result<Self> {
        let instructions = artifact::decode_symbol_for_analysis(symbol)?;
        let instruction_indices = instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| (instruction.address() as u32, index))
            .collect::<BTreeMap<_, _>>();
        // Checkpoints are consumed only when a later conditional branch targets
        // an earlier instruction during polling-loop recognition. Retaining one
        // at every instruction copied symbolic stack and pointer provenance on
        // every step, although almost all checkpoints could never be queried.
        let loop_checkpoint_addresses = instructions
            .iter()
            .filter_map(|instruction| {
                let decoded = instruction.supported()?;
                let offset = match decoded.instruction {
                    Inst::Beq { offset, .. }
                    | Inst::Bne { offset, .. }
                    | Inst::Blt { offset, .. }
                    | Inst::Bge { offset, .. }
                    | Inst::Bltu { offset, .. }
                    | Inst::Bgeu { offset, .. } => offset,
                    _ => return None,
                };
                let target = (decoded.address as u32).wrapping_add(offset.as_u32());
                (target < decoded.address as u32).then_some(target)
            })
            .collect::<BTreeSet<_>>();
        Ok(Self {
            instructions,
            instruction_indices,
            loop_checkpoint_addresses,
        })
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "a reusable structural program retains the existing explicit trace contract"
)]
pub fn trace_structural_program_with_branches_bounded(
    symbol: &artifact::ArtifactSymbolDefinition,
    program: &StructuralProgram,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    forced_branches: &BTreeMap<u32, bool>,
    budget: StructuralTraceBudget,
) -> Result<FunctionAnalysis> {
    let mut state = StructuralTraceState::new(specialized_arguments);
    let instructions = &program.instructions;
    let instruction_indices = &program.instruction_indices;
    let loop_checkpoint_addresses = &program.loop_checkpoint_addresses;
    let mut instruction_index = 0usize;
    let mut instruction_steps = 0usize;
    let mut instruction_visits = BTreeMap::<u32, u16>::new();
    // Reference-flow exploration forces one outcome per unresolved branch
    // site. A loop-invariant branch inside a concrete counted loop therefore
    // has one semantic decision even though the instruction executes many
    // times. Keep only its first event; otherwise flow construction would
    // incorrectly require both outcomes again inside the already selected
    // arm.
    let mut emitted_forced_branch_decisions = BTreeSet::<u32>::new();
    let mut checkpoints = BTreeMap::<u32, StructuralCheckpoint>::new();
    while let Some(decoded_or_blocker) = instructions.get(instruction_index).copied() {
        if instruction_steps >= budget.max_instruction_steps {
            state.blockers.push(format!(
                "structural trace exceeds the artifact-wide budget of {} instruction steps",
                budget.max_instruction_steps
            ));
            break;
        }
        let emitted_events = state.events.len() + state.reference_events.len();
        if emitted_events >= budget.max_events {
            state.blockers.push(format!(
                "structural trace exceeds the artifact-wide budget of {} emitted events",
                budget.max_events
            ));
            break;
        }
        instruction_steps += 1;
        let pc = decoded_or_blocker.address();
        let width = decoded_or_blocker.width();
        let visits = instruction_visits.entry(pc as u32).or_default();
        if *visits >= MAX_STRUCTURAL_INSTRUCTION_VISITS {
            state.blockers.push(format!(
                "control-flow loop bounded unrolling exceeds {MAX_STRUCTURAL_INSTRUCTION_VISITS} visits at {pc:#x}"
            ));
            break;
        }
        *visits += 1;
        if loop_checkpoint_addresses.contains(&(pc as u32)) {
            checkpoints.insert(pc as u32, state.checkpoint());
        }
        let Some(decoded) = decoded_or_blocker.supported() else {
            let artifact::AnalysisInstruction::Unsupported(blocker) = decoded_or_blocker else {
                unreachable!();
            };
            state.blockers.push(blocker.to_string());
            apply_floating_memory_instruction(blocker, symbol, pointer_context, svd, &mut state);
            if let Some(destination) = blocker.integer_destination {
                state.values[usize::from(destination)] = SymbolicValue::Unknown;
            }
            apply_floating_data_instruction(blocker, &mut state);
            state.values[0] = SymbolicValue::Constant(0);
            if blocker.linear_control_flow {
                instruction_index += 1;
                continue;
            }
            break;
        };
        let instruction = decoded.instruction;
        match apply_relocated_call(
            decoded,
            instructions
                .get(instruction_index + 1)
                .and_then(|instruction| instruction.supported()),
            symbol,
            relocated_calls,
            pointer_context,
            &mut state,
        ) {
            StructuralCallControl::NotCall => {}
            StructuralCallControl::Advance(count) => {
                state.invalidate_floating_call_clobbers();
                state.values[0] = SymbolicValue::Constant(0);
                instruction_index += count;
                continue;
            }
            StructuralCallControl::Stop => break,
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
        match apply_call_instruction(decoded, symbol, pointer_context, &mut state) {
            StructuralCallControl::NotCall => {}
            StructuralCallControl::Advance(count) => {
                state.invalidate_floating_call_clobbers();
                state.values[0] = SymbolicValue::Constant(0);
                instruction_index += count;
                continue;
            }
            StructuralCallControl::Stop => break,
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
                            instructions,
                            loop_start_index,
                            instruction_index,
                            &condition,
                            checkpoint,
                            &state.events,
                            &state.located_reference_events,
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
                        state.push_reference_event(poll.read_site, poll.event);
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
                debug_assert_eq!(dest, Reg::ZERO);
                let target = (pc as u32).wrapping_add(offset.as_u32());
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
            Inst::Fence { fence } => {
                let event = ObservableEvent::Fence {
                    fm: fence.fm,
                    predecessor: encode_fence_set(fence.pred),
                    successor: encode_fence_set(fence.succ),
                };
                state.events.push(event.clone());
                state.located_events.push(LocatedObservableEvent {
                    site: pc as u32,
                    event: event.clone(),
                });
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
