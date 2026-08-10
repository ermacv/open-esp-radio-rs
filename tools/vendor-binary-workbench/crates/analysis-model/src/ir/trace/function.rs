//! Per-function analysis results and eligibility queries.

use super::validation::{
    validate_reference_events_detailed, validate_reference_flow_with_calls_detailed,
};
use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedObservableEvent {
    pub site: u32,
    pub event: ObservableEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionAnalysis {
    pub symbol: String,
    pub events: Vec<ObservableEvent>,
    /// Instruction-local evidence for directly observed events. Reviewed or
    /// synthesized summaries may leave this empty because they do not claim a
    /// binary instruction site.
    pub located_events: Vec<LocatedObservableEvent>,
    pub reference_events: Vec<DraftReferenceEvent>,
    pub reference_dependencies: Vec<String>,
    pub blockers: Vec<String>,
    pub reference_blockers: Vec<String>,
    pub return_value: SymbolicValue,
    pub reference_flow: Option<DraftReferenceFlow>,
    pub unresolved_branch: Option<BranchCondition>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSymbolIdentity {
    pub member: Option<String>,
    pub name: String,
}

impl FunctionAnalysis {
    pub fn reference_failure_reasons(&self) -> Vec<String> {
        let mut reasons = self.blockers.clone();
        reasons.extend(self.reference_blockers.iter().cloned());
        reasons.extend(
            self.reference_unmapped_addresses()
                .into_iter()
                .map(|address| format!("unmapped-register {address:#010x}")),
        );
        if self.unresolved_branch.is_some() && reasons.is_empty() {
            reasons.push("unresolved symbolic branch".to_owned());
        }
        if reasons.is_empty() && !self.is_reference_eligible() {
            let validation_error = self.reference_flow.as_ref().map_or_else(
                || {
                    validate_reference_events_detailed(&self.reference_events, BTreeMap::new())
                        .err()
                },
                |flow| validate_reference_flow_with_calls_detailed(flow, BTreeMap::new()).err(),
            );
            reasons.push(validation_error.map_or_else(
                || "reference eligibility failed without a classified cause".to_owned(),
                |error| format!("reference-event-validation: {error}"),
            ));
        }
        reasons
    }

    pub fn reference_unmapped_addresses(&self) -> BTreeSet<u32> {
        fn collect_event(event: &DraftReferenceEvent, output: &mut BTreeSet<u32>) {
            match event {
                DraftReferenceEvent::Observable(event) => {
                    output.extend(event.unmapped_address());
                }
                DraftReferenceEvent::ComposedCall { flow, .. } => collect_flow(flow, output),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                    collect_flow(flow, output)
                }
                DraftReferenceEvent::BoundedPoll { body, .. } => collect_flow(body, output),
                DraftReferenceEvent::PollFlow { body, .. } => collect_flow(body, output),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    collect_flow(initial_read, output);
                    collect_flow(setup, output);
                    collect_flow(write_candidate, output);
                    collect_flow(sample, output);
                }
                _ => {}
            }
        }

        fn collect_flow(flow: &DraftReferenceFlow, output: &mut BTreeSet<u32>) {
            for event in &flow.events {
                collect_event(event, output);
            }
            if let DraftReferenceTerminator::Branch {
                taken, not_taken, ..
            } = &flow.terminator
            {
                collect_flow(taken, output);
                collect_flow(not_taken, output);
            }
        }

        let mut output = BTreeSet::new();
        if let Some(flow) = &self.reference_flow {
            collect_flow(flow, &mut output);
        } else {
            for event in &self.reference_events {
                collect_event(event, &mut output);
            }
        }
        output
    }

    pub fn reference_indexed_mmio_count(&self) -> usize {
        fn flow_count(flow: &DraftReferenceFlow) -> usize {
            let events = flow
                .events
                .iter()
                .map(|event| match event {
                    DraftReferenceEvent::IndexedMmio { .. }
                    | DraftReferenceEvent::PollMmio { .. } => 1,
                    DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                    DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_count(flow),
                    DraftReferenceEvent::BoundedPoll { body, .. } => flow_count(body),
                    DraftReferenceEvent::PollFlow { body, .. } => flow_count(body),
                    DraftReferenceEvent::SymmetricCalibrationSearch {
                        initial_read,
                        setup,
                        write_candidate,
                        sample,
                        ..
                    } => {
                        flow_count(initial_read)
                            + flow_count(setup)
                            + flow_count(write_candidate)
                            + flow_count(sample)
                    }
                    _ => 0,
                })
                .sum::<usize>();
            events
                + match &flow.terminator {
                    DraftReferenceTerminator::Return(_) => 0,
                    DraftReferenceTerminator::Branch {
                        taken, not_taken, ..
                    } => flow_count(taken) + flow_count(not_taken),
                }
        }

        self.reference_flow.as_ref().map_or_else(
            || {
                self.reference_events
                    .iter()
                    .map(|event| match event {
                        DraftReferenceEvent::IndexedMmio { .. }
                        | DraftReferenceEvent::PollMmio { .. } => 1,
                        DraftReferenceEvent::ComposedCall { flow, .. } => flow_count(flow),
                        DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                            flow_count(flow)
                        }
                        DraftReferenceEvent::BoundedPoll { body, .. } => flow_count(body),
                        DraftReferenceEvent::PollFlow { body, .. } => flow_count(body),
                        DraftReferenceEvent::SymmetricCalibrationSearch {
                            initial_read,
                            setup,
                            write_candidate,
                            sample,
                            ..
                        } => {
                            flow_count(initial_read)
                                + flow_count(setup)
                                + flow_count(write_candidate)
                                + flow_count(sample)
                        }
                        _ => 0,
                    })
                    .sum()
            },
            flow_count,
        )
    }

    pub fn is_exact(&self) -> bool {
        fn event_contains_reference_only_control_flow(event: &DraftReferenceEvent) -> bool {
            match event {
                DraftReferenceEvent::IndexedMmio { .. } | DraftReferenceEvent::PollMmio { .. } => {
                    true
                }
                DraftReferenceEvent::ComposedCall { flow, .. } => {
                    flow_contains_reference_only_control_flow(flow)
                }
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => {
                    flow_contains_reference_only_control_flow(flow)
                }
                DraftReferenceEvent::BoundedPoll { .. } => true,
                DraftReferenceEvent::PollFlow { .. } => true,
                DraftReferenceEvent::SymmetricCalibrationSearch { .. } => true,
                _ => false,
            }
        }

        fn flow_contains_reference_only_control_flow(flow: &DraftReferenceFlow) -> bool {
            flow.events
                .iter()
                .any(event_contains_reference_only_control_flow)
                || match &flow.terminator {
                    DraftReferenceTerminator::Return(_) => false,
                    DraftReferenceTerminator::Branch {
                        taken, not_taken, ..
                    } => {
                        flow_contains_reference_only_control_flow(taken)
                            || flow_contains_reference_only_control_flow(not_taken)
                    }
                }
        }

        self.reference_flow.is_none()
            && self.blockers.is_empty()
            && !self
                .reference_events
                .iter()
                .any(event_contains_reference_only_control_flow)
            && self
                .events
                .iter()
                .all(|event| event.unmapped_address().is_none())
    }

    pub fn is_reference_eligible(&self) -> bool {
        self.blockers.is_empty()
            && self.reference_blockers.is_empty()
            && self.unresolved_branch.is_none()
            && self.reference_observables_are_mapped()
            && self.reference_calls_are_valid()
    }

    pub fn reference_observables_are_mapped(&self) -> bool {
        fn flow_is_mapped(flow: &DraftReferenceFlow) -> bool {
            flow.events.iter().all(|event| match event {
                DraftReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                DraftReferenceEvent::IndexedMmio { registers, .. }
                | DraftReferenceEvent::PollMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::BoundedPoll { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::PollFlow { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    flow_is_mapped(initial_read)
                        && flow_is_mapped(setup)
                        && flow_is_mapped(write_candidate)
                        && flow_is_mapped(sample)
                }
                _ => true,
            }) && match &flow.terminator {
                DraftReferenceTerminator::Return(_) => true,
                DraftReferenceTerminator::Branch {
                    taken, not_taken, ..
                } => flow_is_mapped(taken) && flow_is_mapped(not_taken),
            }
        }

        fn reference_event_is_mapped(event: &DraftReferenceEvent) -> bool {
            match event {
                DraftReferenceEvent::Observable(event) => event.unmapped_address().is_none(),
                DraftReferenceEvent::IndexedMmio { registers, .. }
                | DraftReferenceEvent::PollMmio { registers, .. } => !registers.is_empty(),
                DraftReferenceEvent::ComposedCall { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::ComposedCallWithScratch { flow, .. } => flow_is_mapped(flow),
                DraftReferenceEvent::BoundedPoll { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::PollFlow { body, .. } => flow_is_mapped(body),
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    flow_is_mapped(initial_read)
                        && flow_is_mapped(setup)
                        && flow_is_mapped(write_candidate)
                        && flow_is_mapped(sample)
                }
                _ => true,
            }
        }

        self.reference_flow.as_ref().map_or_else(
            || self.reference_events.iter().all(reference_event_is_mapped),
            flow_is_mapped,
        )
    }

    pub fn reference_calls_are_valid(&self) -> bool {
        if let Some(flow) = &self.reference_flow {
            reference_flow_calls_are_valid(flow)
        } else {
            validate_reference_events(&self.reference_events, BTreeMap::new()).is_some()
        }
    }

    pub fn reference_exit_return_modeled(&self) -> bool {
        self.reference_flow.as_ref().map_or_else(
            || {
                validate_reference_events(&self.reference_events, BTreeMap::new()).is_some_and(
                    |available| {
                        self.return_value.is_resolved()
                            && value_call_results_available(&self.return_value, &available)
                    },
                )
            },
            reference_flow_exit_modeled,
        )
    }
}
