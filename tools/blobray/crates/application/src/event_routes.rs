//! Conditional route acquisition and review share selected navigation and local facts.
use crate::*;
use blobray_analysis::event_routes::{Prepared, RouteValue, ValueProducer, offset, same_pointer};
mod local;
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
struct Owned<'m, T> {
    value: T,
    _capacity: MemoryReservation<'m>,
}
struct Descriptor {
    id: FunctionAnalysisId,
    recipe: FunctionRecipe,
}
struct Call {
    hop: FlowHop,
    arguments: Vec<RouteValue>,
}
struct Target {
    site: SavedSite,
    value: RouteValue,
    callback: FunctionAnalysisId,
}
struct Rows<'m> {
    values: AdmittedVec<'m, Owned<'m, EventRouteRecord>>,
}
impl<'m> Rows<'m> {
    fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            values: AdmittedVec::new(memory),
        }
    }
    fn push(
        &mut self,
        value: EventRouteRecord,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let bytes = match &value {
            EventRouteRecord::Evidence { site, fact } => {
                site.analysis.allocated_bytes()
                    + std::mem::size_of::<FunctionRecord>() as u64
                    + fact.allocated_bytes()
            }
            EventRouteRecord::Check { name, evidence, .. } => {
                name.capacity() as u64
                    + (evidence.capacity() * std::mem::size_of::<SavedSite>()) as u64
                    + evidence
                        .iter()
                        .map(|s| s.analysis.allocated_bytes())
                        .sum::<u64>()
            }
            EventRouteRecord::Condition { .. } => 0,
        };
        let capacity = memory.reserve(bytes, c.position())?;
        self.values.push(
            Owned {
                value,
                _capacity: capacity,
            },
            c.position(),
        )
    }
    fn check(
        &mut self,
        name: &str,
        status: RouteCheckStatus,
        evidence: Vec<SavedSite>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.push(
            EventRouteRecord::Check {
                name: name.into(),
                status,
                evidence,
            },
            memory,
            c,
        )
    }
    fn fact(
        &mut self,
        site: &SavedSite,
        fact: &FunctionRecord,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.push(
            EventRouteRecord::Evidence {
                site: site.clone(),
                fact: Box::new(fact.clone()),
            },
            memory,
            c,
        )
    }
}
fn site(hop: &FlowHop) -> SavedSite {
    SavedSite {
        analysis: hop.caller.clone(),
        record: hop.record,
    }
}
fn constant(value: &RouteValue, expected: u32) -> RouteCheckStatus {
    match value.constant() {
        Some(actual) if actual == expected => RouteCheckStatus::Established,
        Some(_) => RouteCheckStatus::Mismatch,
        None => RouteCheckStatus::Unresolved,
    }
}
fn call<'a>(calls: &'a [Owned<'_, Call>], hop: &FlowHop) -> Result<&'a Call> {
    calls
        .iter()
        .find(|c| c.value.hop == *hop)
        .map(|c| &c.value)
        .ok_or_else(|| invalid("event call observation is absent"))
}
fn argument<'a>(calls: &'a [Owned<'_, Call>], hop: &FlowHop, word: u8) -> Result<&'a RouteValue> {
    // Slot 8 is explicitly unknown for the current saved-call stack profile.
    call(calls, hop)?
        .arguments
        .get(usize::from(word.min(8)))
        .ok_or_else(|| invalid("event call argument is absent"))
}
fn descriptor<'a>(
    descriptors: &'a [Owned<'_, Descriptor>],
    id: &FunctionAnalysisId,
) -> Result<&'a FunctionRecipe> {
    descriptors
        .iter()
        .find(|d| d.value.id == *id)
        .map(|d| &d.value.recipe)
        .ok_or_else(|| invalid("event participant is absent"))
}
fn selected(route: &ReviewedEventRoute) -> Vec<FlowHop> {
    let mut calls = Vec::new();
    route.visit_calls(|call| {
        if !calls.contains(call) {
            calls.push(call.clone());
        }
    });
    calls
}
pub(crate) fn query(
    project: &Project,
    request: &EventRouteQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&EventRouteRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<EventRouteSummary> {
    evaluate(project, request, None, memory, c, emit).map(|(summary, _)| summary)
}
fn evaluate(
    project: &Project,
    request: &EventRouteQuery,
    occurrence: Option<&KnowledgeOccurrence>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&EventRouteRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<(EventRouteSummary, Vec<ArtifactId>)> {
    blobray_knowledge::validate_event_route(&request.route)?;
    let _construction = memory.reserve(
        2 * 1024 * 1024 + request.route.allocated_bytes(),
        c.position(),
    )?;
    let hops = selected(&request.route);
    let mut ids = Vec::new();
    for hop in &hops {
        ids.push(hop.caller.clone());
        ids.push(hop.callee.clone());
    }
    match &request.route.route {
        EventRouteMechanism::StaticCallback(r) => ids.push(r.callback.clone()),
        EventRouteMechanism::BrokerSubscription(r) => ids.push(r.callback.clone()),
        _ => (),
    }
    let mut descriptors = AdmittedVec::new(memory);
    let mut calls = AdmittedVec::new(memory);
    let mut targets = AdmittedVec::new(memory);
    let mut rows = Rows::new(memory);
    let mut call_rows = Rows::new(memory);
    let mut roots = Vec::new();
    let mut seen = vec![false; hops.len()];
    let navigation = crate::navigation::query_inspected(
        project,
        &NavigationQuery {
            scope: NavigationScope {
                revision: request.revision.clone(),
                publications: vec![],
                analyses: ids,
                knowledge: None,
            },
            filter: NavigationFilter::Calls {
                function: None,
                direction: CallDirection::Callees,
            },
        },
        memory,
        c,
        &mut |function, manifest, c| {
            if &function.analysis == request.route.root()
                && occurrence.is_some_and(|o| {
                    o.revision != request.revision
                        || o.source != function.location.source
                        || o.object != *function.location.selector.object()
                        || o.symbol
                            .as_ref()
                            .is_some_and(|s| function.location.selector.symbol() != Some(s))
                })
            {
                return Err(invalid(
                    "event route root differs from its physical occurrence",
                ));
            }
            roots.push(function.analysis.as_str().parse()?);
            roots.push(manifest.records.clone());
            let capacity = memory.reserve(2048, c.position())?; // Fixed-size identities in the current function recipe.
            descriptors.push(
                Owned {
                    value: Descriptor {
                        id: function.analysis.clone(),
                        recipe: manifest.recipe.clone(),
                    },
                    _capacity: capacity,
                },
                c.position(),
            )
        },
        &mut |function, manifest, records, facts, c| {
            c.phase(RunPhase::AnalyzeValues)?;
            let prepared = Prepared::new(
                records,
                &manifest.recipe,
                manifest.coverage,
                facts,
                memory,
                c,
            )?;
            for hop in &hops {
                c.checkpoint(1)?;
                if hop.caller != function.analysis {
                    continue;
                }
                let record = prepared.record(hop.record)?;
                rows.fact(&site(hop), record, memory, c)?;
                if let Some(ordinal) = facts.call_inputs_record(offset(record)?, c)? {
                    rows.fact(
                        &SavedSite {
                            analysis: function.analysis.clone(),
                            record: ordinal,
                        },
                        &records[ordinal as usize],
                        memory,
                        c,
                    )?;
                }
                let mut arguments = Vec::with_capacity(9);
                for word in 0..9 {
                    arguments.push(prepared.argument(hop.record, word, c)?);
                }
                let capacity = memory.reserve(
                    (arguments.capacity() * std::mem::size_of::<RouteValue>()) as u64
                        + arguments
                            .iter()
                            .map(RouteValue::allocated_bytes)
                            .sum::<u64>()
                        + hop.caller.allocated_bytes()
                        + hop.callee.allocated_bytes(),
                    c.position(),
                )?;
                calls.push(
                    Owned {
                        value: Call {
                            hop: hop.clone(),
                            arguments,
                        },
                        _capacity: capacity,
                    },
                    c.position(),
                )?;
            }
            local::inspect(
                &request.route,
                &function.analysis,
                &prepared,
                &mut rows,
                &mut targets,
                memory,
                c,
            )
        },
        &mut |record, c| {
            if let NavigationRecord::Call {
                caller,
                record,
                candidates,
                issue,
                ..
            } = record
            {
                for (i, hop) in hops.iter().enumerate() {
                    c.checkpoint(1)?;
                    if hop.caller == caller.analysis && hop.record == *record {
                        seen[i] = true;
                        let status = if issue.is_some() || candidates.len() != 1 {
                            RouteCheckStatus::Unresolved
                        } else if candidates[0].analysis == hop.callee {
                            RouteCheckStatus::Established
                        } else {
                            RouteCheckStatus::Mismatch
                        };
                        call_rows.check("selected-call", status, vec![site(hop)], memory, c)?;
                    }
                }
            }
            Ok(())
        },
    )?;
    for (i, hop) in hops.iter().enumerate() {
        if !seen[i] {
            call_rows.check(
                "selected-call",
                RouteCheckStatus::Mismatch,
                vec![site(hop)],
                memory,
                c,
            )?;
        }
    }
    c.phase(RunPhase::AnalyzeValues)?;
    match &request.route.route {
        EventRouteMechanism::SelectorDelivery(r) => {
            rows.check(
                "dispatch-selector",
                constant(
                    argument(&calls, &r.dispatch.call, r.dispatch.word)?,
                    r.selector,
                ),
                vec![site(&r.dispatch.call)],
                memory,
                c,
            )?;
        }
        EventRouteMechanism::StaticCallback(r) => {
            for d in &r.dispatches {
                rows.check(
                    "registered-dispatch-object",
                    same_pointer(
                        argument(&calls, &d.call, d.object_word)?,
                        argument(&calls, &r.registration.call, r.registration.object_word)?,
                        0,
                    ),
                    vec![site(&d.call), site(&r.registration.call)],
                    memory,
                    c,
                )?;
                rows.check(
                    "dispatch-receive-queue",
                    same_pointer(
                        argument(&calls, &d.call, d.queue_word)?,
                        argument(&calls, &r.receive.call, r.receive.queue_word)?,
                        0,
                    ),
                    vec![site(&d.call), site(&r.receive.call)],
                    memory,
                    c,
                )?;
            }
        }
        EventRouteMechanism::BrokerSubscription(r) => {
            rows.check(
                "published-selector",
                constant(argument(&calls, &r.publish, r.selector_word)?, r.selector),
                vec![site(&r.publish)],
                memory,
                c,
            )?;
            rows.check(
                "attached-domain",
                constant(
                    argument(&calls, &r.domain.attach, r.domain.selector_word)?,
                    r.domain.selector,
                ),
                vec![site(&r.domain.attach)],
                memory,
                c,
            )?;
            rows.check(
                "subscribed-domain",
                constant(
                    argument(&calls, &r.subscribe, r.domain_word)?,
                    r.domain.selector,
                ),
                vec![site(&r.subscribe)],
                memory,
                c,
            )?;
            rows.check(
                "attached-publisher-object",
                same_pointer(
                    argument(&calls, &r.publish, r.object_word)?,
                    argument(&calls, &r.domain.attach, r.domain.object_word)?,
                    0,
                ),
                vec![site(&r.publish), site(&r.domain.attach)],
                memory,
                c,
            )?;
        }
    }
    for target in targets.iter() {
        c.checkpoint(1)?;
        let target = &target.value;
        let caller = descriptor(&descriptors, &target.site.analysis)?;
        let value = target.value.target();
        let mut matches = 0;
        let mut expected = false;
        for d in descriptors.iter() {
            c.checkpoint(1)?;
            if value
                .as_ref()
                .is_some_and(|v| crate::navigation::target_matches(caller, &d.value.recipe, v))
            {
                matches += 1;
                expected |= d.value.id == target.callback;
            }
        }
        let status = if matches == 1 && expected {
            RouteCheckStatus::Established
        } else if matches > 1 || matches == 0 {
            RouteCheckStatus::Unresolved
        } else {
            RouteCheckStatus::Mismatch
        };
        rows.check(
            "callback-identity",
            status,
            vec![target.site.clone()],
            memory,
            c,
        )?;
    }
    for condition in [
        RouteCondition::MechanismSemantics,
        RouteCondition::ObjectLifetime,
        RouteCondition::DeliveryOrder,
        RouteCondition::ExecutionContext,
        RouteCondition::RuntimeGuard,
    ] {
        rows.push(EventRouteRecord::Condition { condition }, memory, c)?;
    }
    if !matches!(
        request.route.route,
        EventRouteMechanism::SelectorDelivery(_)
    ) {
        rows.push(
            EventRouteRecord::Condition {
                condition: RouteCondition::RegistrationBeforeDispatch,
            },
            memory,
            c,
        )?;
    }
    let mut summary = EventRouteSummary {
        schema: 1,
        analyses_read: navigation.analyses_read,
        request: request.clone(),
        established: 0,
        unresolved: 0,
        mismatched: 0,
        conditions: 0,
    };
    for row in call_rows.values.iter().chain(rows.values.iter()) {
        c.checkpoint(1)?;
        match &row.value {
            EventRouteRecord::Check { status, .. } => match status {
                RouteCheckStatus::Established => summary.established += 1,
                RouteCheckStatus::Unresolved => summary.unresolved += 1,
                RouteCheckStatus::Mismatch => summary.mismatched += 1,
            },
            EventRouteRecord::Condition { .. } => summary.conditions += 1,
            _ => (),
        }
        emit(&row.value, c)?;
    }
    Ok((summary, roots))
}
pub(crate) fn validate(
    project: &Project,
    proposal: &KnowledgeProposal,
    route: &ReviewedEventRoute,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    let (summary, roots) = evaluate(
        project,
        &EventRouteQuery {
            revision: proposal.occurrence.revision.clone(),
            route: route.clone(),
        },
        Some(&proposal.occurrence),
        memory,
        c,
        &mut |_, _| Ok(()),
    )?;
    if summary.unresolved > 0 || summary.mismatched > 0 {
        return Err(invalid(
            "event route has unresolved or mismatched structural bindings; inspect the event-route query before review",
        ));
    }
    Ok(roots)
}
