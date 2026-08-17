//! Execution scenarios, observations and persistent-session state.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
};

use super::{ExecutableImage, execute};
use crate::{MmioMap, Result};
use open_radio_vendor_execution_model::{
    DeviceModel, ExecutionGoal, FifoLifecycleEvent, FifoServiceBinding, FifoServiceInstance,
    MemoryRange, TableInstance, TableLifecycleEvent,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionEvent {
    Read {
        width: u8,
        address: u32,
        region: String,
        register: Option<String>,
        value: u32,
    },
    Write {
        width: u8,
        address: u32,
        region: String,
        register: Option<String>,
        value: u32,
    },
    DelayMicros(u32),
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionTimelineEvent {
    Observable(ExecutionEvent),
    Call(OrderedCall),
    Branch {
        site: u32,
        taken: bool,
    },
    RamRead {
        site: u32,
        width: u8,
        address: u32,
        value: u32,
    },
    RamWrite {
        site: u32,
        width: u8,
        address: u32,
        value: u32,
    },
    Atomic {
        operation: AtomicOperation,
        ordering: AtomicOrdering,
        address: u32,
        /// `Some` only for store-conditional, whose architectural result is
        /// itself part of the ownership transition.
        succeeded: Option<bool>,
    },
}

/// Normal-RAM atomic operation retained in the detailed execution timeline.
/// It is not promoted to the MMIO effect trace, but reviewed ownership
/// adapters can require its ordering semantics explicitly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicOperation {
    LoadReserved,
    StoreConditional,
    Swap,
    Add,
    Xor,
    And,
    Or,
    Min,
    Max,
    MinUnsigned,
    MaxUnsigned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicOrdering {
    Relaxed,
    Acquire,
    Release,
    AcquireRelease,
}

/// Instruction that produced one observable event, resolved against the
/// linked image without requiring debug information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionProducer {
    pub pc: u32,
    pub symbol: Option<String>,
    pub symbol_offset: Option<u32>,
}

/// Who is allowed to change a RAM range between two modeled CPU calls.
///
/// `Interrupt`, `Dma`, and `SharedUnknown` are invalidated at every session
/// boundary and must be seeded again by the next scenario before they can be
/// read. `MmioDerived` remains CPU-owned storage whose value was computed from
/// an explicit peripheral response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the ownership schema includes interrupt/DMA domains before a contract needs them"
)]
pub enum MemoryOwner {
    Cpu,
    MmioDerived,
    Interrupt,
    Dma,
    SharedUnknown,
    Immutable,
}

impl MemoryOwner {
    pub(super) const fn may_change_outside_cpu(self) -> bool {
        matches!(self, Self::Interrupt | Self::Dma | Self::SharedUnknown)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryOwnership {
    pub range: MemoryRange,
    pub owner: MemoryOwner,
}

impl MemoryOwnership {
    pub(super) fn overlaps(self, other: Self) -> bool {
        let self_end = self.range.start.saturating_add(self.range.length);
        let other_end = other.range.start.saturating_add(other.range.length);
        self.range.start < other_end && other.range.start < self_end
    }
}

/// Memory initialization policy at an [`ExecutionSession`] boundary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "reset modes are exercised by session clients and blobray tests"
)]
pub enum ResetPolicy {
    /// An ordinary function call: retain CPU-owned writable state.
    #[default]
    Continue,
    /// Recreate `.data`/`.bss` from the immutable linked ELF image.
    ColdBoot,
    /// Recreate ELF-backed state but retain explicitly persistent/no-init RAM.
    WarmReset,
}

/// One observed range whose reported addresses are normalized for comparison
/// with a corresponding range in another ELF image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryAlias {
    pub start: u32,
    pub length: u32,
    pub comparison_start: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryChange {
    pub address: u32,
    pub before: u8,
    pub after: u8,
}

pub(super) struct MmioValue;

impl MmioValue {
    pub(super) const fn mask(width: u8) -> u32 {
        match width {
            8 => 0xff,
            16 => 0xffff,
            _ => u32::MAX,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct IndirectCall {
    pub site: u32,
    pub symbol: String,
    pub arguments: [u32; 8],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedCall {
    pub site: u32,
    pub symbol: String,
    pub arguments: [u32; 8],
}

/// One concrete write performed by a reviewed external call response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModeledCallOutput {
    /// Write one 8/16/32-bit little-endian value through an RV32 argument
    /// whose address must belong to the executor-private stack.
    PrivateStack {
        pointer_argument: u8,
        width: u8,
        value: u32,
    },
}

/// One deterministic arena consumed by a reviewed allocator call.
///
/// `capacity` describes the fresh address range reserved by the scenario;
/// the call's `size_argument` selects how many leading bytes become valid,
/// zero-initialized CPU-owned memory for this execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeledAllocation {
    pub address: u32,
    pub size_argument: u8,
    pub capacity: u32,
}

/// One concrete fresh allocation created by a reviewed external call model.
///
/// This is proof evidence about the modeled environment, not an observable
/// effect compared between vendor and Rust addresses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocationLifecycleEvent {
    pub site: u32,
    pub symbol: String,
    pub address: u32,
    pub requested: u32,
    pub capacity: u32,
    pub zeroed: bool,
}

/// Independently modeled ABI effects for one concrete external call.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModeledCallResponse {
    /// Optional values for RV32 return registers a0 (low) and a1 (high).
    pub return_words: [Option<u32>; 2],
    pub outputs: Vec<ModeledCallOutput>,
    pub allocation: Option<ModeledAllocation>,
}

impl ModeledCallResponse {
    pub const fn scalar(value: u32) -> Self {
        Self {
            return_words: [Some(value), None],
            outputs: Vec::new(),
            allocation: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scenario {
    pub arguments: Vec<u32>,
    pub mmio_initial: BTreeMap<u32, u32>,
    pub mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    /// Optional deterministic contents for otherwise-uninitialized bytes in
    /// the executor's private stack. This exists for optimized Rust ABIs that
    /// copy enum/struct padding. Verification should execute at least two
    /// distinct fills and require identical observables; `None` retains the
    /// ordinary poison-on-read behavior.
    pub private_stack_fill: Option<u8>,
    pub memory_initial: BTreeMap<u32, u8>,
    pub observed_memory: Vec<MemoryRange>,
    pub memory_aliases: Vec<MemoryAlias>,
    /// Non-ELF RAM that must survive between calls made through an
    /// [`ExecutionSession`]. ELF-backed `.data`/`.bss` is retained
    /// automatically; the private executor stack is fresh for every call.
    pub persistent_memory: Vec<MemoryRange>,
    /// Reviewed ownership of RAM that can outlive this call. Externally owned
    /// ranges become poison at every call boundary unless explicitly seeded.
    pub memory_ownership: Vec<MemoryOwnership>,
    /// Explicit responses for named linked calls that form a reviewed
    /// platform/driver boundary. Each invocation consumes one response. A
    /// declared model with too few or unused responses fails closed.
    pub call_responses: BTreeMap<String, VecDeque<ModeledCallResponse>>,
    /// Concrete callback/service tables installed by the scenario. They are
    /// materialized as 32-bit little-endian RAM words before the call starts.
    pub table_instances: Vec<TableInstance>,
    /// Stateful mechanism-neutral services available at reviewed external
    /// call boundaries. Bindings select their ABI without teaching the
    /// executor vendor or RTOS names.
    pub fifo_services: Vec<FifoServiceInstance>,
    pub fifo_bindings: Vec<FifoServiceBinding>,
    /// Fresh execution-time peripheral state. Every factory is instantiated
    /// independently for each vendor or Rust run.
    pub device_models: Vec<Arc<dyn DeviceModel>>,
    pub reset_policy: ResetPolicy,
    /// Explicit completion condition. Ordinary equivalence keeps `Return`;
    /// long-lived tasks require a reviewed bounded observation goal.
    pub goal: ExecutionGoal,
    pub max_steps: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionCompletion {
    Returned,
    GoalReached(ExecutionGoal),
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub events: Vec<ExecutionEvent>,
    pub event_producers: Vec<ExecutionProducer>,
    pub timeline: Vec<ExecutionTimelineEvent>,
    pub return_value: u32,
    pub completion: ExecutionCompletion,
    pub steps: u64,
    /// Exact instruction addresses reached by this concrete execution. This
    /// is execution provenance, not a semantic equivalence input.
    pub executed_pcs: BTreeSet<u32>,
    pub branches: BTreeSet<(u32, bool)>,
    pub ordered_branches: Vec<(u32, bool)>,
    pub calls: BTreeSet<String>,
    pub ordered_calls: Vec<OrderedCall>,
    pub indirect_calls: BTreeSet<IndirectCall>,
    pub allocations: Vec<AllocationLifecycleEvent>,
    pub table_lifecycle: Vec<TableLifecycleEvent>,
    pub table_lifecycle_complete: bool,
    pub fifo_lifecycle: Vec<FifoLifecycleEvent>,
    pub fifo_services: Vec<FifoServiceInstance>,
    pub device_model_coverage: Vec<super::DeviceModelOutcome>,
    pub memory_changes: Vec<MemoryChange>,
    /// Explicit RAM overlay at function entry. Semantic normalizers combine
    /// this with ordered writes when they need a call-time value rather than
    /// the immutable ELF baseline or the final persistent state.
    pub initial_memory: BTreeMap<u32, u8>,
    /// Bytes inherited from the preceding invocation in the same execution
    /// session. Explicit phase seeds override these bytes at entry.
    pub carried_memory: BTreeMap<u32, u8>,
    /// Bytes explicitly supplied by this invocation after symbol and runtime
    /// object bindings have been resolved.
    pub explicit_memory: BTreeMap<u32, u8>,
    /// Final bytes eligible for reuse by [`ExecutionSession`]. This contains
    /// ELF-backed writes and explicitly declared persistent RAM, never the
    /// executor's private stack.
    pub persistent_memory: BTreeMap<u32, u8>,
}

/// Persistent software memory across a sequence of vendor calls.
///
/// The linked ELF remains the immutable load baseline. Only writes to its
/// load segments and ranges explicitly declared through
/// [`Scenario::persistent_memory`] are carried into the next invocation.
/// MMIO responses and the private call stack are deliberately per-scenario.
#[derive(Clone, Debug, Default)]
pub struct ExecutionSession {
    memory: BTreeMap<u32, u8>,
    persistent_ranges: Vec<MemoryRange>,
    memory_ownership: Vec<MemoryOwnership>,
    fifo_services: BTreeMap<String, FifoServiceInstance>,
}

#[derive(Clone, Debug)]
pub struct ExecutionPhase {
    pub name: String,
    pub symbol: String,
    pub scenario: Scenario,
}

#[derive(Clone, Debug)]
pub struct ExecutionPhaseResult {
    pub name: String,
    pub symbol: String,
    pub result: ExecutionResult,
}

impl ExecutionSession {
    pub fn execute_phases(
        &mut self,
        image: &ExecutableImage,
        svd: &MmioMap,
        phases: Vec<ExecutionPhase>,
    ) -> Result<Vec<ExecutionPhaseResult>> {
        if phases.is_empty() {
            return Err("execution replay has no phases".into());
        }
        let mut names = BTreeSet::new();
        let mut output = Vec::with_capacity(phases.len());
        for phase in phases {
            if phase.name.trim().is_empty() || !names.insert(phase.name.clone()) {
                return Err(format!(
                    "execution replay phase names must be non-empty and unique: {:?}",
                    phase.name
                )
                .into());
            }
            let result = self
                .execute(image, svd, &phase.symbol, phase.scenario)
                .map_err(|error| {
                    format!(
                        "execution replay phase {} ({}) failed: {error}",
                        phase.name, phase.symbol
                    )
                })?;
            output.push(ExecutionPhaseResult {
                name: phase.name,
                symbol: phase.symbol,
                result,
            });
        }
        Ok(output)
    }

    pub fn execute(
        &mut self,
        image: &ExecutableImage,
        svd: &MmioMap,
        symbol: &str,
        mut scenario: Scenario,
    ) -> Result<ExecutionResult> {
        for service in scenario.fifo_services.drain(..) {
            match self.fifo_services.get(&service.id) {
                Some(previous)
                    if previous.handle != service.handle
                        || previous.item_width != service.item_width
                        || previous.capacity != service.capacity =>
                {
                    return Err(format!(
                        "FIFO service {} changes its handle, item width, or capacity inside one execution session",
                        service.id
                    )
                    .into());
                }
                Some(_) => {}
                None => {
                    self.fifo_services.insert(service.id.clone(), service);
                }
            }
        }
        scenario.fifo_services = self.fifo_services.values().cloned().collect();
        for ownership in scenario.memory_ownership.drain(..) {
            if let Some(previous) = self
                .memory_ownership
                .iter()
                .find(|previous| previous.overlaps(ownership) && previous.owner != ownership.owner)
            {
                return Err(format!(
                    "conflicting RAM ownership {:?} and {:?} for overlapping ranges at {:#010x}",
                    previous.owner, ownership.owner, ownership.range.start
                )
                .into());
            }
            if !self.memory_ownership.contains(&ownership) {
                self.memory_ownership.push(ownership);
            }
        }
        for range in scenario.persistent_memory.drain(..) {
            if !self.persistent_ranges.contains(&range) {
                self.persistent_ranges.push(range);
            }
        }
        match scenario.reset_policy {
            ResetPolicy::Continue => {}
            ResetPolicy::ColdBoot => self.memory.clear(),
            ResetPolicy::WarmReset => self.memory.retain(|address, _| {
                self.persistent_ranges
                    .iter()
                    .any(|range| range.contains(*address))
            }),
        }
        for ownership in &self.memory_ownership {
            if ownership.owner.may_change_outside_cpu() {
                self.memory
                    .retain(|address, _| !ownership.range.contains(*address));
            }
        }
        scenario.persistent_memory = self.persistent_ranges.clone();
        scenario.memory_ownership = self.memory_ownership.clone();

        let carried = self.memory.clone();
        let explicit = std::mem::take(&mut scenario.memory_initial);
        scenario.memory_initial = carried.clone();
        scenario.memory_initial.extend(explicit.clone());

        let mut result = execute(image, svd, symbol, scenario)?;
        result.carried_memory = carried;
        result.explicit_memory = explicit;
        self.memory.clone_from(&result.persistent_memory);
        self.fifo_services = result
            .fifo_services
            .iter()
            .cloned()
            .map(|service| (service.id.clone(), service))
            .collect();
        Ok(result)
    }

    pub fn byte(&self, image: &ExecutableImage, address: u32) -> Option<u8> {
        if self.memory_ownership.iter().any(|ownership| {
            ownership.range.contains(address) && ownership.owner.may_change_outside_cpu()
        }) {
            return None;
        }
        self.memory
            .get(&address)
            .copied()
            .or_else(|| image.loaded_byte(address))
    }
}
