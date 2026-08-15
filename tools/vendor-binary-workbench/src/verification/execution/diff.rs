//! Typed first-difference construction shared by CLI, JSON and future UIs.

use crate::{execution, verification::execution_report::*};

const CONTEXT_ITEMS: usize = 3;

pub(super) fn trace_difference(
    vendor: &execution::ExecutionResult,
    rust: &execution::ExecutionResult,
    compare_return: bool,
    transaction_comparison: super::super::profiles::TransactionComparison,
    call_equivalences: &[super::super::profiles::CallEquivalence],
) -> Option<TraceDiffReport> {
    if transaction_comparison.includes_calls() {
        let vendor_transactions =
            ordered_transactions(vendor, transaction_comparison, compare_return);
        let rust_transactions = ordered_transactions(rust, transaction_comparison, compare_return);
        let vendor_transactions = relevant_transactions(
            vendor_transactions,
            TransactionSide::Vendor,
            transaction_comparison,
            call_equivalences,
        );
        let rust_transactions = relevant_transactions(
            rust_transactions,
            TransactionSide::Rust,
            transaction_comparison,
            call_equivalences,
        );
        let vendor_keys = comparable_transactions(
            &vendor_transactions,
            TransactionSide::Vendor,
            call_equivalences,
        );
        let rust_keys =
            comparable_transactions(&rust_transactions, TransactionSide::Rust, call_equivalences);
        if let Some(index) = first_difference(&vendor_keys, &rust_keys) {
            return Some(TraceDiffReport {
                first_difference: index,
                kind: DifferenceKind::Transaction,
                vendor: vendor_transactions
                    .get(index)
                    .cloned()
                    .map(|transaction| TraceItemReport::Transaction { transaction }),
                rust: rust_transactions
                    .get(index)
                    .cloned()
                    .map(|transaction| TraceItemReport::Transaction { transaction }),
                context_before: transaction_context(
                    &vendor_transactions,
                    &rust_transactions,
                    context_before(index),
                    call_equivalences,
                ),
                context_after: transaction_context(
                    &vendor_transactions,
                    &rust_transactions,
                    index + 1
                        ..context_end(index, vendor_transactions.len(), rust_transactions.len()),
                    call_equivalences,
                ),
                path: Some(execution_path(vendor, rust)),
            });
        }
    }
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

pub(super) fn ordered_transactions(
    result: &execution::ExecutionResult,
    comparison: super::super::profiles::TransactionComparison,
    compare_return: bool,
) -> Vec<OrderedTransactionReport> {
    let mut output = result
        .timeline
        .iter()
        .filter_map(|event| match event {
            execution::ExecutionTimelineEvent::Observable(event) => {
                Some(OrderedTransactionReport::Observable {
                    event: event.into(),
                })
            }
            execution::ExecutionTimelineEvent::Call(call) if comparison.includes_calls() => {
                Some(OrderedTransactionReport::Call {
                    site: call.site,
                    symbol: call.symbol.clone(),
                    arguments: call.arguments,
                })
            }
            execution::ExecutionTimelineEvent::Branch { site, taken }
                if comparison.includes_internal_state() =>
            {
                Some(OrderedTransactionReport::Branch {
                    site: *site,
                    taken: *taken,
                })
            }
            execution::ExecutionTimelineEvent::RamRead {
                site,
                width,
                address,
                value,
            } if comparison.includes_internal_state() => Some(OrderedTransactionReport::RamRead {
                site: *site,
                width: *width,
                address: *address,
                value: *value,
            }),
            execution::ExecutionTimelineEvent::RamWrite {
                site,
                width,
                address,
                value,
            } if comparison.includes_internal_state() => Some(OrderedTransactionReport::RamWrite {
                site: *site,
                width: *width,
                address: *address,
                value: *value,
            }),
            execution::ExecutionTimelineEvent::Atomic {
                operation,
                ordering,
                address,
                succeeded,
            } if comparison.includes_internal_state() => Some(OrderedTransactionReport::Atomic {
                operation: format!("{operation:?}"),
                ordering: format!("{ordering:?}"),
                address: *address,
                succeeded: *succeeded,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    if compare_return {
        output.push(OrderedTransactionReport::Return {
            value: result.return_value,
        });
    }
    output
}

#[derive(Clone, Copy)]
enum TransactionSide {
    Vendor,
    Rust,
}

fn relevant_transactions(
    transactions: Vec<OrderedTransactionReport>,
    side: TransactionSide,
    comparison: super::super::profiles::TransactionComparison,
    call_equivalences: &[super::super::profiles::CallEquivalence],
) -> Vec<OrderedTransactionReport> {
    if !comparison.reviewed_calls_only() {
        return transactions;
    }
    transactions
        .into_iter()
        .filter(|transaction| match transaction {
            OrderedTransactionReport::Call { symbol, .. } => call_equivalences.iter().any(|pair| {
                (match side {
                    TransactionSide::Vendor => &pair.vendor_symbol,
                    TransactionSide::Rust => &pair.rust_symbol,
                }) == symbol
            }),
            _ => true,
        })
        .collect()
}

fn comparable_transactions(
    transactions: &[OrderedTransactionReport],
    side: TransactionSide,
    call_equivalences: &[super::super::profiles::CallEquivalence],
) -> Vec<OrderedTransactionReport> {
    transactions
        .iter()
        .cloned()
        .map(|transaction| match transaction {
            OrderedTransactionReport::Call {
                mut symbol,
                arguments,
                ..
            } => {
                if let Some(pair) = call_equivalences.iter().find(|pair| {
                    (match side {
                        TransactionSide::Vendor => &pair.vendor_symbol,
                        TransactionSide::Rust => &pair.rust_symbol,
                    }) == &symbol
                }) {
                    symbol.clone_from(&pair.operation);
                    let arguments = match pair.argument_comparison {
                        super::super::profiles::CallArgumentComparison::Exact => arguments,
                        super::super::profiles::CallArgumentComparison::Ignore => [0; 8],
                        super::super::profiles::CallArgumentComparison::Selected => {
                            let mut selected = [0; 8];
                            for index in &pair.argument_indices {
                                selected[usize::from(*index)] = arguments[usize::from(*index)];
                            }
                            selected
                        }
                    };
                    return OrderedTransactionReport::Call {
                        site: 0,
                        symbol,
                        arguments,
                    };
                }
                OrderedTransactionReport::Call {
                    site: 0,
                    symbol,
                    arguments,
                }
            }
            OrderedTransactionReport::Branch { taken, .. } => {
                OrderedTransactionReport::Branch { site: 0, taken }
            }
            OrderedTransactionReport::RamRead {
                width,
                address,
                value,
                ..
            } => OrderedTransactionReport::RamRead {
                site: 0,
                width,
                address,
                value,
            },
            OrderedTransactionReport::RamWrite {
                width,
                address,
                value,
                ..
            } => OrderedTransactionReport::RamWrite {
                site: 0,
                width,
                address,
                value,
            },
            other => other,
        })
        .collect()
}

pub(super) fn ordered_transactions_equal(
    vendor: &execution::ExecutionResult,
    rust: &execution::ExecutionResult,
    comparison: super::super::profiles::TransactionComparison,
    compare_return: bool,
    call_equivalences: &[super::super::profiles::CallEquivalence],
) -> bool {
    let vendor = relevant_transactions(
        ordered_transactions(vendor, comparison, compare_return),
        TransactionSide::Vendor,
        comparison,
        call_equivalences,
    );
    let rust = relevant_transactions(
        ordered_transactions(rust, comparison, compare_return),
        TransactionSide::Rust,
        comparison,
        call_equivalences,
    );
    comparable_transactions(&vendor, TransactionSide::Vendor, call_equivalences)
        == comparable_transactions(&rust, TransactionSide::Rust, call_equivalences)
}

fn transaction_context(
    vendor: &[OrderedTransactionReport],
    rust: &[OrderedTransactionReport],
    indices: std::ops::Range<usize>,
    call_equivalences: &[super::super::profiles::CallEquivalence],
) -> Vec<AlignedTraceItemReport> {
    let vendor_keys = comparable_transactions(vendor, TransactionSide::Vendor, call_equivalences);
    let rust_keys = comparable_transactions(rust, TransactionSide::Rust, call_equivalences);
    indices
        .map(|index| AlignedTraceItemReport {
            index,
            vendor: vendor
                .get(index)
                .cloned()
                .map(|transaction| TraceItemReport::Transaction { transaction }),
            rust: rust
                .get(index)
                .cloned()
                .map(|transaction| TraceItemReport::Transaction { transaction }),
            equal: vendor_keys.get(index) == rust_keys.get(index),
        })
        .collect()
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
            completion: execution::ExecutionCompletion::Returned,
            steps: 0,
            executed_pcs: BTreeSet::new(),
            branches: BTreeSet::new(),
            ordered_branches: vec![(0x1000, true)],
            calls: BTreeSet::new(),
            ordered_calls: Vec::new(),
            indirect_calls: BTreeSet::new(),
            allocations: Vec::new(),
            table_lifecycle: Vec::new(),
            table_lifecycle_complete: true,
            fifo_lifecycle: Vec::new(),
            fifo_services: Vec::new(),
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

        let difference = trace_difference(
            &vendor,
            &rust,
            false,
            super::super::super::profiles::TransactionComparison::Observables,
            &[],
        )
        .unwrap();
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
    fn ordered_call_comparison_detects_reordering_hidden_by_observable_trace() {
        let write = execution::ExecutionEvent::Write {
            width: 32,
            address: 0x4000_0010,
            region: "radio".to_owned(),
            register: None,
            value: 1,
        };
        let call = |site, symbol: &str| {
            execution::ExecutionTimelineEvent::Call(execution::OrderedCall {
                site,
                symbol: symbol.to_owned(),
                arguments: [0; 8],
            })
        };
        let mut vendor = result(vec![write.clone()]);
        vendor.timeline = vec![
            call(0x1000, "prepare"),
            execution::ExecutionTimelineEvent::Observable(write.clone()),
            call(0x1004, "publish"),
        ];
        let mut rust = result(vec![write.clone()]);
        rust.timeline = vec![
            call(0x2000, "publish"),
            execution::ExecutionTimelineEvent::Observable(write),
            call(0x2004, "prepare"),
        ];

        assert!(ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::Observables,
            false,
            &[],
        ));
        assert!(!ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::ObservablesAndCalls,
            false,
            &[],
        ));
        let difference = trace_difference(
            &vendor,
            &rust,
            false,
            super::super::super::profiles::TransactionComparison::ObservablesAndCalls,
            &[],
        )
        .unwrap();
        assert_eq!(difference.kind, DifferenceKind::Transaction);
        assert_eq!(difference.first_difference, 0);
        assert!(matches!(
            difference.vendor,
            Some(TraceItemReport::Transaction {
                transaction: OrderedTransactionReport::Call { ref symbol, .. }
            }) if symbol == "prepare"
        ));
    }

    #[test]
    fn transaction_comparison_treats_site_addresses_as_provenance() {
        let mut vendor = result(Vec::new());
        vendor.timeline = vec![execution::ExecutionTimelineEvent::Call(
            execution::OrderedCall {
                site: 0x1000,
                symbol: "same_semantic_boundary".to_owned(),
                arguments: [7; 8],
            },
        )];
        let mut rust = result(Vec::new());
        rust.timeline = vec![execution::ExecutionTimelineEvent::Call(
            execution::OrderedCall {
                site: 0x8000,
                symbol: "same_semantic_boundary".to_owned(),
                arguments: [7; 8],
            },
        )];

        assert!(ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::ObservablesAndCalls,
            false,
            &[],
        ));
    }

    #[test]
    fn reviewed_call_mapping_compares_semantics_and_ignores_unlisted_calls() {
        let call = |site, symbol: &str, arguments| {
            execution::ExecutionTimelineEvent::Call(execution::OrderedCall {
                site,
                symbol: symbol.to_owned(),
                arguments,
            })
        };
        let mut vendor = result(Vec::new());
        vendor.timeline = vec![
            call(0x1000, "wifi_assert", [1; 8]),
            call(0x1004, "lmacProcessAckTimeout", [2; 8]),
        ];
        let mut rust = result(Vec::new());
        rust.timeline = vec![
            call(0x2000, "inlined_helper_detail", [3; 8]),
            call(0x2004, "open_libpp_tx_retry_ack_timeout", [4; 8]),
        ];
        let mappings = [super::super::super::profiles::CallEquivalence {
            operation: "tx.retry.ack-timeout".to_owned(),
            vendor_symbol: "lmacProcessAckTimeout".to_owned(),
            rust_symbol: "open_libpp_tx_retry_ack_timeout".to_owned(),
            argument_comparison: super::super::super::profiles::CallArgumentComparison::Ignore,
            argument_indices: Vec::new(),
        }];

        assert!(ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::ObservablesAndReviewedCalls,
            false,
            &mappings,
        ));
    }

    #[test]
    fn reviewed_call_mapping_can_compare_only_reviewed_abi_positions() {
        let call = |symbol: &str, arguments| {
            execution::ExecutionTimelineEvent::Call(execution::OrderedCall {
                site: 0,
                symbol: symbol.to_owned(),
                arguments,
            })
        };
        let mut vendor = result(Vec::new());
        vendor.timeline = vec![call("vendor_leaf", [7, 11, 13, 0, 0, 0, 0, 0])];
        let mut rust = result(Vec::new());
        rust.timeline = vec![call("rust_leaf", [7, 99, 101, 0, 0, 0, 0, 0])];
        let mappings = [super::super::super::profiles::CallEquivalence {
            operation: "semantic-leaf".to_owned(),
            vendor_symbol: "vendor_leaf".to_owned(),
            rust_symbol: "rust_leaf".to_owned(),
            argument_comparison: super::super::super::profiles::CallArgumentComparison::Selected,
            argument_indices: vec![0],
        }];

        assert!(ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::ObservablesAndReviewedCalls,
            false,
            &mappings,
        ));
        rust.timeline = vec![call("rust_leaf", [8, 11, 13, 0, 0, 0, 0, 0])];
        assert!(!ordered_transactions_equal(
            &vendor,
            &rust,
            super::super::super::profiles::TransactionComparison::ObservablesAndReviewedCalls,
            false,
            &mappings,
        ));
    }

    #[test]
    fn length_difference_retains_the_missing_side() {
        let event = execution::ExecutionEvent::DelayMicros(1);
        let vendor = result(vec![event.clone(), event]);
        let rust = result(vec![execution::ExecutionEvent::DelayMicros(1)]);

        let difference = trace_difference(
            &vendor,
            &rust,
            false,
            super::super::super::profiles::TransactionComparison::Observables,
            &[],
        )
        .unwrap();
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

        let difference = trace_difference(
            &vendor,
            &rust,
            true,
            super::super::super::profiles::TransactionComparison::Observables,
            &[],
        )
        .unwrap();
        assert_eq!(difference.kind, DifferenceKind::Memory);
    }

    #[test]
    fn return_difference_is_ignored_unless_requested() {
        let mut vendor = result(Vec::new());
        vendor.return_value = 1;
        let mut rust = result(Vec::new());
        rust.return_value = 2;

        assert!(
            trace_difference(
                &vendor,
                &rust,
                false,
                super::super::super::profiles::TransactionComparison::Observables,
                &[],
            )
            .is_none()
        );
        assert_eq!(
            trace_difference(
                &vendor,
                &rust,
                true,
                super::super::super::profiles::TransactionComparison::Observables,
                &[],
            )
            .unwrap()
            .kind,
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
