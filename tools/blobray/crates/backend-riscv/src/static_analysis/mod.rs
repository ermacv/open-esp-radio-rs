//! Fail-closed structural tracing of RV32 functions.

use std::collections::{BTreeMap, BTreeSet};

use rv_asm::{Inst, Reg};

use crate::{
    ALLOCATED_EXTERNAL_RESULT_TOKEN_FLAG, BitSource, BranchCondition, BranchOperation,
    DEFERRED_CALL_RESULT_MEMORY_REGION, DEFERRED_CALLER_MEMORY_REGION, DirectSemanticFunctionSpec,
    DraftReferenceEvent, ExpressionOperation, ExternalOutputModel, ExternalReturnModel,
    FloatingPointOperation, FloatingRoundingMode, FunctionAnalysis, FunctionTableRef,
    IndexedMmioDomain, IndexedMmioRegister, LocatedObservableEvent, LocatedReferenceEvent,
    MemoryAccess, MemoryObjectLocation, MemoryObjectRoot, MmioMap,
    OPAQUE_POINTER_EXTERNAL_RESULT_TOKEN_FLAG, ObservableEvent, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, Result, ReviewedExternalCall, ReviewedExternalCallExecutionModel,
    Rv32CallArguments, SECONDARY_CALL_RESULT_TOKEN_FLAG, StandardMemoryFunction, SymbolicValue,
    UNINITIALIZED_ALLOCATION_EXTERNAL_RESULT_TOKEN_FLAG, artifact, collect_evaluable_input_bits,
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
    StructuralRelocatedCallView, StructuralRelocatedCalls,
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
    /// Exact pure result for a known external/compiler-runtime symbol. This
    /// keeps folded or deliberately opaque linked stubs out of the analysis
    /// target while preserving their standardized value semantics.
    pub direct_external_intrinsic:
        fn(&str, &Rv32CallArguments) -> Option<crate::Rv32IntrinsicResult>,
    pub reference_intrinsic: fn(
        &artifact::ArtifactSymbolDefinition,
        &MmioMap,
        &StructuralPointerContext,
    ) -> Option<FunctionAnalysis>,
    /// Optional language add-on mapping from an exact public symbol to a
    /// standardized memory contract. The backend interprets the contract for
    /// RV32; it never infers one from symbol spelling on its own.
    pub standard_memory_function: fn(&str) -> Option<StandardMemoryFunction>,
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
        artifact::FloatingDataOperation::SignedWordToSingle
        | artifact::FloatingDataOperation::SubtractSingle
        | artifact::FloatingDataOperation::DivideSingle
        | artifact::FloatingDataOperation::FusedMultiplyAddSingle
        | artifact::FloatingDataOperation::SingleToSignedWord => {
            let operation = match instruction.operation {
                artifact::FloatingDataOperation::SignedWordToSingle => {
                    FloatingPointOperation::SignedWordToSingle
                }
                artifact::FloatingDataOperation::SubtractSingle => {
                    FloatingPointOperation::SubtractSingle
                }
                artifact::FloatingDataOperation::DivideSingle => {
                    FloatingPointOperation::DivideSingle
                }
                artifact::FloatingDataOperation::FusedMultiplyAddSingle => {
                    FloatingPointOperation::FusedMultiplyAddSingle
                }
                artifact::FloatingDataOperation::SingleToSignedWord => {
                    FloatingPointOperation::SingleToSignedWord
                }
                _ => unreachable!(),
            };
            let rounding = instruction
                .rounding
                .expect("arithmetic floating instruction has a rounding mode");
            let operands = match instruction.operation {
                artifact::FloatingDataOperation::SignedWordToSingle => {
                    vec![state.values[usize::from(instruction.source1)].clone()]
                }
                artifact::FloatingDataOperation::SubtractSingle
                | artifact::FloatingDataOperation::DivideSingle => vec![
                    state.floating_values[usize::from(instruction.source1)].clone(),
                    state.floating_values[usize::from(instruction.source2)].clone(),
                ],
                artifact::FloatingDataOperation::FusedMultiplyAddSingle => vec![
                    state.floating_values[usize::from(instruction.source1)].clone(),
                    state.floating_values[usize::from(instruction.source2)].clone(),
                    state.floating_values
                        [usize::from(instruction.source3.expect("fused operation has source3"))]
                    .clone(),
                ],
                artifact::FloatingDataOperation::SingleToSignedWord => {
                    vec![state.floating_values[usize::from(instruction.source1)].clone()]
                }
                _ => unreachable!(),
            };
            let value = SymbolicValue::floating_point(operation, rounding, operands);
            if instruction.operation == artifact::FloatingDataOperation::SingleToSignedWord {
                structural_set(&mut state.values, Reg(instruction.destination), value);
            } else {
                state.floating_values[usize::from(instruction.destination)] = value;
            }
            let executable_rounding = matches!(
                (operation, rounding),
                (
                    FloatingPointOperation::SingleToSignedWord,
                    FloatingRoundingMode::TowardZero
                ) | (
                    FloatingPointOperation::SignedWordToSingle
                        | FloatingPointOperation::SubtractSingle
                        | FloatingPointOperation::DivideSingle
                        | FloatingPointOperation::FusedMultiplyAddSingle,
                    FloatingRoundingMode::NearestEven
                )
            );
            if !executable_rounding {
                state.reference_blockers.push(format!(
                    "floating-rounding-mode at {:#x}: {operation:?} uses {rounding:?}; executable reference requires an explicit reviewed rounding state",
                    blocker.address
                ));
            } else {
                state.reference_blockers.push(format!(
                    "floating-execution-model at {:#x}: {operation:?} with {rounding:?} is structurally recovered but not executable",
                    blocker.address
                ));
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
                let exact = left.zip(right);
                let value = exact
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
                // The comparison relation is not represented symbolically.
                // It is fully accounted only when both input bit patterns are
                // concrete; otherwise retain the original decode blocker.
                return exact.is_some();
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

/// Why bounded artifact-wide CFG exploration stopped before scheduling every
/// discovered path.  Consumers attach their own evidence vocabulary; the
/// architecture backend owns only the control-flow fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuralExplorationLimit {
    States { maximum: usize },
    BranchDecisions { site: u32, maximum: usize },
    RevisitedBranch { site: u32 },
}

/// Aggregate accounting for one function-local exploration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StructuralExplorationSummary {
    pub explored_states: usize,
    /// Instructions interpreted across the bounded path replays.
    pub executed_instruction_steps: usize,
    pub limits: Vec<StructuralExplorationLimit>,
}

struct StructuralTraceCursor {
    state: StructuralTraceState,
    instruction_index: usize,
    instruction_steps: usize,
    instruction_visits: BTreeMap<u32, u16>,
    emitted_branch_decisions: BTreeSet<u32>,
    checkpoints: BTreeMap<u32, StructuralCheckpoint>,
}

impl StructuralTraceCursor {
    fn initial(specialized_arguments: Option<&Rv32CallArguments>) -> Self {
        Self {
            state: StructuralTraceState::new(specialized_arguments),
            instruction_index: 0,
            instruction_steps: 0,
            instruction_visits: BTreeMap::new(),
            emitted_branch_decisions: BTreeSet::new(),
            checkpoints: BTreeMap::new(),
        }
    }
}

struct StructuralTraceRun {
    analysis: FunctionAnalysis,
    executed_instruction_steps: usize,
}

/// Explore every bounded input-dependent path of one already-decoded
/// function.  MMIO discovery and linked-IR construction intentionally share
/// this scheduler so path coverage, limits and future CFG-state caching cannot
/// drift between projections.
#[allow(
    clippy::too_many_arguments,
    reason = "the shared explorer retains the explicit backend trace contract"
)]
pub fn explore_structural_program_bounded(
    symbol: &artifact::ArtifactSymbolDefinition,
    program: &StructuralProgram,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    trace_budget: StructuralTraceBudget,
    maximum_states: usize,
    maximum_branch_decisions: usize,
    mut observe: impl FnMut(Result<FunctionAnalysis>),
) -> StructuralExplorationSummary {
    let mut summary = StructuralExplorationSummary::default();
    let mut queue = std::collections::VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let relocated_calls = StructuralRelocatedCallView::new(symbol, relocated_calls);

    while let Some(forced_branches) = queue.pop_front() {
        if summary.explored_states >= maximum_states {
            summary.limits.push(StructuralExplorationLimit::States {
                maximum: maximum_states,
            });
            break;
        }
        summary.explored_states += 1;
        let run = run_bound_structural_program_with_branches_bounded(
            symbol,
            program,
            svd,
            &relocated_calls,
            pointer_context,
            StructuralTraceCursor::initial(specialized_arguments),
            &forced_branches,
            trace_budget,
        );
        let run = match run {
            Ok(run) => run,
            Err(error) => {
                observe(Err(error));
                continue;
            }
        };
        summary.executed_instruction_steps += run.executed_instruction_steps;
        let branch = run.analysis.unresolved_branch.clone();
        observe(Ok(run.analysis));

        let Some(branch) = branch else {
            continue;
        };
        if forced_branches.len() >= maximum_branch_decisions {
            summary
                .limits
                .push(StructuralExplorationLimit::BranchDecisions {
                    site: branch.site,
                    maximum: maximum_branch_decisions,
                });
            continue;
        }
        for taken in [false, true] {
            let mut next = forced_branches.clone();
            if next.insert(branch.site, taken).is_some() {
                summary
                    .limits
                    .push(StructuralExplorationLimit::RevisitedBranch { site: branch.site });
            } else if queued.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }

    summary
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
    run_bound_structural_program_with_branches_bounded(
        symbol,
        program,
        svd,
        &StructuralRelocatedCallView::new(symbol, relocated_calls),
        pointer_context,
        StructuralTraceCursor::initial(specialized_arguments),
        forced_branches,
        budget,
    )
    .map(|run| run.analysis)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the bound reusable structural program retains the explicit trace contract"
)]
fn run_bound_structural_program_with_branches_bounded(
    symbol: &artifact::ArtifactSymbolDefinition,
    program: &StructuralProgram,
    svd: &MmioMap,
    relocated_calls: &StructuralRelocatedCallView<'_>,
    pointer_context: &StructuralPointerContext,
    cursor: StructuralTraceCursor,
    forced_branches: &BTreeMap<u32, bool>,
    budget: StructuralTraceBudget,
) -> Result<StructuralTraceRun> {
    let started_instruction_steps = cursor.instruction_steps;
    let StructuralTraceCursor {
        mut state,
        mut instruction_index,
        mut instruction_steps,
        mut instruction_visits,
        mut emitted_branch_decisions,
        mut checkpoints,
    } = cursor;
    let instructions = &program.instructions;
    let instruction_indices = &program.instruction_indices;
    let loop_checkpoint_addresses = &program.loop_checkpoint_addresses;
    // Reference-flow exploration forces one outcome per unresolved branch
    // site. A loop-invariant branch inside a concrete counted loop therefore
    // has one semantic decision even though the instruction executes many
    // times. Keep only its first event; otherwise flow construction would
    // incorrectly require both outcomes again inside the already selected
    // arm.
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
            let floating_memory_accounted = apply_floating_memory_instruction(
                blocker,
                symbol,
                pointer_context,
                svd,
                &mut state,
            );
            if let Some(destination) = blocker.integer_destination {
                state.values[usize::from(destination)] = SymbolicValue::Unknown;
            }
            let floating_data_accounted = apply_floating_data_instruction(blocker, &mut state);
            if !floating_memory_accounted && !floating_data_accounted {
                state.blockers.push(blocker.to_string());
            }
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
                    let selected = forced_branches.get(&(pc as u32)).copied();
                    let Some(taken) = selected else {
                        state.blockers.push(format!(
                            "input-dependent control-flow at {pc:#x}: {instruction}"
                        ));
                        state.unresolved_branch = Some(condition);
                        break;
                    };
                    if emitted_branch_decisions.insert(pc as u32) {
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

    // Forced-path traces are later normalized into one structured CFG. Keep
    // common private-stack spills on every path until that merge; pruning
    // them independently makes otherwise identical prefixes path-dependent.
    Ok(StructuralTraceRun {
        analysis: state.finish(symbol, !forced_branches.is_empty()),
        executed_instruction_steps: instruction_steps - started_instruction_steps,
    })
}

#[cfg(test)]
mod exploration_tests {
    use super::*;

    #[test]
    fn bounded_exploration_visits_every_discovered_path() {
        let symbol = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "branched".to_owned(),
            address: 0x1000,
            bytes: vec![
                0x63, 0x08, 0x05, 0x00, // beq a0, zero, 0x1010
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xa8, 0xc5, 0x02, // sw a2, 0x30(a1)
                0x67, 0x80, 0x00, 0x00, // ret
                0xb7, 0x75, 0x10, 0x20, // lui a1, 0x20107
                0x23, 0xaa, 0xd5, 0x02, // sw a3, 0x34(a1)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let program = StructuralProgram::decode(&symbol).unwrap();
        let mut traces = 0;
        let summary = explore_structural_program_bounded(
            &symbol,
            &program,
            &MmioMap {
                registers: Vec::new(),
                regions: Vec::new(),
            },
            &StructuralRelocatedCalls::new(),
            &StructuralPointerContext::default(),
            None,
            StructuralTraceBudget::UNBOUNDED,
            127,
            12,
            |trace| {
                trace.unwrap();
                traces += 1;
            },
        );

        assert_eq!(traces, 3);
        assert_eq!(summary.explored_states, 3);
        assert_eq!(summary.executed_instruction_steps, 9);
        assert!(summary.limits.is_empty());
    }
}
