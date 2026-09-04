//! Typed result model and human renderer for concrete execution comparison.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::execution;
use open_radio_vendor_semantics::{EquivalenceMode, EquivalenceVerdict};

/// Persistent concrete-comparison report schema.
///
/// Schema 16 records the exact compiled knowledge-provider revision and the
/// sorted diagnostic ABI contracts installed for execution. Readers must not
/// accept a comparison without knowing which opaque diagnostic boundaries
/// affected reachability and concrete replay.
pub const EXECUTION_COMPARISON_REPORT_SCHEMA: u32 = 16;

/// One reviewed opaque diagnostic call boundary installed in the executor.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticCallContractReport {
    pub symbol: String,
    pub argument_count: u8,
}

/// Canonical provenance for the diagnostic contracts used by an execution.
///
/// `knowledge_provider` records the composed knowledge and executable-model
/// IDs with their revisions. A neutral target has no provider and must
/// keep `calls` empty. Calls are sorted by `(symbol, argument_count)` so the
/// same compiled contract has one report and fingerprint identity regardless
/// of declaration order.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticContractsReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge_provider: Option<String>,
    pub calls: Vec<DiagnosticCallContractReport>,
}

impl DiagnosticContractsReport {
    pub(crate) fn from_calls(
        knowledge_provider: Option<String>,
        calls: impl IntoIterator<Item = (impl Into<String>, u8)>,
    ) -> crate::Result<Self> {
        let mut calls = calls
            .into_iter()
            .map(|(symbol, argument_count)| DiagnosticCallContractReport {
                symbol: symbol.into(),
                argument_count,
            })
            .collect::<Vec<_>>();
        calls.sort();
        let report = Self {
            knowledge_provider,
            calls,
        };
        report.validate()?;
        Ok(report)
    }

    pub(crate) fn validate(&self) -> crate::Result<()> {
        if self.knowledge_provider.is_none() && !self.calls.is_empty() {
            return Err(crate::Error::invalid(
                "neutral diagnostic contract provenance must be empty",
            ));
        }
        if let Some(provider) = self.knowledge_provider.as_deref() {
            let valid = provider
                .rsplit_once('@')
                .filter(|(id, _)| !id.is_empty())
                .and_then(|(_, revision)| revision.parse::<u32>().ok())
                .is_some_and(|revision| revision > 0);
            if !valid {
                return Err(crate::Error::invalid(format!(
                    "diagnostic contract knowledge-provider identity {provider:?} must be id@analysis_cache_revision",
                )));
            }
        }
        for call in &self.calls {
            if call.symbol.trim().is_empty() {
                return Err(crate::Error::invalid(
                    "diagnostic call symbol must not be empty",
                ));
            }
            if call.argument_count > 8 {
                return Err(crate::Error::invalid(format!(
                    "diagnostic call {} declares {} arguments; RV32 execution supports at most 8 register arguments",
                    call.symbol, call.argument_count
                )));
            }
        }
        for pair in self.calls.windows(2) {
            if pair[0] > pair[1] {
                return Err(crate::Error::invalid(
                    "diagnostic call contracts must be sorted by symbol and argument count",
                ));
            }
            if pair[0].symbol == pair[1].symbol {
                return Err(crate::Error::invalid(format!(
                    "diagnostic call {} is declared more than once",
                    pair[0].symbol
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn configured_calls(&self) -> impl Iterator<Item = (&str, u8)> {
        self.calls
            .iter()
            .map(|call| (call.symbol.as_str(), call.argument_count))
    }

    /// Length-delimited canonical identity used by caches and evidence
    /// fingerprints. The human-readable report retains the same inputs.
    pub(crate) fn canonical(&self) -> String {
        let provider = self.knowledge_provider.as_deref().unwrap_or("<none>");
        let mut identity = format!("provider:{}:{provider}", provider.len());
        for call in &self.calls {
            use std::fmt::Write as _;
            write!(
                identity,
                "\ncall:{}:{}:{}",
                call.symbol.len(),
                call.symbol,
                call.argument_count
            )
            .expect("writing to a String cannot fail");
        }
        identity
    }
}

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

#[derive(Debug, Serialize)]
pub struct SetupPhaseReport {
    pub name: String,
    pub symbol: String,
    pub completion: ExecutionCompletionReport,
    pub steps: u64,
    pub calls: Vec<String>,
    pub memory_changes: Vec<MemoryChangeReport>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageScopeReport {
    StaticDomain,
    ConcreteStateCases,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
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
    Transaction,
    Event,
    Memory,
    ReturnValue,
    Coverage,
}

impl DifferenceKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Transaction => "transaction",
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
    Transaction {
        transaction: OrderedTransactionReport,
    },
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

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OrderedTransactionReport {
    Observable {
        event: ExecutionEventReport,
    },
    Call {
        site: u32,
        symbol: String,
        arguments: [u32; 8],
    },
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
        operation: String,
        ordering: String,
        address: u32,
        succeeded: Option<bool>,
    },
    Return {
        value: u32,
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
pub struct MemoryInputReport {
    pub address: u32,
    pub value: u8,
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
    pub vendor_stack_fill: Option<u8>,
    pub rust_stack_fill: Option<u8>,
    pub vendor_tables: Vec<TableInstanceReport>,
    pub rust_tables: Vec<TableInstanceReport>,
    pub vendor_memory_instances: Vec<RuntimeMemoryInstanceReport>,
    pub rust_memory_instances: Vec<RuntimeMemoryInstanceReport>,
    pub vendor_carried_memory: Vec<MemoryInputReport>,
    pub rust_carried_memory: Vec<MemoryInputReport>,
    pub vendor_explicit_memory: Vec<MemoryInputReport>,
    pub rust_explicit_memory: Vec<MemoryInputReport>,
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

/// One effect shared by both sides of a successful comparison, with separate
/// producer provenance.  Storing the event once keeps aggregate reports small
/// while retaining the exact vendor-to-Rust correspondence needed by focused
/// investigation.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MatchedEventReport {
    pub index: usize,
    pub event: ExecutionEventReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_producer: Option<EventProducerReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_producer: Option<EventProducerReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MatchedTraceReport {
    pub events: Vec<MatchedEventReport>,
    pub vendor_transactions: Vec<OrderedTransactionReport>,
    pub rust_transactions: Vec<OrderedTransactionReport>,
    pub memory_changes: Vec<MemoryChangeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_value: Option<u32>,
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
        trace: MatchedTraceReport,
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
    pub case_execution: super::profiles::CaseExecution,
    pub diagnostic_contracts: DiagnosticContractsReport,
    pub coverage_scope: CoverageScopeReport,
    pub vendor_setup: Vec<SetupPhaseReport>,
    /// Concrete Rust PCs reached by all complete cases. Kept out of the
    /// persistent report; the verification engine consumes it immediately to
    /// prove that a reviewed production component was actually executed.
    #[serde(skip)]
    pub(crate) rust_executed_pcs: std::collections::BTreeSet<u32>,
    pub cases: Vec<CaseReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage_gap: Option<TraceDiffReport>,
    pub vendor_coverage: CoverageReport,
    pub rust_coverage: CoverageReport,
    pub summary: ComparisonSummary,
    pub verdict: EquivalenceVerdict,
}
