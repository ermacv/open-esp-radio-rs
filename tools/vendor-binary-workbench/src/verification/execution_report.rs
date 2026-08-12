//! Typed result model and human renderer for concrete execution comparison.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::execution;
use open_radio_vendor_semantics::{EquivalenceMode, EquivalenceVerdict};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactReport {
    pub path: String,
    pub sha256: String,
    pub companion: Option<ArtifactIdentity>,
    pub symbol: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactIdentity {
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionEventReport {
    Read {
        width: u8,
        address: u32,
        region: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        register: Option<String>,
        value: u32,
    },
    Write {
        width: u8,
        address: u32,
        region: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        register: Option<String>,
        value: u32,
    },
    DelayMicros {
        micros: u32,
    },
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventProducerReport {
    pub pc: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_offset: Option<u32>,
}

impl From<&execution::ExecutionProducer> for EventProducerReport {
    fn from(producer: &execution::ExecutionProducer) -> Self {
        Self {
            pc: producer.pc,
            symbol: producer.symbol.clone(),
            symbol_offset: producer.symbol_offset,
        }
    }
}

impl From<&execution::ExecutionEvent> for ExecutionEventReport {
    fn from(event: &execution::ExecutionEvent) -> Self {
        match event {
            execution::ExecutionEvent::Read {
                width,
                address,
                region,
                register,
                value,
            } => Self::Read {
                width: *width,
                address: *address,
                region: region.clone(),
                register: register.clone(),
                value: *value,
            },
            execution::ExecutionEvent::Write {
                width,
                address,
                region,
                register,
                value,
            } => Self::Write {
                width: *width,
                address: *address,
                region: region.clone(),
                register: register.clone(),
                value: *value,
            },
            execution::ExecutionEvent::DelayMicros(micros) => Self::DelayMicros { micros: *micros },
            execution::ExecutionEvent::Fence {
                fm,
                predecessor,
                successor,
            } => Self::Fence {
                fm: *fm,
                predecessor: *predecessor,
                successor: *successor,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryChangeReport {
    pub address: u32,
    pub before: u8,
    pub after: u8,
}

impl From<&execution::MemoryChange> for MemoryChangeReport {
    fn from(change: &execution::MemoryChange) -> Self {
        Self {
            address: change.address,
            before: change.before,
            after: change.after,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DifferenceKind {
    Event,
    Memory,
    ReturnValue,
    Coverage,
}

impl DifferenceKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Memory => "memory",
            Self::ReturnValue => "return-value",
            Self::Coverage => "coverage",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TraceItemReport {
    Event {
        event: ExecutionEventReport,
        #[serde(skip_serializing_if = "Option::is_none")]
        producer: Option<EventProducerReport>,
    },
    Memory {
        change: MemoryChangeReport,
    },
    ReturnValue {
        value: u32,
    },
    Coverage {
        issue: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AlignedTraceItemReport {
    pub index: usize,
    pub vendor: Option<TraceItemReport>,
    pub rust: Option<TraceItemReport>,
    pub equal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BranchDecisionReport {
    pub site: u32,
    pub taken: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrderedCallReport {
    pub site: u32,
    pub symbol: String,
    pub arguments: [u32; 8],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionPathSideReport {
    pub branches: Vec<BranchDecisionReport>,
    pub calls: Vec<OrderedCallReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ExecutionPathReport {
    pub vendor: ExecutionPathSideReport,
    pub rust: ExecutionPathSideReport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceDiffReport {
    pub first_difference: usize,
    pub kind: DifferenceKind,
    pub vendor: Option<TraceItemReport>,
    pub rust: Option<TraceItemReport>,
    pub context_before: Vec<AlignedTraceItemReport>,
    pub context_after: Vec<AlignedTraceItemReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<ExecutionPathReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TableSlotTargetReport {
    Null,
    Address { address: u32 },
    Symbol { symbol: String },
    ModeledSymbol { symbol: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TableInstanceSlotReport {
    pub offset: u32,
    pub target: TableSlotTargetReport,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TableInstanceReport {
    pub layout_id: String,
    pub base_address: u32,
    pub layout_size: u32,
    pub pointer_cells: Vec<u32>,
    pub pointer_cell_symbols: Vec<String>,
    pub slots: Vec<TableInstanceSlotReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FifoServiceReport {
    pub id: String,
    pub handle: u32,
    pub item_width: u8,
    pub capacity: usize,
    pub items: Vec<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionCompletionReport {
    Returned,
    GoalReached {
        goal: crate::execution_model::ExecutionGoal,
    },
}

impl From<&execution::ExecutionCompletion> for ExecutionCompletionReport {
    fn from(completion: &execution::ExecutionCompletion) -> Self {
        match completion {
            execution::ExecutionCompletion::Returned => Self::Returned,
            execution::ExecutionCompletion::GoalReached(goal) => {
                Self::GoalReached { goal: goal.clone() }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum FifoLifecycleReport {
    Enqueued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
        woke_receiver: bool,
    },
    Dequeued {
        service_id: String,
        site: u32,
        value: u32,
        depth_before: usize,
        depth_after: usize,
    },
    Full {
        service_id: String,
        site: u32,
        value: u32,
        depth: usize,
    },
    Empty {
        service_id: String,
        site: u32,
    },
    Length {
        service_id: String,
        site: u32,
        depth: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeMemoryBindingReport {
    Argument { index: usize },
    Global { symbol: String },
    DereferencedGlobal { symbol: String, pointer_offset: u32 },
    Absolute { address_space: String, address: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeMemoryInstanceReport {
    pub id: String,
    pub base_address: u32,
    pub length: u32,
    pub bindings: Vec<RuntimeMemoryBindingReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AllocationLifecycleReport {
    pub site: u32,
    pub symbol: String,
    pub address: u32,
    pub requested: u32,
    pub capacity: u32,
    pub zeroed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum TableLifecycleReport {
    SlotInitialized {
        layout_id: String,
        offset: u32,
        target: u32,
    },
    SlotWritten {
        layout_id: String,
        offset: u32,
        width: u8,
        value: u32,
        site: u32,
    },
    PointerInstalled {
        layout_id: String,
        address: u32,
        base_address: u32,
    },
    IndirectCall {
        layout_id: Option<String>,
        slot_offset: Option<u32>,
        site: u32,
        target: u32,
        symbol: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviceModelReport {
    pub id: String,
    pub kind: String,
    pub start: u32,
    pub length: u32,
    pub configuration: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviceModelCoverageReport {
    pub id: String,
    pub kind: String,
    pub complete: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ScenarioEnvironmentReport {
    pub vendor_tables: Vec<TableInstanceReport>,
    pub rust_tables: Vec<TableInstanceReport>,
    pub vendor_memory_instances: Vec<RuntimeMemoryInstanceReport>,
    pub rust_memory_instances: Vec<RuntimeMemoryInstanceReport>,
    pub device_models: Vec<DeviceModelReport>,
    pub vendor_device_coverage: Vec<DeviceModelCoverageReport>,
    pub rust_device_coverage: Vec<DeviceModelCoverageReport>,
    pub vendor_allocations: Vec<AllocationLifecycleReport>,
    pub rust_allocations: Vec<AllocationLifecycleReport>,
    pub vendor_table_lifecycle: Vec<TableLifecycleReport>,
    pub rust_table_lifecycle: Vec<TableLifecycleReport>,
    pub vendor_table_lifecycle_complete: Option<bool>,
    pub rust_table_lifecycle_complete: Option<bool>,
    pub vendor_fifo_services: Vec<FifoServiceReport>,
    pub rust_fifo_services: Vec<FifoServiceReport>,
    pub vendor_fifo_lifecycle: Vec<FifoLifecycleReport>,
    pub rust_fifo_lifecycle: Vec<FifoLifecycleReport>,
    pub vendor_completion: Option<ExecutionCompletionReport>,
    pub rust_completion: Option<ExecutionCompletionReport>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum CaseReport {
    Match {
        name: String,
        environment: ScenarioEnvironmentReport,
        events: usize,
        memory_changes: usize,
        return_compared: bool,
    },
    Diff {
        name: String,
        environment: ScenarioEnvironmentReport,
        difference: Box<TraceDiffReport>,
    },
    Incomplete {
        name: String,
        environment: ScenarioEnvironmentReport,
        vendor_error: Option<String>,
        rust_error: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct BranchOutcomeReport {
    pub site: u32,
    pub location: String,
    pub taken: bool,
    pub covered: bool,
}

#[derive(Debug, Serialize)]
pub struct ControlFlowReport {
    pub site: u32,
    pub location: String,
    pub edge: String,
    pub targets: Vec<String>,
    pub covered: bool,
}

#[derive(Debug, Serialize)]
pub struct CoverageReport {
    pub covered_calls: Vec<String>,
    pub branch_outcomes: Vec<BranchOutcomeReport>,
    pub unresolved_control_flow: Vec<ControlFlowReport>,
    pub unnamed_mmio: Vec<u32>,
}

impl CoverageReport {
    pub fn uncovered_branch_outcomes(&self) -> usize {
        self.branch_outcomes
            .iter()
            .filter(|outcome| !outcome.covered)
            .count()
    }

    pub fn uncovered_control_flow(&self) -> usize {
        self.unresolved_control_flow
            .iter()
            .filter(|edge| !edge.covered)
            .count()
    }
}

#[derive(Debug, Serialize)]
pub struct ComparisonSummary {
    pub cases: usize,
    pub matched: usize,
    pub different: usize,
    pub incomplete: usize,
    pub vendor_uncovered_branch_outcomes: usize,
    pub rust_uncovered_branch_outcomes: usize,
    pub vendor_unresolved_control_flow: usize,
    pub rust_unresolved_control_flow: usize,
    pub vendor_unnamed_mmio: usize,
    pub rust_unnamed_mmio: usize,
}

#[derive(Debug, Serialize)]
pub struct ExecutionComparisonReport {
    pub schema_version: u32,
    pub command: &'static str,
    pub mode: EquivalenceMode,
    pub vendor: ArtifactReport,
    pub rust: ArtifactReport,
    pub compare_return: bool,
    pub cases: Vec<CaseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_gap: Option<TraceDiffReport>,
    pub vendor_coverage: CoverageReport,
    pub rust_coverage: CoverageReport,
    pub summary: ComparisonSummary,
    pub verdict: EquivalenceVerdict,
}
