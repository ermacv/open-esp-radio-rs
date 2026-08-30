//! Typed, path-guarded return alternatives for structured reference flow.

use super::*;

const MAX_RETURN_LEAVES: usize = 64;
const MAX_RETURN_GUARDS: usize = 32;

#[derive(Clone, Default)]
struct FrontierEvidence {
    next_memory_read_token: u32,
    memory_read_sources: BTreeMap<u32, open_radio_vendor_analysis_model::MemoryObjectLocation>,
    call_results: BTreeMap<u32, String>,
    modeled_results: BTreeSet<(&'static str, u32)>,
}

impl FrontierEvidence {
    fn observe_flow_prefix(&mut self, flow: &DraftReferenceFlow, identities: &IrIdentityCatalog) {
        for event in &flow.events {
            if let DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                address,
                ..
            } = event
            {
                let token = self.next_memory_read_token;
                self.next_memory_read_token = self.next_memory_read_token.wrapping_add(1);
                if let Some(location) =
                    address.memory_object_location_with_reads(&self.memory_read_sources)
                {
                    self.memory_read_sources.insert(token, location);
                } else {
                    self.memory_read_sources.remove(&token);
                }
            }
            if let Some((token, target)) = call_result_identity(event, identities) {
                self.call_results.insert(token, target);
            }
            let result_token = match event {
                DraftReferenceEvent::ModeledDirectCall { token, .. }
                | DraftReferenceEvent::ReviewedExternalCall { token, .. }
                | DraftReferenceEvent::Call { token, .. }
                | DraftReferenceEvent::ScratchCall { token, .. }
                | DraftReferenceEvent::TailCall { token, .. }
                | DraftReferenceEvent::ComposedCall { token, .. }
                | DraftReferenceEvent::ComposedCallWithScratch { token, .. } => Some(*token),
                _ => None,
            };
            if let Some(token) = result_token {
                self.modeled_results.remove(&("call-result", token));
                self.modeled_results.remove(&("external-result", token));
            }
            if let Some(((kind, token), _)) = call_result_origin(event, "frontier", identities) {
                self.modeled_results.insert((kind, token));
            }
        }
    }

    fn value_is_exact(&self, value: &SymbolicValue) -> bool {
        value.is_resolved()
            && value.tree().all(|value| match value {
                SymbolicValue::CallResult(token) => {
                    self.modeled_results.contains(&("call-result", *token))
                }
                SymbolicValue::ExternalResult(token) | SymbolicValue::ExternalResultHigh(token) => {
                    self.modeled_results
                        .contains(&("external-result", external_result_call_token(*token)))
                }
                SymbolicValue::ExternalOutput { .. } => false,
                SymbolicValue::Bits(bits) => bits.iter().all(|source| match source {
                    BitSource::CallResult { call_token, .. } => {
                        self.modeled_results.contains(&("call-result", *call_token))
                    }
                    BitSource::ExternalResult { call_token, .. }
                    | BitSource::ExternalResultHigh { call_token, .. } => self
                        .modeled_results
                        .contains(&("external-result", external_result_call_token(*call_token))),
                    BitSource::ExternalOutput { .. } => false,
                    _ => true,
                }),
                _ => true,
            })
    }
}

fn value_has_epoch_sensitive_dependency(value: &SymbolicValue) -> bool {
    value.tree().any(|value| match value {
        SymbolicValue::RegisterImage { and_mask, .. }
        | SymbolicValue::IndexedRegisterImage { and_mask, .. }
        | SymbolicValue::MemoryImage { and_mask, .. } => *and_mask != 0,
        SymbolicValue::Bits(bits) => bits.iter().any(|source| {
            matches!(
                source,
                BitSource::Register { .. }
                    | BitSource::IndexedRegister { .. }
                    | BitSource::Memory { .. }
            )
        }),
        _ => false,
    })
}

fn canonical_value(
    value: &SymbolicValue,
    evidence: &FrontierEvidence,
    resolver: &ReferenceResolver,
) -> String {
    let value = name_call_results(&value.canonical(), &evidence.call_results);
    name_memory_reads(&value, &evidence.memory_read_sources, resolver)
}

pub(super) fn guarded_return_frontier(
    flow: &DraftReferenceFlow,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioMap,
) -> LinkedGuardedReturnFrontier {
    let mut frontier = LinkedGuardedReturnFrontier {
        structurally_complete: true,
        leaves: Vec::new(),
        fail_stops: Vec::new(),
        blockers: Vec::new(),
    };

    fn walk(
        flow: &DraftReferenceFlow,
        guards: &mut Vec<LinkedReturnGuard>,
        mut evidence: FrontierEvidence,
        identities: &IrIdentityCatalog,
        resolver: &ReferenceResolver,
        svd: &MmioMap,
        frontier: &mut LinkedGuardedReturnFrontier,
    ) {
        if frontier.leaves.len() + frontier.fail_stops.len() >= MAX_RETURN_LEAVES {
            frontier.structurally_complete = false;
            if frontier.blockers.is_empty() {
                frontier.blockers.push(format!(
                    "guarded return frontier exceeds {MAX_RETURN_LEAVES} terminal paths"
                ));
            }
            return;
        }
        evidence.observe_flow_prefix(flow, identities);
        match &flow.terminator {
            DraftReferenceTerminator::Return(value) => {
                let provenance = return_provenance(value, &evidence.call_results, svd);
                let epoch_sensitive_dependency = value_has_epoch_sensitive_dependency(value);
                frontier.leaves.push(LinkedGuardedReturnLeaf {
                    value: canonical_value(value, &evidence, resolver),
                    exact: evidence.value_is_exact(value),
                    epoch_sensitive_dependency,
                    provenance,
                    guard_path: LinkedReturnGuardPath {
                        guards: guards.clone(),
                    },
                });
            }
            DraftReferenceTerminator::FailStop { site, function, .. } => {
                frontier.fail_stops.push(LinkedGuardedFailStop {
                    site: *site,
                    function: function.clone(),
                    guard_path: LinkedReturnGuardPath {
                        guards: guards.clone(),
                    },
                });
            }
            DraftReferenceTerminator::Branch {
                condition,
                taken,
                not_taken,
            } => {
                if guards.len() >= MAX_RETURN_GUARDS {
                    frontier.structurally_complete = false;
                    if frontier.blockers.is_empty() {
                        frontier.blockers.push(format!(
                            "guarded return path exceeds {MAX_RETURN_GUARDS} branch predicates"
                        ));
                    }
                    return;
                }
                let guard = |taken| LinkedReturnGuard {
                    site: condition.site,
                    operation: branch_operation(condition.operation),
                    taken,
                    left: canonical_value(&condition.left, &evidence, resolver),
                    right: canonical_value(&condition.right, &evidence, resolver),
                    left_exact: evidence.value_is_exact(&condition.left),
                    right_exact: evidence.value_is_exact(&condition.right),
                };
                guards.push(guard(true));
                walk(
                    taken,
                    guards,
                    evidence.clone(),
                    identities,
                    resolver,
                    svd,
                    frontier,
                );
                guards.pop();
                guards.push(guard(false));
                walk(
                    not_taken, guards, evidence, identities, resolver, svd, frontier,
                );
                guards.pop();
            }
        }
    }

    walk(
        flow,
        &mut Vec::new(),
        FrontierEvidence::default(),
        identities,
        resolver,
        svd,
        &mut frontier,
    );
    frontier.leaves.sort();
    frontier.leaves.dedup();
    frontier.fail_stops.sort();
    frontier.fail_stops.dedup();
    frontier.blockers.sort();
    frontier.blockers.dedup();
    frontier
}

fn call_event_site_and_arguments(event: &DraftReferenceEvent) -> Option<(u32, &[SymbolicValue])> {
    match event {
        DraftReferenceEvent::ModeledDirectCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::ReviewedExternalCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::DiagnosticCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::TailCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::Call {
            site, arguments, ..
        }
        | DraftReferenceEvent::ComposedCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::ScratchCall {
            site, arguments, ..
        }
        | DraftReferenceEvent::ComposedCallWithScratch {
            site, arguments, ..
        } => Some((*site, arguments)),
        _ => None,
    }
}

pub(super) fn publish_guarded_call_result_frontiers(
    trace: &FunctionAnalysis,
    owner: &str,
    calls: &mut [LinkedCall],
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioMap,
) -> Vec<LinkedCallResultFrontier> {
    // Internal call-result proof is published only together with its guarded
    // frontier. Remove any preliminary direct-trace association first so a
    // missing, conflicting, or over-limit specialization cannot survive.
    for call in calls.iter_mut() {
        if call.kind == "internal" {
            call.result_modeled = false;
            if call
                .result_provenance
                .as_ref()
                .is_some_and(|producer| producer.kind == "call-result")
            {
                call.result_provenance = None;
            }
        }
        call.argument_result_provenance
            .retain(|association| association.producer.kind != "call-result");
    }
    let Some(flow) = trace.reference_flow.as_ref() else {
        return Vec::new();
    };
    let mut frontier_candidates =
        BTreeMap::<LinkedCallResultProvenance, BTreeSet<LinkedGuardedReturnFrontier>>::new();

    struct ProducerContext<'a> {
        owner: &'a str,
        calls: &'a [LinkedCall],
        resolver: &'a ReferenceResolver,
        identities: &'a IrIdentityCatalog,
        svd: &'a MmioMap,
    }

    fn visit_event(
        event: &DraftReferenceEvent,
        context: &ProducerContext<'_>,
        frontier_candidates: &mut BTreeMap<
            LinkedCallResultProvenance,
            BTreeSet<LinkedGuardedReturnFrontier>,
        >,
    ) {
        match event {
            DraftReferenceEvent::ComposedCall {
                site,
                flow,
                result_modeled,
                ..
            }
            | DraftReferenceEvent::ComposedCallWithScratch {
                site,
                flow,
                result_modeled,
                ..
            } => {
                let matching = context
                    .calls
                    .iter()
                    .filter(|call| call.site == Some(*site) && call.kind == "internal")
                    .collect::<Vec<_>>();
                if matching.len() != 1 || !result_modeled {
                    return;
                }
                let call = matching[0];
                let frontier = guarded_return_frontier(
                    flow,
                    context.resolver,
                    context.identities,
                    context.svd,
                );
                if frontier.leaves.is_empty() {
                    return;
                }
                let producer = LinkedCallResultProvenance {
                    kind: "call-result",
                    function: context.owner.to_owned(),
                    site: *site,
                    target: call.target.clone(),
                    operation: call.semantic_operation.clone(),
                };
                frontier_candidates
                    .entry(producer)
                    .or_default()
                    .insert(frontier);
            }
            DraftReferenceEvent::BoundedPoll {
                body, on_exhausted, ..
            } => {
                visit_producers(body, context, frontier_candidates);
                if let Some(event) = on_exhausted.as_deref() {
                    visit_event(event, context, frontier_candidates);
                }
            }
            DraftReferenceEvent::PollFlow { body, .. } => {
                visit_producers(body, context, frontier_candidates)
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                initial_read,
                setup,
                write_candidate,
                sample,
                ..
            } => {
                for flow in [initial_read, setup, write_candidate, sample] {
                    visit_producers(flow, context, frontier_candidates);
                }
            }
            _ => {}
        }
    }

    fn visit_producers(
        flow: &DraftReferenceFlow,
        context: &ProducerContext<'_>,
        frontier_candidates: &mut BTreeMap<
            LinkedCallResultProvenance,
            BTreeSet<LinkedGuardedReturnFrontier>,
        >,
    ) {
        for event in &flow.events {
            visit_event(event, context, frontier_candidates);
        }
        if let DraftReferenceTerminator::Branch {
            taken, not_taken, ..
        } = &flow.terminator
        {
            visit_producers(taken, context, frontier_candidates);
            visit_producers(not_taken, context, frontier_candidates);
        }
    }

    let context = ProducerContext {
        owner,
        calls,
        resolver,
        identities,
        svd,
    };
    visit_producers(flow, &context, &mut frontier_candidates);
    let frontiers = frontier_candidates
        .into_iter()
        .filter_map(|(producer, mut candidates)| {
            (candidates.len() == 1)
                .then(|| (producer, candidates.pop_first().expect("one frontier")))
        })
        .collect::<BTreeMap<_, _>>();
    for producer in frontiers.keys() {
        let mut matching = calls.iter_mut().filter(|call| {
            call.kind == "internal"
                && call.site == Some(producer.site)
                && call.target == producer.target
                && call.semantic_operation == producer.operation
        });
        if let Some(call) = matching.next()
            && matching.next().is_none()
        {
            call.result_modeled = true;
            call.result_provenance = Some(producer.clone());
        }
    }
    fn collect_consumers(
        flow: &DraftReferenceFlow,
        frontiers: &BTreeMap<LinkedCallResultProvenance, LinkedGuardedReturnFrontier>,
        active: &mut BTreeMap<u32, LinkedCallResultProvenance>,
        observations: &mut BTreeMap<(u32, usize), BTreeSet<Option<LinkedCallResultProvenance>>>,
    ) {
        fn collect_event(
            event: &DraftReferenceEvent,
            frontiers: &BTreeMap<LinkedCallResultProvenance, LinkedGuardedReturnFrontier>,
            active: &mut BTreeMap<u32, LinkedCallResultProvenance>,
            observations: &mut BTreeMap<(u32, usize), BTreeSet<Option<LinkedCallResultProvenance>>>,
        ) {
            if let Some((site, arguments)) = call_event_site_and_arguments(event) {
                for (position, value) in arguments.iter().enumerate() {
                    let producer = match value {
                        SymbolicValue::CallResult(token) => active.get(token).cloned(),
                        _ => None,
                    };
                    observations
                        .entry((site, position))
                        .or_default()
                        .insert(producer);
                }
            }
            match event {
                DraftReferenceEvent::BoundedPoll {
                    body, on_exhausted, ..
                } => {
                    collect_consumers(body, frontiers, &mut active.clone(), observations);
                    if let Some(event) = on_exhausted.as_deref() {
                        collect_event(event, frontiers, &mut active.clone(), observations);
                    }
                }
                DraftReferenceEvent::PollFlow { body, .. } => {
                    collect_consumers(body, frontiers, &mut active.clone(), observations);
                }
                DraftReferenceEvent::SymmetricCalibrationSearch {
                    initial_read,
                    setup,
                    write_candidate,
                    sample,
                    ..
                } => {
                    for flow in [initial_read, setup, write_candidate, sample] {
                        collect_consumers(flow, frontiers, &mut active.clone(), observations);
                    }
                }
                // A composed body belongs to the callee. Its result is
                // represented by the specialized frontier, not by attributing
                // its internal consumers to this owner.
                _ => {}
            }
            let producer_event = match event {
                DraftReferenceEvent::ComposedCall { token, site, .. }
                | DraftReferenceEvent::ComposedCallWithScratch { token, site, .. } => {
                    Some((*token, *site))
                }
                _ => None,
            };
            if let Some((token, site)) = producer_event {
                let mut matching = frontiers.keys().filter(|producer| producer.site == site);
                if let Some(producer) = matching.next()
                    && matching.next().is_none()
                {
                    active.insert(token, producer.clone());
                } else {
                    active.remove(&token);
                }
            }
        }

        for event in &flow.events {
            collect_event(event, frontiers, active, observations);
        }
        if let DraftReferenceTerminator::Branch {
            taken, not_taken, ..
        } = &flow.terminator
        {
            collect_consumers(taken, frontiers, &mut active.clone(), observations);
            collect_consumers(not_taken, frontiers, &mut active.clone(), observations);
        }
    }

    let mut observations = BTreeMap::new();
    collect_consumers(flow, &frontiers, &mut BTreeMap::new(), &mut observations);
    for ((site, position), mut candidates) in observations {
        if candidates.len() != 1 {
            continue;
        }
        let Some(producer) = candidates.pop_first().flatten() else {
            continue;
        };
        let mut matching = calls
            .iter_mut()
            .filter(|call| call.site == Some(site))
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            continue;
        }
        matching
            .pop()
            .expect("one matching call")
            .argument_result_provenance
            .push(LinkedCallArgumentResultProvenance { position, producer });
    }
    for call in calls {
        call.argument_result_provenance.sort();
        call.argument_result_provenance.dedup();
    }
    frontiers
        .into_iter()
        .map(|(producer, frontier)| LinkedCallResultFrontier { producer, frontier })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_resolver() -> ReferenceResolver {
        ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: direct::StructuralRelocatedCalls::new(),
            pointer_context: direct::StructuralPointerContext::default(),
            data_symbols: Vec::new(),
            data_objects: Vec::new(),
            projected_direct_semantics: BTreeMap::new(),
            projected_origins: BTreeMap::new(),
        }
    }

    fn empty_mmio() -> MmioMap {
        MmioMap {
            registers: Vec::new(),
            regions: Vec::new(),
        }
    }

    fn frontier(flow: &DraftReferenceFlow) -> LinkedGuardedReturnFrontier {
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        guarded_return_frontier(flow, &resolver, &identities, &empty_mmio())
    }

    #[test]
    fn unmodeled_call_results_are_not_exact_return_or_guard_values() {
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::Call {
                token: 7,
                site: 0x100,
                target: 0x200,
                direct: true,
                arguments: Vec::new().into_boxed_slice(),
            }],
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x104,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::CallResult(7),
                    right: SymbolicValue::ExternalResult(9),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(7)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::ExternalResult(9)),
                }),
            },
        };

        let frontier = frontier(&flow);

        assert_eq!(frontier.leaves.len(), 2);
        assert!(frontier.leaves.iter().all(|leaf| !leaf.exact));
        assert!(frontier.leaves.iter().all(|leaf| {
            leaf.guard_path
                .guards
                .iter()
                .all(|guard| !guard.left_exact && !guard.right_exact)
        }));
    }

    #[test]
    fn transformed_ram_and_mmio_values_retain_epoch_sensitive_dependency() {
        let transformed = |source| {
            SymbolicValue::Bits(Box::new(core::array::from_fn(|bit| {
                if bit == 0 {
                    source
                } else {
                    BitSource::Constant(false)
                }
            })))
        };
        let values = [
            SymbolicValue::memory_read(0, 32, false),
            SymbolicValue::register_read(1, 0x6000_1000, 32, false),
            SymbolicValue::indexed_register_read(2, 32, false),
            transformed(BitSource::Memory {
                read_token: 3,
                bit: 4,
                inverted: false,
            }),
            transformed(BitSource::Register {
                read_token: 4,
                address: 0x6000_1000,
                bit: 7,
                inverted: true,
            }),
            transformed(BitSource::IndexedRegister {
                read_token: 5,
                bit: 2,
                inverted: false,
            }),
        ];
        assert!(matches!(values[0], SymbolicValue::MemoryImage { .. }));
        assert!(matches!(values[1], SymbolicValue::RegisterImage { .. }));
        assert!(matches!(
            values[2],
            SymbolicValue::IndexedRegisterImage { .. }
        ));
        assert!(
            values[3..]
                .iter()
                .all(|value| matches!(value, SymbolicValue::Bits(_)))
        );

        for value in values {
            let frontier = frontier(&DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(value),
            });
            assert_eq!(frontier.leaves.len(), 1);
            assert!(frontier.leaves[0].exact);
            assert!(frontier.leaves[0].epoch_sensitive_dependency);
        }
    }

    #[test]
    fn composed_result_publishes_guarded_alternatives_and_unanimous_provenance() {
        let body = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x104,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0x20)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0x40)),
                }),
            },
        };
        let producer = DraftReferenceEvent::ComposedCall {
            token: 7,
            site: 0x100,
            symbol: "select_object".to_owned(),
            direct: true,
            tail: false,
            arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
            flow: Box::new(body),
            result_modeled: true,
        };
        let consumer = DraftReferenceEvent::Call {
            token: 8,
            site: 0x108,
            target: 0x200,
            direct: true,
            arguments: vec![SymbolicValue::CallResult(7)].into_boxed_slice(),
        };
        let flow = DraftReferenceFlow {
            events: vec![producer.clone(), consumer.clone()],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
        };
        let trace = FunctionAnalysis {
            symbol: "owner".to_owned(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: vec!["select_object".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(flow),
            unresolved_branch: None,
        };
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let mut calls = BTreeSet::new();
        for event in [&producer, &consumer] {
            collect_call_event(
                event,
                &resolver,
                &identities,
                None,
                &BTreeMap::new(),
                &mut calls,
            );
        }
        let mut calls = calls.into_iter().collect::<Vec<_>>();
        let consumer = calls
            .iter_mut()
            .find(|call| call.site == Some(0x108))
            .expect("consumer call");
        consumer.arguments[0] = "varies-across-shapes".to_owned();
        consumer.argument_exact[0] = false;

        let frontiers = publish_guarded_call_result_frontiers(
            &trace,
            "fixture::owner",
            &mut calls,
            &resolver,
            &identities,
            &empty_mmio(),
        );

        assert_eq!(frontiers.len(), 1);
        assert!(frontiers[0].frontier.structurally_complete);
        assert_eq!(
            frontiers[0]
                .frontier
                .leaves
                .iter()
                .map(|leaf| leaf.value.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["const:0x00000020", "const:0x00000040"])
        );
        assert_eq!(
            calls
                .iter()
                .find(|call| call.site == Some(0x108))
                .expect("consumer call")
                .argument_result_provenance
                .len(),
            1
        );
    }

    #[test]
    fn conflicting_composed_specializations_are_suppressed() {
        let producer = |value| DraftReferenceEvent::ComposedCall {
            token: 7,
            site: 0x100,
            symbol: "select_object".to_owned(),
            direct: true,
            tail: false,
            arguments: Vec::new().into_boxed_slice(),
            flow: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(value)),
            }),
            result_modeled: true,
        };
        let first = producer(0x20);
        let second = producer(0x40);
        let flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x80,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: vec![first.clone()],
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: vec![second],
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
            },
        };
        let trace = FunctionAnalysis {
            symbol: "owner".to_owned(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: vec!["select_object".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(flow),
            unresolved_branch: None,
        };
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let mut calls = BTreeSet::new();
        collect_call_event(
            &first,
            &resolver,
            &identities,
            None,
            &BTreeMap::new(),
            &mut calls,
        );
        let mut calls = calls.into_iter().collect::<Vec<_>>();

        let frontiers = publish_guarded_call_result_frontiers(
            &trace,
            "fixture::owner",
            &mut calls,
            &resolver,
            &identities,
            &empty_mmio(),
        );

        assert!(frontiers.is_empty());
        assert!(!calls[0].result_modeled);
        assert!(calls[0].result_provenance.is_none());
    }

    #[test]
    fn branch_local_call_tokens_keep_their_site_local_producer() {
        let producer = |site, value| DraftReferenceEvent::ComposedCall {
            token: 0,
            site,
            symbol: "select_object".to_owned(),
            direct: true,
            tail: false,
            arguments: Vec::new().into_boxed_slice(),
            flow: Box::new(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(value)),
            }),
            result_modeled: true,
        };
        let consumer = |site| DraftReferenceEvent::Call {
            token: 1,
            site,
            target: 0x200,
            direct: true,
            arguments: vec![SymbolicValue::CallResult(0)].into_boxed_slice(),
        };
        let events = [
            producer(0x100, 0x20),
            consumer(0x108),
            producer(0x120, 0x40),
            consumer(0x128),
        ];
        let flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x80,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: events[..2].to_vec(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: events[2..].to_vec(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                }),
            },
        };
        let trace = FunctionAnalysis {
            symbol: "owner".to_owned(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: vec!["select_object".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(flow),
            unresolved_branch: None,
        };
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let mut calls = BTreeSet::new();
        for event in &events {
            collect_call_event(
                event,
                &resolver,
                &identities,
                None,
                &BTreeMap::new(),
                &mut calls,
            );
        }
        let mut calls = calls.into_iter().collect::<Vec<_>>();

        let frontiers = publish_guarded_call_result_frontiers(
            &trace,
            "fixture::owner",
            &mut calls,
            &resolver,
            &identities,
            &empty_mmio(),
        );

        assert_eq!(frontiers.len(), 2);
        for (consumer_site, producer_site) in [(0x108, 0x100), (0x128, 0x120)] {
            assert_eq!(
                calls
                    .iter()
                    .find(|call| call.site == Some(consumer_site))
                    .expect("consumer")
                    .argument_result_provenance[0]
                    .producer
                    .site,
                producer_site
            );
        }
    }

    #[test]
    fn guarded_frontier_keeps_branch_local_memory_read_namespaces() {
        let branch = |address| DraftReferenceFlow {
            events: vec![DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: SymbolicValue::Constant(address),
                region: "dram".to_owned(),
                value: None,
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::memory_read(0, 32, false)),
        };
        let flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x80,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(branch(0x3fca_1000)),
                not_taken: Box::new(branch(0x3fca_2000)),
            },
        };
        let resolver = empty_resolver();
        let frontier = guarded_return_frontier(
            &flow,
            &resolver,
            &IrIdentityCatalog::new(&resolver, None),
            &empty_mmio(),
        );

        assert!(frontier.structurally_complete);
        assert_eq!(
            frontier
                .leaves
                .iter()
                .map(|leaf| leaf.value.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "memory:absolute:0x3fca1000&0xffffffff|0x00000000",
                "memory:absolute:0x3fca2000&0xffffffff|0x00000000",
            ])
        );
    }

    #[test]
    fn guarded_frontier_keeps_branch_local_call_sites() {
        let branch = |site| DraftReferenceFlow {
            events: vec![DraftReferenceEvent::Call {
                token: 0,
                site,
                target: 0x200,
                direct: true,
                arguments: Vec::new().into_boxed_slice(),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(0)),
        };
        let flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x80,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(branch(0x100)),
                not_taken: Box::new(branch(0x120)),
            },
        };
        let resolver = empty_resolver();
        let frontier = guarded_return_frontier(
            &flow,
            &resolver,
            &IrIdentityCatalog::new(&resolver, None),
            &empty_mmio(),
        );

        assert!(frontier.structurally_complete);
        assert_eq!(
            frontier
                .leaves
                .iter()
                .map(|leaf| leaf.value.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
    }

    #[test]
    fn unresolved_values_do_not_erase_structural_terminals() {
        let flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x80,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::Unknown,
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
            },
        };
        let resolver = empty_resolver();
        let frontier = guarded_return_frontier(
            &flow,
            &resolver,
            &IrIdentityCatalog::new(&resolver, None),
            &empty_mmio(),
        );

        assert!(frontier.structurally_complete);
        assert_eq!(frontier.leaves.len(), 2);
        assert!(frontier.leaves.iter().any(|leaf| !leaf.exact));
        assert!(frontier.leaves.iter().all(|leaf| {
            leaf.guard_path
                .guards
                .first()
                .is_some_and(|guard| !guard.left_exact)
        }));
    }
}
