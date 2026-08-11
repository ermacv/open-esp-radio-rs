//! Typed first-difference construction shared by CLI, JSON and future UIs.

use crate::{execution, verification::execution_report::*};

const CONTEXT_ITEMS: usize = 3;

pub(super) fn trace_difference(
    vendor: &execution::ExecutionResult,
    rust: &execution::ExecutionResult,
    compare_return: bool,
) -> Option<TraceDiffReport> {
    if let Some(index) = first_difference(&vendor.events, &rust.events) {
        return Some(TraceDiffReport {
            first_difference: index,
            kind: DifferenceKind::Event,
            vendor: event_item(vendor, index),
            rust: event_item(rust, index),
            context_before: event_context(vendor, rust, context_before(index)),
            context_after: event_context(
                vendor,
                rust,
                index + 1..context_end(index, vendor.events.len(), rust.events.len()),
            ),
            path: Some(execution_path(vendor, rust)),
        });
    }
    if let Some(index) = first_difference(&vendor.memory_changes, &rust.memory_changes) {
        return Some(TraceDiffReport {
            first_difference: index,
            kind: DifferenceKind::Memory,
            vendor: vendor.memory_changes.get(index).map(memory_item),
            rust: rust.memory_changes.get(index).map(memory_item),
            context_before: memory_context(
                &vendor.memory_changes,
                &rust.memory_changes,
                context_before(index),
            ),
            context_after: memory_context(
                &vendor.memory_changes,
                &rust.memory_changes,
                index + 1
                    ..context_end(
                        index,
                        vendor.memory_changes.len(),
                        rust.memory_changes.len(),
                    ),
            ),
            path: Some(execution_path(vendor, rust)),
        });
    }
    (compare_return && vendor.return_value != rust.return_value).then(|| TraceDiffReport {
        first_difference: 0,
        kind: DifferenceKind::ReturnValue,
        vendor: Some(TraceItemReport::ReturnValue {
            value: vendor.return_value,
        }),
        rust: Some(TraceItemReport::ReturnValue {
            value: rust.return_value,
        }),
        context_before: Vec::new(),
        context_after: Vec::new(),
        path: Some(execution_path(vendor, rust)),
    })
}

pub(super) fn coverage_gap(
    vendor: &CoverageReport,
    rust: &CoverageReport,
) -> Option<TraceDiffReport> {
    let vendor = first_coverage_issue("vendor", vendor);
    let rust = first_coverage_issue("rust", rust);
    (vendor.is_some() || rust.is_some()).then(|| TraceDiffReport {
        first_difference: 0,
        kind: DifferenceKind::Coverage,
        vendor,
        rust,
        context_before: Vec::new(),
        context_after: Vec::new(),
        path: None,
    })
}

fn first_coverage_issue(side: &str, coverage: &CoverageReport) -> Option<TraceItemReport> {
    coverage
        .branch_outcomes
        .iter()
        .find(|outcome| !outcome.covered)
        .map(|outcome| TraceItemReport::Coverage {
            issue: format!(
                "{side}: uncovered branch {} taken={}",
                outcome.location, outcome.taken
            ),
        })
        .or_else(|| {
            coverage
                .unresolved_control_flow
                .iter()
                .find(|edge| !edge.covered)
                .map(|edge| TraceItemReport::Coverage {
                    issue: format!(
                        "{side}: unresolved control flow {}: {}",
                        edge.location, edge.edge
                    ),
                })
        })
}

fn first_difference<T: PartialEq>(vendor: &[T], rust: &[T]) -> Option<usize> {
    let shared = vendor.len().min(rust.len());
    vendor[..shared]
        .iter()
        .zip(&rust[..shared])
        .position(|(vendor, rust)| vendor != rust)
        .or_else(|| (vendor.len() != rust.len()).then_some(shared))
}

fn context_before(index: usize) -> std::ops::Range<usize> {
    index.saturating_sub(CONTEXT_ITEMS)..index
}

fn context_end(index: usize, vendor_len: usize, rust_len: usize) -> usize {
    (index + 1 + CONTEXT_ITEMS).min(vendor_len.max(rust_len))
}

fn event_item(result: &execution::ExecutionResult, index: usize) -> Option<TraceItemReport> {
    result
        .events
        .get(index)
        .map(|event| TraceItemReport::Event {
            event: event.into(),
            producer: result.event_producers.get(index).map(Into::into),
        })
}

fn memory_item(change: &execution::MemoryChange) -> TraceItemReport {
    TraceItemReport::Memory {
        change: change.into(),
    }
}

fn event_context(
    vendor: &execution::ExecutionResult,
    rust: &execution::ExecutionResult,
    indices: std::ops::Range<usize>,
) -> Vec<AlignedTraceItemReport> {
    indices
        .map(|index| AlignedTraceItemReport {
            index,
            vendor: event_item(vendor, index),
            rust: event_item(rust, index),
            equal: vendor.events.get(index) == rust.events.get(index),
        })
        .collect()
}

fn memory_context(
    vendor: &[execution::MemoryChange],
    rust: &[execution::MemoryChange],
    indices: std::ops::Range<usize>,
) -> Vec<AlignedTraceItemReport> {
    indices
        .map(|index| AlignedTraceItemReport {
            index,
            vendor: vendor.get(index).map(memory_item),
            rust: rust.get(index).map(memory_item),
            equal: vendor.get(index) == rust.get(index),
        })
        .collect()
}

fn execution_path(
    vendor: &execution::ExecutionResult,
    rust: &execution::ExecutionResult,
) -> ExecutionPathReport {
    ExecutionPathReport {
        vendor: execution_path_side(vendor),
        rust: execution_path_side(rust),
    }
}

fn execution_path_side(result: &execution::ExecutionResult) -> ExecutionPathSideReport {
    ExecutionPathSideReport {
        branches: result
            .ordered_branches
            .iter()
            .map(|(site, taken)| BranchDecisionReport {
                site: *site,
                taken: *taken,
            })
            .collect(),
        calls: result
            .ordered_calls
            .iter()
            .map(|call| OrderedCallReport {
                site: call.site,
                symbol: call.symbol.clone(),
                arguments: call.arguments,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    fn result(events: Vec<execution::ExecutionEvent>) -> execution::ExecutionResult {
        execution::ExecutionResult {
            event_producers: vec![
                execution::ExecutionProducer {
                    pc: 0x1000,
                    symbol: Some("fixture".to_owned()),
                    symbol_offset: Some(0),
                };
                events.len()
            ],
            events,
            timeline: Vec::new(),
            return_value: 0,
            steps: 0,
            branches: BTreeSet::new(),
            ordered_branches: vec![(0x1000, true)],
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            allocations: Vec::new(),
            table_lifecycle: Vec::new(),
            table_lifecycle_complete: true,
            device_model_coverage: Vec::new(),
            memory_changes: Vec::new(),
            initial_memory: BTreeMap::new(),
            persistent_memory: BTreeMap::new(),
        }
    }

    #[test]
    fn reports_first_event_difference_with_bounded_context_and_path() {
        let event = |value| execution::ExecutionEvent::Write {
            width: 32,
            address: 0x4000_0010,
            region: "radio".to_owned(),
            register: None,
            value,
        };
        let vendor = result(vec![event(1), event(2), event(3), event(4), event(5)]);
        let rust = result(vec![event(1), event(2), event(9), event(4), event(5)]);

        let difference = trace_difference(&vendor, &rust, false).unwrap();
        assert_eq!(difference.kind, DifferenceKind::Event);
        assert_eq!(difference.first_difference, 2);
        assert_eq!(difference.context_before.len(), 2);
        assert_eq!(difference.context_after.len(), 2);
        assert!(difference.context_before.iter().all(|item| item.equal));
        assert_eq!(difference.path.unwrap().vendor.branches[0].site, 0x1000);
        assert!(matches!(
            difference.vendor,
            Some(TraceItemReport::Event {
                producer: Some(EventProducerReport {
                    pc: 0x1000,
                    symbol_offset: Some(0),
                    ..
                }),
                ..
            })
        ));
    }

    #[test]
    fn length_difference_retains_the_missing_side() {
        let event = execution::ExecutionEvent::DelayMicros(1);
        let vendor = result(vec![event.clone(), event]);
        let rust = result(vec![execution::ExecutionEvent::DelayMicros(1)]);

        let difference = trace_difference(&vendor, &rust, false).unwrap();
        assert_eq!(difference.first_difference, 1);
        assert!(difference.vendor.is_some());
        assert!(difference.rust.is_none());
    }

    #[test]
    fn memory_difference_precedes_an_optional_return_difference() {
        let mut vendor = result(Vec::new());
        vendor.memory_changes.push(execution::MemoryChange {
            address: 0x2000,
            before: 0,
            after: 1,
        });
        vendor.return_value = 1;
        let mut rust = result(Vec::new());
        rust.memory_changes.push(execution::MemoryChange {
            address: 0x2000,
            before: 0,
            after: 2,
        });
        rust.return_value = 2;

        let difference = trace_difference(&vendor, &rust, true).unwrap();
        assert_eq!(difference.kind, DifferenceKind::Memory);
    }

    #[test]
    fn return_difference_is_ignored_unless_requested() {
        let mut vendor = result(Vec::new());
        vendor.return_value = 1;
        let mut rust = result(Vec::new());
        rust.return_value = 2;

        assert!(trace_difference(&vendor, &rust, false).is_none());
        assert_eq!(
            trace_difference(&vendor, &rust, true).unwrap().kind,
            DifferenceKind::ReturnValue
        );
    }

    #[test]
    fn coverage_gap_is_typed_as_incompleteness_not_an_event_difference() {
        let vendor = CoverageReport {
            covered_calls: Vec::new(),
            branch_outcomes: vec![BranchOutcomeReport {
                site: 0x1000,
                location: "vendor+0x0".to_owned(),
                taken: false,
                covered: false,
            }],
            unresolved_control_flow: Vec::new(),
            unnamed_mmio: Vec::new(),
        };
        let rust = CoverageReport {
            covered_calls: Vec::new(),
            branch_outcomes: Vec::new(),
            unresolved_control_flow: Vec::new(),
            unnamed_mmio: Vec::new(),
        };

        let gap = coverage_gap(&vendor, &rust).unwrap();
        assert_eq!(gap.kind, DifferenceKind::Coverage);
        assert!(gap.vendor.is_some());
        assert!(gap.rust.is_none());
        assert!(gap.path.is_none());
    }
}
