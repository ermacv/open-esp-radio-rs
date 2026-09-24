//! One read owner for explicit research scopes and bounded navigation indexes.
use crate::*;
use blobray_analysis::navigation::{CallObservation, Facts};
mod accesses;
mod calls;
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
struct Selected<'m> {
    value: FunctionAnalysisId,
    name: Option<Vec<u8>>,
    _capacity: MemoryReservation<'m>,
}
struct Node<'m> {
    function: NavigationFunction,
    extent: CodeRange,
    section: u32,
    space: CodeAddressSpace,
    user_extent: bool,
    _capacity: MemoryReservation<'m>,
}
struct Pending<'m> {
    caller: usize,
    call: CallObservation,
    _capacity: MemoryReservation<'m>,
}
struct Declaration<'a, 'm> {
    key: FunctionLocation,
    entry: &'a KnowledgeEntry,
    _capacity: MemoryReservation<'m>,
}
fn sort_charge(n: usize, c: &mut dyn RunControl) -> Result<()> {
    c.checkpoint(n as u64 * (n.max(1).ilog2() as u64 + 1))
}
pub(crate) fn query(
    project: &Project,
    request: &NavigationQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<NavigationSummary> {
    query_observed(project, request, memory, c, &mut |_, _, _, _| Ok(()), emit)
}
type DescriptorObserver<'a> = dyn FnMut(&NavigationFunction, &FunctionManifest, Option<&[u8]>, &mut dyn RunControl) -> Result<()>
    + 'a;
/// Same selection and single fact pass, with borrowed names from selected publications.
pub(crate) fn query_observed(
    project: &Project,
    request: &NavigationQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    observe: &mut DescriptorObserver<'_>,
    emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<NavigationSummary> {
    query_inner(project, request, memory, c, observe, None, emit)
}
type FactsObserver<'a> = dyn FnMut(
        &NavigationFunction,
        &FunctionManifest,
        &[FunctionRecord],
        &Facts<'_, '_>,
        &mut dyn RunControl,
    ) -> Result<()>
    + 'a;
/// Additional synchronous borrowed-facts consumer; it must admit any retained copies.
pub(crate) fn query_inspected(
    project: &Project,
    request: &NavigationQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    observe: &mut DescriptorObserver<'_>,
    inspect: &mut FactsObserver<'_>,
    emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<NavigationSummary> {
    query_inner(project, request, memory, c, observe, Some(inspect), emit)
}
fn query_inner(
    project: &Project,
    request: &NavigationQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    observe: &mut DescriptorObserver<'_>,
    mut inspect: Option<&mut FactsObserver<'_>>,
    emit: &mut dyn FnMut(&NavigationRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<NavigationSummary> {
    let scope = &request.scope;
    if scope.publications.len() > 64
        || scope.analyses.len() > 4096
        || scope.publications.is_empty() && scope.analyses.is_empty()
    {
        return Err(invalid(
            "navigation needs a bounded explicit publication/analysis selection",
        ));
    }
    // Current manifest, one JSON record and bounded result construction; retained indexes are separate.
    let _envelope = memory.reserve(4 * 1024 * 1024, c.position())?;
    let snapshot = project.knowledge_snapshot(scope.knowledge.as_ref(), memory, c)?;
    let target = accesses::Target::new(project, request, &snapshot, memory, c)?;
    let mut summary = NavigationSummary {
        schema: 1,
        request: request.clone(),
        selected_analyses: 0,
        analyses_read: 0,
        unavailable_entries: 0,
        partial_analyses: 0,
        observations: 0,
        unresolved: 0,
        ambiguous: 0,
        data: target.as_ref().and_then(accesses::Target::data),
    };
    let mut ids = AdmittedVec::new(memory);
    let mut add = |id: FunctionAnalysisId, name: Option<Vec<u8>>, c: &mut dyn RunControl| {
        let capacity = memory.reserve(
            id.allocated_bytes() + name.as_ref().map_or(0, |n| n.capacity() as u64),
            c.position(),
        )?;
        ids.push(
            Selected {
                value: id,
                name,
                _capacity: capacity,
            },
            c.position(),
        )
    };
    for id in &scope.analyses {
        c.checkpoint(1)?;
        add(id.clone(), None, c)?;
    }
    for (i, id) in scope.publications.iter().enumerate() {
        c.checkpoint(i as u64 + 1)?;
        if scope.publications[..i].contains(id) {
            continue;
        }
        let publication = project.publication(id, c)?;
        if publication.manifest.plan.recipe.request.revision.as_ref() != Some(&scope.revision) {
            return Err(invalid("publication belongs to another source revision"));
        }
        blobray_store::visit_jsonl(&publication.members, c, |member: InvestigationMember, c| {
            match &member.outcome {
                InvestigationOutcome::Analyzed { analysis, .. } => {
                    let PlanEntry::Function { name, .. } = member.entry else {
                        return Err(Error::new(
                            ErrorCode::Integrity,
                            "analyzed membership is not a function",
                        ));
                    };
                    add(analysis.clone(), name, c)?;
                }
                _ if matches!(
                    member.entry,
                    PlanEntry::Gap { .. }
                        | PlanEntry::ImageGap { .. }
                        | PlanEntry::Function { .. }
                        | PlanEntry::Object {
                            supported: false,
                            ..
                        }
                ) || matches!(member.outcome, InvestigationOutcome::Blocked { .. }) =>
                {
                    summary.unavailable_entries += 1;
                    emit(
                        &NavigationRecord::Unavailable {
                            publication: id.clone(),
                            member: Box::new(member),
                        },
                        c,
                    )?;
                }
                _ => (),
            }
            Ok(())
        })?;
    }
    sort_charge(ids.len(), c)?;
    ids.sort_unstable_by(|a, b| a.value.cmp(&b.value));
    let mut declarations = AdmittedVec::new(memory);
    if matches!(request.filter, NavigationFilter::Functions { .. }) {
        for entry in snapshot.entries() {
            c.checkpoint(1)?;
            if entry.proposal.occurrence.revision != scope.revision {
                continue;
            }
            if let KnowledgeClaim::Function { contract } = &entry.proposal.claim {
                let key = FunctionLocation {
                    source: entry.proposal.occurrence.source.clone(),
                    selector: contract.selector.clone(),
                };
                let capacity = memory.reserve(key.allocated_bytes(), c.position())?;
                declarations.push(
                    Declaration {
                        key,
                        entry,
                        _capacity: capacity,
                    },
                    c.position(),
                )?;
            }
        }
        sort_charge(declarations.len(), c)?;
        declarations.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    }
    let mut reader = project.analysis_reader(memory);
    let mut nodes = AdmittedVec::new(memory);
    let mut pending = AdmittedVec::new(memory);
    let focus = match &request.filter {
        NavigationFilter::Functions { function } | NavigationFilter::Calls { function, .. } => {
            function.as_ref()
        }
        _ => None,
    };
    let mut focus_found = focus.is_none();
    let mut context_found = !matches!(request.filter, NavigationFilter::Context { .. });
    for (i, id) in ids.iter().enumerate() {
        c.checkpoint(1)?;
        if i > 0 && ids[i - 1].value == id.value {
            continue;
        }
        let mut name = None;
        for candidate in ids[i..].iter().take_while(|n| n.value == id.value) {
            c.checkpoint(1)?;
            if let Some(current) = candidate.name.as_deref() {
                if name.is_some_and(|previous| previous != current) {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "selected publications disagree on a function name",
                    ));
                }
                name = Some(current);
            }
        }
        let lease = reader.analysis(&id.value, c)?;
        let recipe = &lease.manifest.recipe;
        if recipe.revision != scope.revision {
            return Err(invalid("analysis belongs to another source revision"));
        }
        let function = NavigationFunction {
            analysis: id.value.clone(),
            location: FunctionLocation {
                source: recipe.source.clone(),
                selector: recipe.selector.clone(),
            },
        };
        focus_found |= focus.is_some_and(|f| f == &function.location);
        let capacity = memory.reserve(function.allocated_bytes(), c.position())?;
        let node = Node {
            function,
            extent: recipe.extent,
            section: recipe.section,
            space: recipe.address_space,
            user_extent: recipe.user_extent,
            _capacity: capacity,
        };
        observe(&node.function, &lease.manifest, name, c)?;
        summary.selected_analyses += 1;
        summary.partial_analyses += u64::from(
            !lease.manifest.coverage.complete()
                || lease
                    .manifest
                    .semantics
                    .as_ref()
                    .is_none_or(|s| !s.complete),
        );
        if let NavigationFilter::Functions { .. } = &request.filter
            && focus.is_none_or(|f| f == &node.function.location)
        {
            emit(
                &NavigationRecord::Function {
                    function: node.function.clone(),
                    extent: node.extent,
                    address_space: node.space,
                    coverage: lease.manifest.coverage,
                    semantic_complete: lease.manifest.semantics.as_ref().map(|s| s.complete),
                },
                c,
            )?;
            summary.observations += 1;
            c.checkpoint(declarations.len().max(1).ilog2() as u64 + 1)?;
            let start = declarations.partition_point(|d| d.key < node.function.location);
            for declaration in declarations[start..]
                .iter()
                .take_while(|d| d.key == node.function.location)
            {
                c.checkpoint(1)?;
                emit(
                    &NavigationRecord::Declaration {
                        function: node.function.clone(),
                        assertion: declaration.entry.id.clone(),
                        state: declaration.entry.state,
                    },
                    c,
                )?;
            }
        }
        if inspect.is_some()
            || matches!(request.filter, NavigationFilter::Calls { .. })
            || target.as_ref().is_some_and(|t| t.relevant(recipe))
        {
            context_found = true;
            let records = crate::research::load_records(&lease.records, memory, c)?;
            summary.analyses_read += 1;
            let facts = Facts::new(&records, memory, c)?;
            if let Some(inspect) = inspect.as_mut() {
                inspect(&node.function, &lease.manifest, &records, &facts, c)?;
            }
            if matches!(request.filter, NavigationFilter::Calls { .. }) {
                facts.calls(recipe, c, &mut |call, c| {
                    let capacity = memory.reserve(
                        call.target.allocated_bytes()
                            + call
                                .saved_resolution
                                .as_ref()
                                .map_or(0, FunctionAnalysisId::allocated_bytes),
                        c.position(),
                    )?;
                    pending.push(
                        Pending {
                            caller: nodes.len(),
                            call,
                            _capacity: capacity,
                        },
                        c.position(),
                    )
                })?;
            } else if let Some(target) = &target {
                target.visit(&node, recipe, &facts, memory, c, &mut |record, c| {
                    count(record, &mut summary);
                    emit(record, c)
                })?;
            }
        }
        nodes.push(node, c.position())?;
    }
    if !focus_found || !context_found {
        return Err(Error::new(
            ErrorCode::NotFound,
            "selected function is absent from the explicit research scope",
        ));
    }
    if let NavigationFilter::Calls {
        function,
        direction,
    } = &request.filter
    {
        calls::emit(
            &nodes,
            &pending,
            function.as_ref(),
            *direction,
            memory,
            c,
            &mut |record, c| {
                count(record, &mut summary);
                emit(record, c)
            },
        )?;
    }
    Ok(summary)
}
fn count(record: &NavigationRecord, summary: &mut NavigationSummary) {
    summary.observations += 1;
    let issue = match record {
        NavigationRecord::Call { issue, .. } | NavigationRecord::Access { issue, .. } => *issue,
        _ => None,
    };
    match issue {
        Some(NavigationIssue::AmbiguousTarget | NavigationIssue::AmbiguousAddress) => {
            summary.ambiguous += 1
        }
        Some(_) => summary.unresolved += 1,
        None => (),
    }
}

#[cfg(test)]
mod tests;

pub(crate) fn target_matches(
    caller: &FunctionRecipe,
    target: &FunctionRecipe,
    value: &AbstractValue,
) -> bool {
    calls::target_matches(caller, target, value)
}

/// Heap payload retained when storing a call row beyond its borrowed callback.
pub(crate) fn call_bytes(record: &NavigationRecord) -> u64 {
    match record {
        NavigationRecord::Call {
            caller,
            target,
            saved_resolution,
            candidates,
            ..
        } => {
            caller.allocated_bytes()
                + target.allocated_bytes()
                + saved_resolution
                    .as_ref()
                    .map_or(0, FunctionAnalysisId::allocated_bytes)
                + (candidates.capacity() * std::mem::size_of::<NavigationFunction>()) as u64
                + candidates
                    .iter()
                    .map(NavigationFunction::allocated_bytes)
                    .sum::<u64>()
        }
        _ => 0,
    }
}
