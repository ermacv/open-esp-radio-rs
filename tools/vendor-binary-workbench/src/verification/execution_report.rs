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
    const fn label(self) -> &'static str {
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
    Event { event: ExecutionEventReport },
    Memory { change: MemoryChangeReport },
    ReturnValue { value: u32 },
    Coverage { issue: String },
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
    pub slots: Vec<TableInstanceSlotReport>,
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
    pub vendor_table_lifecycle: Vec<TableLifecycleReport>,
    pub rust_table_lifecycle: Vec<TableLifecycleReport>,
    pub vendor_table_lifecycle_complete: Option<bool>,
    pub rust_table_lifecycle_complete: Option<bool>,
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

fn trace_item_text(item: Option<&TraceItemReport>) -> String {
    let Some(item) = item else {
        return "<missing>".to_owned();
    };
    match item {
        TraceItemReport::Event { event } => match event {
            ExecutionEventReport::Read {
                width,
                address,
                region,
                register,
                value,
            } => format!(
                "READ/{width} {address:#010x} region={region} register={} -> {value:#010x}",
                register.as_deref().unwrap_or("-")
            ),
            ExecutionEventReport::Write {
                width,
                address,
                region,
                register,
                value,
            } => format!(
                "WRITE/{width} {address:#010x} region={region} register={} <- {value:#010x}",
                register.as_deref().unwrap_or("-")
            ),
            ExecutionEventReport::DelayMicros { micros } => format!("DELAY {micros} us"),
            ExecutionEventReport::Fence {
                fm,
                predecessor,
                successor,
            } => format!("FENCE fm={fm:#x} pred={predecessor:#x} succ={successor:#x}"),
        },
        TraceItemReport::Memory { change } => format!(
            "RAM {:#010x} {:#04x} -> {:#04x}",
            change.address, change.before, change.after
        ),
        TraceItemReport::ReturnValue { value } => format!("RETURN {value:#010x}"),
        TraceItemReport::Coverage { issue } => issue.clone(),
    }
}

fn print_difference(case: &str, difference: &TraceDiffReport) {
    outputln!(
        "FIRST-DIFFERENCE\tcase={case}\tkind={}\tindex={}",
        difference.kind.label(),
        difference.first_difference
    );
    for item in &difference.context_before {
        outputln!(
            "DIFF-CONTEXT\tbefore\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    outputln!(
        "DIFF-ITEM\t{}\tvendor={}\trust={}",
        difference.first_difference,
        trace_item_text(difference.vendor.as_ref()),
        trace_item_text(difference.rust.as_ref())
    );
    for item in &difference.context_after {
        outputln!(
            "DIFF-CONTEXT\tafter\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    if let Some(path) = &difference.path {
        outputln!(
            "DIFF-PATH\tvendor-branches={}\trust-branches={}\tvendor-calls={}\trust-calls={}",
            path.vendor.branches.len(),
            path.rust.branches.len(),
            path.vendor.calls.len(),
            path.rust.calls.len()
        );
    }
}

fn print_coverage(side: &str, coverage: &CoverageReport) {
    for call in &coverage.covered_calls {
        outputln!("COVERED-CALL\t{side}\t{call}");
    }
    for outcome in &coverage.branch_outcomes {
        outputln!(
            "{}\t{side}\t{}\ttaken={}",
            if outcome.covered {
                "COVERED-BRANCH"
            } else {
                "UNCOVERED-BRANCH"
            },
            outcome.location,
            outcome.taken
        );
    }
    let sites = coverage
        .branch_outcomes
        .iter()
        .map(|outcome| outcome.site)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let uncovered = coverage.uncovered_branch_outcomes();
    outputln!(
        "SUMMARY-BRANCHES\t{side}\tsites={sites}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        coverage.branch_outcomes.len(),
        coverage.branch_outcomes.len() - uncovered
    );
    for edge in &coverage.unresolved_control_flow {
        if edge.covered {
            outputln!(
                "COVERED-CONTROL-FLOW\t{side}\t{}\ttargets={}",
                edge.location,
                edge.targets.join(",")
            );
        } else {
            outputln!(
                "UNCOVERED-CONTROL-FLOW\t{side}\t{}\t{}",
                edge.location,
                edge.edge
            );
        }
    }
    for address in &coverage.unnamed_mmio {
        outputln!("UNNAMED-MMIO\t{side}\t{address:#010x}");
    }
}

pub fn print_execution_comparison(report: &ExecutionComparisonReport) {
    outputln!(
        "ORACLE\t{}\tsha256={}",
        report.vendor.path,
        report.vendor.sha256
    );
    if let Some(companion) = &report.vendor.companion {
        outputln!("ORACLE\t{}\tsha256={}", companion.path, companion.sha256);
    }
    for case in &report.cases {
        match case {
            CaseReport::Match {
                name,
                environment,
                events,
                memory_changes,
                return_compared,
            } => {
                print_table_environment(name, environment);
                outputln!(
                    "CASE\t{name}\tMATCH\tevents={events}\tmemory-changes={memory_changes}\treturn={}",
                    if *return_compared {
                        "checked"
                    } else {
                        "ignored"
                    }
                );
            }
            CaseReport::Incomplete {
                name,
                environment,
                vendor_error,
                rust_error,
            } => {
                print_table_environment(name, environment);
                outputln!(
                    "CASE\t{name}\tINCOMPLETE\tvendor={}\trust={}",
                    vendor_error.as_deref().unwrap_or("complete"),
                    rust_error.as_deref().unwrap_or("complete")
                );
            }
            CaseReport::Diff {
                name,
                environment,
                difference,
            } => {
                print_table_environment(name, environment);
                outputln!(
                    "CASE\t{name}\tDIFF\tkind={}\tfirst-difference={}",
                    difference.kind.label(),
                    difference.first_difference,
                );
                print_difference(name, difference);
            }
        }
    }
    if let Some(gap) = &report.coverage_gap {
        print_difference("coverage", gap);
    }
    print_coverage("vendor", &report.vendor_coverage);
    print_coverage("rust", &report.rust_coverage);
    let summary = &report.summary;
    outputln!(
        "SUMMARY\tcases={}\tmatched={}\tdifferent={}\tincomplete={}\tvendor-uncovered-branch-outcomes={}\trust-uncovered-branch-outcomes={}\tvendor-unresolved-control-flow={}\trust-unresolved-control-flow={}\tvendor-unnamed-mmio={}\trust-unnamed-mmio={}",
        summary.cases,
        summary.matched,
        summary.different,
        summary.incomplete,
        summary.vendor_uncovered_branch_outcomes,
        summary.rust_uncovered_branch_outcomes,
        summary.vendor_unresolved_control_flow,
        summary.rust_unresolved_control_flow,
        summary.vendor_unnamed_mmio,
        summary.rust_unnamed_mmio
    );
    outputln!(
        "VERDICT\tmode={}\t{}",
        report.mode.label(),
        report.verdict.label()
    );
}

fn print_table_environment(case: &str, environment: &ScenarioEnvironmentReport) {
    for (side, instances) in [
        ("vendor", &environment.vendor_tables),
        ("rust", &environment.rust_tables),
    ] {
        for instance in instances {
            outputln!(
                "TABLE-INSTANCE\tcase={case}\tside={side}\tlayout={}\tbase={:#010x}\tsize={:#x}\tpointer-cells={}\tslots={}",
                instance.layout_id,
                instance.base_address,
                instance.layout_size,
                instance.pointer_cells.len(),
                instance.slots.len(),
            );
        }
    }
    for device in &environment.device_models {
        outputln!(
            "DEVICE-MODEL\tcase={case}\tid={}\tkind={}\tstart={:#010x}\tlength={:#x}",
            device.id,
            device.kind,
            device.start,
            device.length,
        );
    }
    for (side, coverage) in [
        ("vendor", &environment.vendor_device_coverage),
        ("rust", &environment.rust_device_coverage),
    ] {
        for model in coverage {
            outputln!(
                "DEVICE-COVERAGE\tcase={case}\tside={side}\tid={}\tkind={}\tcomplete={}\treason={}",
                model.id,
                model.kind,
                model.complete,
                model.reason.as_deref().unwrap_or("-"),
            );
        }
    }
    for (side, instances) in [
        ("vendor", &environment.vendor_memory_instances),
        ("rust", &environment.rust_memory_instances),
    ] {
        for instance in instances {
            outputln!(
                "MEMORY-INSTANCE\tcase={case}\tside={side}\tid={}\tbase={:#010x}\tlength={:#x}\tbindings={}",
                instance.id,
                instance.base_address,
                instance.length,
                instance.bindings.len(),
            );
        }
    }
    for (side, events, complete) in [
        (
            "vendor",
            &environment.vendor_table_lifecycle,
            environment.vendor_table_lifecycle_complete,
        ),
        (
            "rust",
            &environment.rust_table_lifecycle,
            environment.rust_table_lifecycle_complete,
        ),
    ] {
        if let Some(complete) = complete {
            outputln!(
                "TABLE-LIFECYCLE\tcase={case}\tside={side}\tcomplete={complete}\tevents={}",
                events.len()
            );
        }
    }
}
