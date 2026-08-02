//! Execution scenarios, observations and persistent-session state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{ExecutableImage, execute};
use crate::{MmioRegisterMap, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionEvent {
    Read {
        width: u8,
        address: u32,
        register: String,
        value: u32,
    },
    Write {
        width: u8,
        address: u32,
        register: String,
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
    Branch { site: u32, taken: bool },
    RamRead { width: u8, address: u32, value: u32 },
    RamWrite { width: u8, address: u32, value: u32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRange {
    pub start: u32,
    pub length: u32,
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
    reason = "reset modes are exercised by session clients and validator tests"
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

impl MemoryRange {
    pub(super) fn contains(self, address: u32) -> bool {
        address
            .checked_sub(self.start)
            .is_some_and(|offset| offset < self.length)
    }
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

#[derive(Clone, Debug, Default)]
pub struct Scenario {
    pub arguments: Vec<u32>,
    pub mmio_initial: BTreeMap<u32, u32>,
    pub mmio_reads: BTreeMap<u32, VecDeque<u32>>,
    /// Optional deterministic contents for otherwise-uninitialized bytes in
    /// the executor's private stack. This exists for optimized Rust ABIs that
    /// copy enum/struct padding. Qualification should execute at least two
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
    /// Explicit scalar results for named linked calls that form a reviewed
    /// platform/driver boundary. Each invocation consumes one value. A
    /// declared model with too few or unused responses fails closed.
    pub call_returns: BTreeMap<String, VecDeque<u32>>,
    pub reset_policy: ResetPolicy,
    pub max_steps: u64,
}

#[derive(Clone, Debug)]
pub struct ExecutionResult {
    pub events: Vec<ExecutionEvent>,
    pub timeline: Vec<ExecutionTimelineEvent>,
    pub return_value: u32,
    pub steps: u64,
    pub branches: BTreeSet<(u32, bool)>,
    pub ordered_branches: Vec<(u32, bool)>,
    pub calls: BTreeSet<String>,
    pub ordered_calls: Vec<OrderedCall>,
    pub indirect_calls: BTreeSet<IndirectCall>,
    pub memory_changes: Vec<MemoryChange>,
    /// Explicit RAM overlay at function entry. Semantic normalizers combine
    /// this with ordered writes when they need a call-time value rather than
    /// the immutable ELF baseline or the final persistent state.
    pub initial_memory: BTreeMap<u32, u8>,
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
}

impl ExecutionSession {
    pub fn execute(
        &mut self,
        image: &ExecutableImage,
        svd: &MmioRegisterMap,
        symbol: &str,
        mut scenario: Scenario,
    ) -> Result<ExecutionResult> {
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

        let explicit = std::mem::take(&mut scenario.memory_initial);
        scenario.memory_initial = self.memory.clone();
        scenario.memory_initial.extend(explicit);

        let result = execute(image, svd, symbol, scenario)?;
        self.memory.clone_from(&result.persistent_memory);
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
