//! Advisory scenario candidates recovered from structural evidence.
//!
//! These records are never coverage or equivalence proof. They only turn
//! already-recovered argument guards, MMIO predicates, and polls into concrete
//! starting points which still require execution by the fail-closed machine.

use super::*;

fn argument_assignment(index: u8, value: u32) -> ScenarioArgumentAssignment {
    ScenarioArgumentAssignment { index, value }
}

fn argument_branch(condition: &BranchCondition) -> Option<ScenarioSuggestion> {
    let (argument, expected) = condition
        .left
        .direct_input_index()
        .zip(condition.right.as_constant())
        .or_else(|| {
            condition
                .right
                .direct_input_index()
                .zip(condition.left.as_constant())
        })?;
    let differs = expected ^ 1;
    let (taken, not_taken) = match condition.operation {
        BranchOperation::Equal => (expected, differs),
        BranchOperation::NotEqual => (differs, expected),
        _ => return None,
    };
    Some(ScenarioSuggestion {
        kind: "argument-branch",
        site: Some(condition.site),
        evidence: format!(
            "{} {} {}",
            condition.left.canonical(),
            branch_operation(condition.operation),
            condition.right.canonical()
        ),
        variants: vec![
            ScenarioSuggestionVariant {
                name: "branch-taken",
                arguments: vec![argument_assignment(argument, taken)],
                mmio_reads: Vec::new(),
            },
            ScenarioSuggestionVariant {
                name: "branch-not-taken",
                arguments: vec![argument_assignment(argument, not_taken)],
                mmio_reads: Vec::new(),
            },
        ],
    })
}

fn collect_flow_argument_branches(
    flow: &DraftReferenceFlow,
    output: &mut BTreeSet<ScenarioSuggestion>,
) {
    for event in &flow.events {
        match event {
            DraftReferenceEvent::BoundedPoll { body, .. }
            | DraftReferenceEvent::PollFlow { body, .. } => {
                collect_flow_argument_branches(body, output);
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                sample,
                ..
            } => {
                collect_flow_argument_branches(initial_read, output);
                collect_flow_argument_branches(setup, output);
                collect_flow_argument_branches(sample, output);
            }
            _ => {}
        }
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        if let Some(suggestion) = argument_branch(condition) {
            output.insert(suggestion);
        }
        collect_flow_argument_branches(taken, output);
        collect_flow_argument_branches(not_taken, output);
    }
}

fn direct_mmio_suggestions(
    predicates: &[LinkedDirectMmioPredicate],
    output: &mut BTreeSet<ScenarioSuggestion>,
) {
    for predicate in predicates {
        if !matches!(predicate.operation, "equal" | "not-equal") {
            continue;
        }
        for source in &predicate.sources {
            let Some(expected) = source.register_comparison_value else {
                continue;
            };
            let mask = source.register_bits;
            if mask == 0 {
                continue;
            }
            let equal = expected & mask;
            let not_equal = equal ^ (1 << mask.trailing_zeros());
            output.insert(ScenarioSuggestion {
                kind: "mmio-predicate",
                site: Some(predicate.site),
                evidence: predicate.condition.clone(),
                variants: vec![
                    ScenarioSuggestionVariant {
                        name: "comparison-equal",
                        arguments: Vec::new(),
                        mmio_reads: vec![ScenarioMmioReadAssignment {
                            address: source.address,
                            mask,
                            expected: equal,
                            values: vec![equal],
                        }],
                    },
                    ScenarioSuggestionVariant {
                        name: "comparison-not-equal",
                        arguments: Vec::new(),
                        mmio_reads: vec![ScenarioMmioReadAssignment {
                            address: source.address,
                            mask,
                            expected: equal,
                            values: vec![not_equal],
                        }],
                    },
                ],
            });
        }
    }
}

fn poll_suggestions(accesses: &[LinkedMmioAccess], output: &mut BTreeSet<ScenarioSuggestion>) {
    for access in accesses.iter().filter(|access| access.access == "poll") {
        let (Some(mask), Some(expected)) = (access.predicate_mask, access.predicate_expected)
        else {
            continue;
        };
        if mask == 0 {
            continue;
        }
        let ready = expected & mask;
        let not_ready = ready ^ (1 << mask.trailing_zeros());
        output.insert(ScenarioSuggestion {
            kind: "mmio-poll",
            site: None,
            evidence: access.guard.clone().unwrap_or_else(|| access.path.clone()),
            variants: vec![
                ScenarioSuggestionVariant {
                    name: "ready-immediately",
                    arguments: Vec::new(),
                    mmio_reads: vec![ScenarioMmioReadAssignment {
                        address: access.address,
                        mask,
                        expected: ready,
                        values: vec![ready],
                    }],
                },
                ScenarioSuggestionVariant {
                    name: "one-retry-then-ready",
                    arguments: Vec::new(),
                    mmio_reads: vec![ScenarioMmioReadAssignment {
                        address: access.address,
                        mask,
                        expected: ready,
                        values: vec![not_ready, ready],
                    }],
                },
            ],
        });
    }
}

pub(super) fn scenario_suggestions(
    trace: Option<&FunctionAnalysis>,
    predicates: &[LinkedDirectMmioPredicate],
    accesses: &[LinkedMmioAccess],
) -> Vec<ScenarioSuggestion> {
    let mut output = BTreeSet::new();
    if let Some(flow) = trace.and_then(|trace| trace.reference_flow.as_ref()) {
        collect_flow_argument_branches(flow, &mut output);
    }
    direct_mmio_suggestions(predicates, &mut output);
    poll_suggestions(accesses, &mut output);
    output.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_equality_produces_both_replay_candidates() {
        let condition = BranchCondition {
            site: 0x1010,
            operation: BranchOperation::Equal,
            left: SymbolicValue::input(2),
            right: SymbolicValue::Constant(3),
        };
        let suggestion = argument_branch(&condition).unwrap();
        assert_eq!(suggestion.variants[0].arguments[0].value, 3);
        assert_eq!(suggestion.variants[1].arguments[0].value, 2);
    }

    #[test]
    fn poll_candidates_include_immediate_and_retried_reads() {
        let access = LinkedMmioAccess {
            ordinal: 0,
            address: 0x6000_0010,
            width: 32,
            register: "STATUS".to_owned(),
            access: "poll",
            mode: "static",
            path: "poll".to_owned(),
            address_expression: None,
            guard: Some("status & 4 == 4".to_owned()),
            predicate_mask: Some(4),
            predicate_expected: Some(4),
            value: None,
            modified_mask: None,
            preserved_mask: None,
            inverted_mask: None,
            forced_zero_mask: None,
            forced_one_mask: None,
            read_derived_mask: None,
            dynamic_mask: None,
        };
        let suggestions = scenario_suggestions(None, &[], &[access]);
        assert_eq!(suggestions[0].variants[0].mmio_reads[0].values, [4]);
        assert_eq!(suggestions[0].variants[1].mmio_reads[0].values, [0, 4]);
    }
}
