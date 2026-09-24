//! Selected graph ownership and phase-bounded effect delivery; never starts analysis.
use crate::*;
use blobray_analysis::flow::{Arc as Edge, reachable};
struct Node<'m> {
    function: NavigationFunction,
    partial: bool,
    _capacity: MemoryReservation<'m>,
}
struct Call<'m> {
    record: NavigationRecord,
    _capacity: MemoryReservation<'m>,
}
fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
fn position(nodes: &[Node<'_>], id: &FunctionAnalysisId, c: &mut dyn RunControl) -> Result<usize> {
    c.checkpoint(nodes.len().max(1).ilog2() as u64 + 1)?;
    nodes
        .binary_search_by(|n| n.function.analysis.cmp(id))
        .map_err(|_| invalid("flow function is absent from selected scope"))
}
pub(crate) fn query(
    project: &Project,
    request: &FlowQuery,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    emit: &mut dyn FnMut(&FlowRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<FlowSummary> {
    let _construction = memory.reserve(65536 + 2 * request.allocated_bytes(), c.position())?;
    if request.max_depth > 4096 {
        return Err(invalid("flow depth exceeds 4096"));
    }
    if matches!(
        request.goal,
        FlowGoal::Effects {
            profile: FlowEffectProfile::Calls,
            address: Some(_)
        }
    ) {
        return Err(invalid(
            "address filtering requires a memory effect profile",
        ));
    }
    let mut nodes = AdmittedVec::new(memory);
    let mut calls = AdmittedVec::new(memory);
    let selection = crate::navigation::query_observed(
        project,
        &NavigationQuery {
            scope: request.scope.clone(),
            filter: NavigationFilter::Calls {
                function: None,
                direction: CallDirection::Callees,
            },
        },
        memory,
        c,
        &mut |function, manifest, _, c| {
            let capacity = memory.reserve(function.allocated_bytes(), c.position())?;
            nodes.push(
                Node {
                    function: function.clone(),
                    partial: !manifest.coverage.complete()
                        || manifest.semantics.as_ref().is_none_or(|s| !s.complete),
                    _capacity: capacity,
                },
                c.position(),
            )
        },
        &mut |record, c| {
            match record {
                NavigationRecord::Call { .. } => {
                    let capacity =
                        memory.reserve(crate::navigation::call_bytes(record), c.position())?;
                    calls.push(
                        Call {
                            record: record.clone(),
                            _capacity: capacity,
                        },
                        c.position(),
                    )?;
                }
                NavigationRecord::Unavailable {
                    publication,
                    member,
                } => emit(
                    &FlowRecord::Unavailable {
                        publication: publication.clone(),
                        member: member.clone(),
                    },
                    c,
                )?,
                _ => (),
            }
            Ok(())
        },
    )?;
    let root = position(&nodes, &request.root, c)?;
    let target = if let FlowGoal::Function { analysis } = &request.goal {
        Some(position(&nodes, analysis, c)?)
    } else {
        None
    };
    let mut edges = AdmittedVec::new(memory);
    for call in calls.iter() {
        c.checkpoint(1)?;
        if let NavigationRecord::Call {
            caller,
            record,
            candidates,
            issue: None,
            ..
        } = &call.record
        {
            if candidates.len() != 1 {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "unambiguous flow edge has no unique candidate",
                ));
            }
            edges.push(
                Edge {
                    from: position(&nodes, &caller.analysis, c)?,
                    to: position(&nodes, &candidates[0].analysis, c)?,
                    record: *record,
                },
                c.position(),
            )?;
        }
    }
    c.checkpoint(edges.len() as u64 * (edges.len().max(1).ilog2() as u64 + 1))?;
    edges.sort_unstable_by_key(|e| (e.from, e.record, e.to));
    let reached = reachable(nodes.len(), &edges, root, request.max_depth, memory, c)?;
    let mut summary = FlowSummary {
        schema: 1,
        request: request.clone(),
        selected_analyses: selection.selected_analyses,
        reached_analyses: 0,
        facts_passes: selection.analyses_read,
        effects: 0,
        frontiers: 0,
        unavailable_entries: selection.unavailable_entries,
        target_reached: target.map(|i| reached[i].is_some()),
    };
    drop(selection);
    for (i, node) in nodes.iter().enumerate() {
        c.checkpoint(1)?;
        let Some(found) = reached[i] else {
            continue;
        };
        summary.reached_analyses += 1;
        let parent = found.parent.map(|j| {
            let edge = &edges[j];
            FlowHop {
                caller: nodes[edge.from].function.analysis.clone(),
                record: edge.record,
                callee: node.function.analysis.clone(),
            }
        });
        emit(
            &FlowRecord::Function {
                function: node.function.clone(),
                depth: found.depth,
                parent,
            },
            c,
        )?;
        if node.partial {
            summary.frontiers += 1;
            emit(
                &FlowRecord::Frontier {
                    analysis: node.function.analysis.clone(),
                    record: None,
                    reason: FlowFrontier::PartialAnalysis,
                },
                c,
            )?;
        }
    }
    for edge in edges.iter() {
        c.checkpoint(1)?;
        if reached[edge.from].is_some() && reached[edge.to].is_none() {
            summary.frontiers += 1;
            emit(
                &FlowRecord::Frontier {
                    analysis: nodes[edge.from].function.analysis.clone(),
                    record: Some(edge.record),
                    reason: FlowFrontier::Depth,
                },
                c,
            )?;
        }
    }
    for call in calls.iter() {
        c.checkpoint(1)?;
        let NavigationRecord::Call {
            caller,
            record,
            issue,
            ..
        } = &call.record
        else {
            continue;
        };
        if reached[position(&nodes, &caller.analysis, c)?].is_none() {
            continue;
        }
        if let Some(issue) = issue {
            let reason = match issue {
                NavigationIssue::AmbiguousTarget => FlowFrontier::AmbiguousCall,
                NavigationIssue::OutsideSelection => FlowFrontier::OutsideSelection,
                _ => FlowFrontier::UnresolvedCall,
            };
            summary.frontiers += 1;
            emit(
                &FlowRecord::Frontier {
                    analysis: caller.analysis.clone(),
                    record: Some(*record),
                    reason,
                },
                c,
            )?;
        }
        if !matches!(
            request.goal,
            FlowGoal::Effects {
                profile: FlowEffectProfile::Memory,
                ..
            }
        ) {
            let _output = memory.reserve(
                crate::navigation::call_bytes(&call.record)
                    + std::mem::size_of::<NavigationRecord>() as u64,
                c.position(),
            )?;
            emit(
                &FlowRecord::Call {
                    observation: Box::new(call.record.clone()),
                },
                c,
            )?;
            if matches!(request.goal, FlowGoal::Effects { .. }) {
                summary.effects += 1;
            }
        }
    }
    // Compact graph ownership ends before the second, one-function-at-a-time fact pass.
    drop(calls);
    drop(edges);
    if let FlowGoal::Effects { profile, address } = &request.goal
        && *profile != FlowEffectProfile::Calls
    {
        let mut reader = project.analysis_reader(memory);
        let _envelope = memory.reserve(1024 * 1024, c.position())?;
        for (i, node) in nodes.iter().enumerate() {
            c.checkpoint(1)?;
            if reached[i].is_none() {
                continue;
            }
            let lease = reader.analysis(&node.function.analysis, c)?;
            summary.facts_passes += 1;
            let facts = crate::research::load_records(&lease.records, memory, c)?;
            for (record, fact) in facts.iter().enumerate() {
                c.checkpoint(1)?;
                let (value, width) = match fact {
                    FunctionRecord::MemoryAccess { address, width, .. }
                    | FunctionRecord::CalleeEffect { address, width, .. } => (address, *width),
                    _ => continue,
                };
                if !matches!(width, 1 | 2 | 4 | 8) {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "invalid flow memory width",
                    ));
                }
                let matched = address.and_then(|at| address_match(value, width, at));
                if matched == Some(false) {
                    continue;
                }
                let _capacity = memory.reserve(
                    fact.allocated_bytes() + std::mem::size_of::<FunctionRecord>() as u64,
                    c.position(),
                )?;
                emit(
                    &FlowRecord::Effect {
                        analysis: node.function.analysis.clone(),
                        record: record as u64,
                        fact: Box::new(fact.clone()),
                        address_match: matched,
                    },
                    c,
                )?;
                summary.effects += 1;
            }
        }
    }
    Ok(summary)
}
fn address_match(value: &AbstractValue, width: u8, at: u32) -> Option<bool> {
    match value {
        AbstractValue::Constant { value }
        | AbstractValue::ImageAddress { address: value }
        | AbstractValue::ScopedAddress { address: value, .. } => {
            Some(*value <= at && u64::from(at) < u64::from(*value) + u64::from(width))
        }
        AbstractValue::Alternatives { values } => {
            let mut any = false;
            let mut all = true;
            for value in values.values() {
                let hit = address_match(&value.as_value(), width, at)?;
                any |= hit;
                all &= hit;
            }
            if all {
                Some(true)
            } else if any {
                None
            } else {
                Some(false)
            }
        }
        _ => None,
    }
}

/// Recheck a finite conditional path through the same saved edge resolver as navigation.
pub(crate) fn validate_path(
    project: &Project,
    proposal: &KnowledgeProposal,
    path: &ReviewedPath,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<ArtifactId>> {
    let mut ids = Vec::with_capacity(path.hops.len() + 1);
    ids.push(path.hops[0].caller.clone());
    ids.extend(path.hops.iter().map(|h| h.callee.clone()));
    let mut keys = AdmittedVec::new(memory);
    for (i, hop) in path.hops.iter().enumerate() {
        keys.push((&hop.caller, hop.record, i), c.position())?;
    }
    c.checkpoint(path.hops.len() as u64 * 7)?;
    keys.sort_unstable_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    let mut seen = 0u64;
    let mut roots = Vec::with_capacity(2 * ids.len());
    let scope = NavigationScope {
        revision: proposal.occurrence.revision.clone(),
        publications: vec![],
        analyses: ids,
        knowledge: None,
    };
    crate::navigation::query_observed(
        project,
        &NavigationQuery {
            scope,
            filter: NavigationFilter::Calls {
                function: None,
                direction: CallDirection::Callees,
            },
        },
        memory,
        c,
        &mut |function, manifest, _, _| {
            if function.analysis == path.hops[0].caller
                && (function.location.source != proposal.occurrence.source
                    || *function.location.selector.object() != proposal.occurrence.object
                    || proposal
                        .occurrence
                        .symbol
                        .as_ref()
                        .is_some_and(|s| function.location.selector.symbol() != Some(s)))
            {
                return Err(invalid("reviewed path root differs from its occurrence"));
            }
            roots.push(function.analysis.as_str().parse()?);
            roots.push(manifest.records.clone());
            Ok(())
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
                c.checkpoint(keys.len().max(1).ilog2() as u64 + 1)?;
                if let Ok(index) =
                    keys.binary_search_by(|k| k.0.cmp(&caller.analysis).then(k.1.cmp(record)))
                {
                    let ordinal = keys[index].2;
                    if issue.is_some()
                        || candidates.len() != 1
                        || candidates[0].analysis != path.hops[ordinal].callee
                    {
                        return Err(invalid(
                            "reviewed path hop is unresolved, ambiguous or names another target",
                        ));
                    }
                    seen |= 1u64 << ordinal;
                }
            }
            Ok(())
        },
    )?;
    let expected = u64::MAX >> (64 - path.hops.len());
    if seen != expected {
        return Err(invalid(
            "reviewed path hop has no matching saved call record",
        ));
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn address_filter_keeps_partial_alternatives_and_unknown_accesses() {
        assert_eq!(
            address_match(&AbstractValue::Constant { value: 0x1000 }, 4, 0x1003),
            Some(true)
        );
        assert_eq!(
            address_match(&AbstractValue::Constant { value: 0x1000 }, 4, 0x1004),
            Some(false)
        );
        assert_eq!(address_match(&AbstractValue::Unknown, 4, 0x1000), None);
        assert_eq!(
            address_match(&AbstractValue::Constant { value: u32::MAX }, 4, 0),
            Some(false)
        );
        let values = ValueAlternatives::new(vec![
            ValueAlternative::Constant { value: 0x1000 },
            ValueAlternative::Constant { value: 0x2000 },
        ])
        .unwrap();
        assert_eq!(
            address_match(&AbstractValue::Alternatives { values }, 4, 0x1000),
            None
        );
    }
}
