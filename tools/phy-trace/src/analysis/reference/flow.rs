//! Bounded CFG exploration and scoped call composition.

use super::*;

fn reference_intrinsic_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Option<FunctionAnalysis> {
    match symbol.name.as_str() {
        "ets_delay_us" => Some(FunctionAnalysis {
            symbol: symbol.name.clone(),
            events: Vec::new(),
            reference_events: vec![DraftReferenceEvent::DelayMicros {
                micros: SymbolicValue::input(0),
            }],
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            reference_flow: None,
            unresolved_branch: None,
        }),
        _ => None,
    }
}

pub(super) struct ReferenceCalleeContext<'a> {
    pub(super) symbols_by_address: &'a BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    pub(super) relocated_calls: &'a StructuralRelocatedCalls,
    pub(super) external_pointer_cells: &'a BTreeMap<u32, external_abi::Table>,
    pub(super) svd: &'a MmioRegisterMap,
}

#[derive(Clone, Debug)]
struct ReferencePath {
    events: VecDeque<DraftReferenceEvent>,
    return_value: SymbolicValue,
}

fn build_reference_flow(
    mut paths: Vec<ReferencePath>,
) -> std::result::Result<DraftReferenceFlow, String> {
    if paths.is_empty() {
        return Err("bounded branch exploration produced no complete paths".to_owned());
    }

    let mut events = Vec::new();
    loop {
        let Some(first) = paths[0].events.front().cloned() else {
            if paths.iter().any(|path| !path.events.is_empty()) {
                return Err("symbolic paths do not share a structured event boundary".to_owned());
            }
            let return_value = paths[0].return_value.clone();
            if paths.iter().any(|path| path.return_value != return_value) {
                return Err("symbolic paths merge with incompatible return states".to_owned());
            }
            return Ok(DraftReferenceFlow {
                events,
                terminator: DraftReferenceTerminator::Return(return_value),
            });
        };

        if let DraftReferenceEvent::BranchDecision { condition, .. } = first {
            let mut taken_paths = Vec::new();
            let mut not_taken_paths = Vec::new();
            for mut path in paths {
                let Some(DraftReferenceEvent::BranchDecision {
                    condition: path_condition,
                    taken,
                }) = path.events.pop_front()
                else {
                    return Err("symbolic paths diverge before a common branch boundary".to_owned());
                };
                if path_condition != condition {
                    return Err(format!(
                        "symbolic paths disagree about branch condition at {:#010x}",
                        condition.site
                    ));
                }
                if taken {
                    taken_paths.push(path);
                } else {
                    not_taken_paths.push(path);
                }
            }
            if taken_paths.is_empty() || not_taken_paths.is_empty() {
                return Err(format!(
                    "branch exploration did not cover both outcomes at {:#010x}",
                    condition.site
                ));
            }
            return Ok(DraftReferenceFlow {
                events,
                terminator: DraftReferenceTerminator::Branch {
                    condition,
                    taken: Box::new(build_reference_flow(taken_paths)?),
                    not_taken: Box::new(build_reference_flow(not_taken_paths)?),
                },
            });
        }

        if paths.iter().any(|path| path.events.front() != Some(&first)) {
            return Err("symbolic paths have incompatible observable event prefixes".to_owned());
        }
        for path in &mut paths {
            path.events.pop_front();
        }
        events.push(first);
    }
}

pub(super) fn explore_reference_flow(
    symbol: &artifact::ArtifactSymbolDefinition,
    svd: &MmioRegisterMap,
    relocated_calls: &StructuralRelocatedCalls,
    external_pointer_cells: &BTreeMap<u32, external_abi::Table>,
    specialized_arguments: Option<&[SymbolicValue; 8]>,
) -> std::result::Result<DraftReferenceFlow, String> {
    const MAX_COMPLETE_PATHS: usize = 64;
    const MAX_EXPLORED_STATES: usize = MAX_COMPLETE_PATHS * 2 - 1;
    const MAX_BRANCH_DECISIONS: usize = 12;

    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut paths = Vec::new();
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        explored_states += 1;
        if explored_states > MAX_EXPLORED_STATES {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {MAX_COMPLETE_PATHS} complete paths"
            ));
        }
        let trace = trace_binary_symbol_with_branches(
            symbol,
            svd,
            relocated_calls,
            external_pointer_cells,
            specialized_arguments,
            &forced_branches,
        )
        .map_err(|error| error.to_string())?;

        let typed_calls = trace
            .reference_events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    DraftReferenceEvent::Call { .. }
                        | DraftReferenceEvent::TailCall { .. }
                        | DraftReferenceEvent::DiagnosticCall { .. }
                )
            })
            .count();
        let call_blockers = trace
            .blockers
            .iter()
            .filter(|blocker| blocker.starts_with("call/jump instruction"))
            .count();

        if let Some(branch) = trace.unresolved_branch {
            let branch_blockers = trace
                .blockers
                .iter()
                .filter(|blocker| blocker.starts_with("input-dependent control-flow"))
                .count();
            if !trace.reference_blockers.is_empty()
                || branch_blockers != 1
                || trace.blockers.len() != call_blockers + branch_blockers
                || typed_calls != call_blockers
            {
                return Err(format!(
                    "path to branch {:#010x} has unsupported effects: {}",
                    branch.site,
                    trace
                        .blockers
                        .iter()
                        .chain(&trace.reference_blockers)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            if forced_branches.len() >= MAX_BRANCH_DECISIONS {
                return Err(format!(
                    "symbolic CFG exceeds the limit of {MAX_BRANCH_DECISIONS} branch decisions per path"
                ));
            }
            for taken in [false, true] {
                let mut next = forced_branches.clone();
                if next.insert(branch.site, taken).is_some() {
                    return Err(format!(
                        "symbolic CFG revisits branch {:#010x}; loops are not supported",
                        branch.site
                    ));
                }
                if queued.insert(next.clone()) {
                    queue.push_back(next);
                }
            }
            continue;
        }

        if !trace.reference_blockers.is_empty()
            || trace.blockers.len() != call_blockers
            || typed_calls != call_blockers
        {
            return Err(format!(
                "symbolic path has unsupported effects: {}",
                trace
                    .blockers
                    .iter()
                    .chain(&trace.reference_blockers)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        paths.push(ReferencePath {
            events: trace.reference_events.into(),
            return_value: trace.return_value,
        });
        if paths.len() > MAX_COMPLETE_PATHS {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {MAX_COMPLETE_PATHS} complete paths"
            ));
        }
    }

    build_reference_flow(paths)
}

pub(super) fn resolve_reference_callee(
    target: u32,
    site: u32,
    arguments: &[SymbolicValue; 8],
    context: &ReferenceCalleeContext<'_>,
    visiting: &mut BTreeSet<u32>,
) -> std::result::Result<(String, FunctionAnalysis), String> {
    let callee = context
        .symbols_by_address
        .get(&target)
        .ok_or_else(|| format!("unresolved-call at {site:#010x} to {target:#010x}"))?;
    if let Some(trace) = reference_intrinsic_trace(callee) {
        return Ok((callee.name.clone(), trace));
    }
    if !visiting.insert(target) {
        return Err(format!("recursive-call at {site:#010x} to {}", callee.name));
    }
    let result = resolve_reference_trace(
        callee,
        context.symbols_by_address,
        context.relocated_calls,
        context.external_pointer_cells,
        Some(arguments),
        context.svd,
        visiting,
    )
    .map_err(|error| format!("callee-decode at {site:#010x}: {}: {error}", callee.name));
    visiting.remove(&target);
    let trace = result?;
    if !trace.is_reference_eligible() {
        return Err(format!(
            "callee-ineligible at {site:#010x}: {}",
            callee.name
        ));
    }
    Ok((callee.name.clone(), trace))
}

pub(super) fn trace_into_reference_flow(mut trace: FunctionAnalysis) -> DraftReferenceFlow {
    trace.reference_flow.take().unwrap_or(DraftReferenceFlow {
        events: std::mem::take(&mut trace.reference_events),
        terminator: DraftReferenceTerminator::Return(trace.return_value),
    })
}

pub(super) fn compose_calls_in_reference_flow(
    mut flow: DraftReferenceFlow,
    context: &ReferenceCalleeContext<'_>,
    visiting: &mut BTreeSet<u32>,
    dependencies: &mut Vec<String>,
) -> std::result::Result<DraftReferenceFlow, String> {
    let mut events = Vec::with_capacity(flow.events.len());
    for event in flow.events {
        let (token, site, target, arguments) = match event {
            DraftReferenceEvent::Call {
                token,
                site,
                target,
                arguments,
            }
            | DraftReferenceEvent::TailCall {
                token,
                site,
                target,
                arguments,
            } => (token, site, target, arguments),
            DraftReferenceEvent::BranchDecision { condition, .. } => {
                return Err(format!(
                    "branch decision at {:#010x} escaped structured flow assembly",
                    condition.site
                ));
            }
            other => {
                events.push(other);
                continue;
            }
        };

        let (callee_name, callee_trace) =
            resolve_reference_callee(target, site, &arguments, context, visiting)?;
        let result_modeled = callee_trace.reference_exit_a0_modeled();
        dependencies.push(callee_name.clone());
        dependencies.extend(callee_trace.reference_dependencies.iter().cloned());
        events.push(DraftReferenceEvent::ComposedCall {
            token,
            symbol: callee_name,
            arguments,
            flow: Box::new(trace_into_reference_flow(callee_trace)),
            result_modeled,
        });
    }
    flow.events = events;
    flow.terminator = match flow.terminator {
        DraftReferenceTerminator::Return(value) => DraftReferenceTerminator::Return(value),
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => DraftReferenceTerminator::Branch {
            condition,
            taken: Box::new(compose_calls_in_reference_flow(
                *taken,
                context,
                visiting,
                dependencies,
            )?),
            not_taken: Box::new(compose_calls_in_reference_flow(
                *not_taken,
                context,
                visiting,
                dependencies,
            )?),
        },
    };
    Ok(flow)
}
