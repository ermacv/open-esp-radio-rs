//! Typed result model and human renderer for concrete execution comparison.

use serde::Serialize;

use crate::execution;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ComparisonVerdict {
    Match,
    Mismatch,
    Incomplete,
}

impl ComparisonVerdict {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Incomplete => "INCOMPLETE",
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ArtifactReport {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) companion: Option<ArtifactIdentity>,
    pub(crate) symbol: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ArtifactIdentity {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ExecutionEventReport {
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
                register,
                value,
            } => Self::Read {
                width: *width,
                address: *address,
                register: register.clone(),
                value: *value,
            },
            execution::ExecutionEvent::Write {
                width,
                address,
                register,
                value,
            } => Self::Write {
                width: *width,
                address: *address,
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

#[derive(Debug, Serialize)]
pub(crate) struct MemoryChangeReport {
    pub(crate) address: u32,
    pub(crate) before: u8,
    pub(crate) after: u8,
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

#[derive(Debug, Serialize)]
pub(crate) struct ExecutionOutcomeReport {
    pub(crate) events: Vec<ExecutionEventReport>,
    pub(crate) memory_changes: Vec<MemoryChangeReport>,
    pub(crate) return_value: u32,
}

impl From<&execution::ExecutionResult> for ExecutionOutcomeReport {
    fn from(result: &execution::ExecutionResult) -> Self {
        Self {
            events: result.events.iter().map(Into::into).collect(),
            memory_changes: result.memory_changes.iter().map(Into::into).collect(),
            return_value: result.return_value,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub(crate) enum CaseReport {
    Match {
        name: String,
        events: usize,
        memory_changes: usize,
        return_compared: bool,
    },
    Mismatch {
        name: String,
        vendor: ExecutionOutcomeReport,
        rust: ExecutionOutcomeReport,
    },
    Incomplete {
        name: String,
        vendor_error: Option<String>,
        rust_error: Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct BranchOutcomeReport {
    pub(crate) site: u32,
    pub(crate) location: String,
    pub(crate) taken: bool,
    pub(crate) covered: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct ControlFlowReport {
    pub(crate) site: u32,
    pub(crate) location: String,
    pub(crate) edge: String,
    pub(crate) targets: Vec<String>,
    pub(crate) covered: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct CoverageReport {
    pub(crate) covered_calls: Vec<String>,
    pub(crate) branch_outcomes: Vec<BranchOutcomeReport>,
    pub(crate) unresolved_control_flow: Vec<ControlFlowReport>,
    pub(crate) unmapped_mmio: Vec<u32>,
}

impl CoverageReport {
    pub(crate) fn uncovered_branch_outcomes(&self) -> usize {
        self.branch_outcomes
            .iter()
            .filter(|outcome| !outcome.covered)
            .count()
    }

    pub(crate) fn uncovered_control_flow(&self) -> usize {
        self.unresolved_control_flow
            .iter()
            .filter(|edge| !edge.covered)
            .count()
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ComparisonSummary {
    pub(crate) cases: usize,
    pub(crate) matched: usize,
    pub(crate) mismatched: usize,
    pub(crate) incomplete: usize,
    pub(crate) vendor_uncovered_branch_outcomes: usize,
    pub(crate) rust_uncovered_branch_outcomes: usize,
    pub(crate) vendor_unresolved_control_flow: usize,
    pub(crate) rust_unresolved_control_flow: usize,
    pub(crate) vendor_unmapped_mmio: usize,
    pub(crate) rust_unmapped_mmio: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ExecutionComparisonReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) vendor: ArtifactReport,
    pub(crate) rust: ArtifactReport,
    pub(crate) compare_return: bool,
    pub(crate) cases: Vec<CaseReport>,
    pub(crate) vendor_coverage: CoverageReport,
    pub(crate) rust_coverage: CoverageReport,
    pub(crate) summary: ComparisonSummary,
    pub(crate) verdict: ComparisonVerdict,
}

fn print_event(side: &str, index: usize, event: &ExecutionEventReport) {
    match event {
        ExecutionEventReport::Read {
            width,
            address,
            register,
            value,
        } => outputln!(
            "TRACE-EVENT\t{side}\t{index}\tR\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"
        ),
        ExecutionEventReport::Write {
            width,
            address,
            register,
            value,
        } => outputln!(
            "TRACE-EVENT\t{side}\t{index}\tW\t{width}\t{address:#010x}\t{register}\tvalue={value:#010x}"
        ),
        ExecutionEventReport::DelayMicros { micros } => {
            outputln!("TRACE-EVENT\t{side}\t{index}\tDELAY\tmicros={micros}");
        }
        ExecutionEventReport::Fence {
            fm,
            predecessor,
            successor,
        } => outputln!(
            "TRACE-EVENT\t{side}\t{index}\tFENCE\tfm={fm:#x}\tpred={predecessor:#x}\tsucc={successor:#x}"
        ),
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
    for address in &coverage.unmapped_mmio {
        outputln!("UNCOVERED-MMIO\t{side}\t{address:#010x}");
    }
}

pub(crate) fn print_execution_comparison(report: &ExecutionComparisonReport) {
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
                events,
                memory_changes,
                return_compared,
            } => outputln!(
                "CASE\t{name}\tMATCH\tevents={events}\tmemory-changes={memory_changes}\treturn={}",
                if *return_compared {
                    "checked"
                } else {
                    "ignored"
                }
            ),
            CaseReport::Incomplete {
                name,
                vendor_error,
                rust_error,
            } => outputln!(
                "CASE\t{name}\tINCOMPLETE\tvendor={}\trust={}",
                vendor_error.as_deref().unwrap_or("complete"),
                rust_error.as_deref().unwrap_or("complete")
            ),
            CaseReport::Mismatch { name, vendor, rust } => {
                outputln!(
                    "CASE\t{name}\tMISMATCH\tvendor-events={}\trust-events={}\tvendor-memory-changes={}\trust-memory-changes={}\tvendor-return={:#010x}\trust-return={:#010x}",
                    vendor.events.len(),
                    rust.events.len(),
                    vendor.memory_changes.len(),
                    rust.memory_changes.len(),
                    vendor.return_value,
                    rust.return_value
                );
                for (index, event) in vendor.events.iter().enumerate() {
                    print_event("vendor", index, event);
                }
                for (index, event) in rust.events.iter().enumerate() {
                    print_event("rust", index, event);
                }
                for change in &vendor.memory_changes {
                    outputln!(
                        "MEMORY-CHANGE\tvendor\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                        change.address,
                        change.before,
                        change.after
                    );
                }
                for change in &rust.memory_changes {
                    outputln!(
                        "MEMORY-CHANGE\trust\t{:#010x}\tbefore={:#04x}\tafter={:#04x}",
                        change.address,
                        change.before,
                        change.after
                    );
                }
            }
        }
    }
    print_coverage("vendor", &report.vendor_coverage);
    print_coverage("rust", &report.rust_coverage);
    let summary = &report.summary;
    outputln!(
        "SUMMARY\tcases={}\tmatched={}\tmismatched={}\tincomplete={}\tvendor-uncovered-branch-outcomes={}\trust-uncovered-branch-outcomes={}\tvendor-unresolved-control-flow={}\trust-unresolved-control-flow={}\tvendor-unmapped-mmio={}\trust-unmapped-mmio={}",
        summary.cases,
        summary.matched,
        summary.mismatched,
        summary.incomplete,
        summary.vendor_uncovered_branch_outcomes,
        summary.rust_uncovered_branch_outcomes,
        summary.vendor_unresolved_control_flow,
        summary.rust_unresolved_control_flow,
        summary.vendor_unmapped_mmio,
        summary.rust_unmapped_mmio
    );
    outputln!("VERDICT\t{}", report.verdict.label());
}
