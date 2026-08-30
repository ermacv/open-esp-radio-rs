//! Bounded CFG exploration and scoped call composition.

use super::*;

pub(super) struct ReferenceCalleeContext<'a> {
    pub(super) symbols_by_address: &'a BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    pub(super) relocated_calls: &'a StructuralRelocatedCalls,
    pub(super) pointer_context: &'a StructuralPointerContext,
    pub(super) svd: &'a MmioMap,
    pub(super) budget: StructuralTraceBudget,
    pub(super) memo: &'a ReferenceAnalysisMemo,
}

#[derive(Clone, Debug)]
struct ReferencePath {
    events: VecDeque<DraftReferenceEvent>,
    return_value: SymbolicValue,
}

fn event_preview(event: Option<&DraftReferenceEvent>) -> String {
    let rendered = event.map_or_else(|| "<end-of-path>".to_owned(), |event| format!("{event:?}"));
    const LIMIT: usize = 192;
    let mut preview = rendered.chars().take(LIMIT).collect::<String>();
    if rendered.chars().count() > LIMIT {
        preview.push_str("...");
    }
    preview
}

pub(super) struct ExploredReferenceFlow {
    pub(super) flow: DraftReferenceFlow,
    pub(super) incomplete_effects: Vec<String>,
    pub(super) reference_dependencies: Vec<String>,
    pub(super) located_events: Vec<LocatedObservableEvent>,
    pub(super) located_reference_events: Vec<LocatedReferenceEvent>,
}

fn preserves_partial_reference_flow(blocker: &str) -> bool {
    [
        "unmodeled-memory-load at ",
        "unmodeled-reviewed-external-call at ",
        "unresolved-indirect-call at ",
        "unresolved-call-relocation at ",
        "unresolved-memory-write at ",
        "base ",
    ]
    .iter()
    .any(|prefix| blocker.starts_with(prefix))
}

fn preserves_partial_direct_flow(blocker: &str) -> bool {
    // Floating-point instructions are classified as linear control flow by
    // the decoder. Unsupported arithmetic may poison later values, and that
    // remains an explicit blocker, but it does not erase independently
    // recovered branch, call and memory evidence from the same path.
    blocker.starts_with("decode-blocker class=floating-point ")
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
            let differing = paths
                .iter()
                .enumerate()
                .filter(|(_, path)| path.events.front() != Some(&first))
                .take(3)
                .map(|(index, path)| format!("path#{index}={}", event_preview(path.events.front())))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "symbolic paths have incompatible observable event prefixes: expected {}; {differing}",
                event_preview(Some(&first))
            ));
        }
        for path in &mut paths {
            path.events.pop_front();
        }
        events.push(first);
    }
}

pub(super) fn explore_reference_flow(
    symbol: &artifact::ArtifactSymbolDefinition,
    context: &ReferenceCalleeContext<'_>,
    specialized_arguments: Option<&Rv32CallArguments>,
    visiting: &mut BTreeSet<u32>,
) -> std::result::Result<ExploredReferenceFlow, String> {
    let max_complete_paths = 64;
    let max_explored_states = max_complete_paths * 2 - 1;
    let max_branch_decisions = 12;

    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut paths = Vec::new();
    let mut normalized_paths = Vec::new();
    let mut incomplete_effects = BTreeSet::new();
    let mut normalized_dependencies = BTreeSet::new();
    let mut located_events = Vec::new();
    let mut located_reference_events = Vec::new();
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        explored_states += 1;
        if explored_states > max_explored_states {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {max_complete_paths} complete paths"
            ));
        }
        let trace = trace_binary_symbol_with_branches_bounded(
            symbol,
            context.svd,
            context.relocated_calls,
            context.pointer_context,
            specialized_arguments,
            &forced_branches,
            context.budget,
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
        let opaque_call_blockers = trace
            .reference_blockers
            .iter()
            .filter(|blocker| {
                blocker.starts_with("unresolved-indirect-call at ")
                    || blocker.starts_with("unresolved-call-relocation at ")
            })
            .count();
        let reference_only_blockers = trace
            .blockers
            .iter()
            .filter(|blocker| is_reference_only_blocker(blocker))
            .count();
        let partial_direct_blockers = trace
            .blockers
            .iter()
            .filter(|blocker| preserves_partial_direct_flow(blocker))
            .count();

        if let Some(branch) = trace.unresolved_branch {
            let branch_blockers = trace
                .blockers
                .iter()
                .filter(|blocker| blocker.starts_with("input-dependent control-flow"))
                .count();
            let unsupported_reference_blockers = trace
                .reference_blockers
                .iter()
                .filter(|blocker| !preserves_partial_reference_flow(blocker))
                .count();
            if unsupported_reference_blockers != 0
                || branch_blockers != 1
                || trace.blockers.len()
                    != call_blockers
                        + branch_blockers
                        + reference_only_blockers
                        + partial_direct_blockers
                || typed_calls + opaque_call_blockers != call_blockers
            {
                return Err(format!(
                    "path to branch {:#010x} has unsupported effects (unsupported-reference={unsupported_reference_blockers}, branch-blockers={branch_blockers}, blockers={}, call-blockers={call_blockers}, typed-calls={typed_calls}, opaque-calls={opaque_call_blockers}, reference-only={reference_only_blockers}): {}",
                    branch.site,
                    trace.blockers.len(),
                    trace
                        .blockers
                        .iter()
                        .chain(&trace.reference_blockers)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            if forced_branches.len() >= max_branch_decisions {
                return Err(format!(
                    "symbolic CFG exceeds the limit of {max_branch_decisions} branch decisions per path"
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

        let unsupported_reference_blockers = trace
            .reference_blockers
            .iter()
            .filter(|blocker| !preserves_partial_reference_flow(blocker))
            .count();
        let partial_direct_blockers = trace
            .blockers
            .iter()
            .filter(|blocker| preserves_partial_direct_flow(blocker))
            .count();
        if unsupported_reference_blockers != 0
            || trace.blockers.len()
                != call_blockers + reference_only_blockers + partial_direct_blockers
            || typed_calls + opaque_call_blockers != call_blockers
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
        incomplete_effects.extend(
            trace
                .blockers
                .iter()
                .filter(|blocker| preserves_partial_direct_flow(blocker))
                .cloned(),
        );
        incomplete_effects.extend(
            trace
                .reference_blockers
                .iter()
                .filter(|blocker| preserves_partial_reference_flow(blocker))
                .cloned(),
        );
        for event in &trace.located_events {
            if !located_events.contains(event) {
                located_events.push(event.clone());
            }
        }
        for event in &trace.located_reference_events {
            if !located_reference_events.contains(event) {
                located_reference_events.push(event.clone());
            }
        }
        // A complete forced path is straight-line even when the source
        // function is not. Resolve its calls and private stack before merging
        // paths back into a structured CFG. This keeps stack state path-local
        // and avoids either sharing mutations across branches or rejecting a
        // safe memory intrinsic merely because its caller has a branch.
        let uncomposed = trace.clone();
        let normalized = if typed_calls != 0
            || trace.reference_events.iter().any(|event| {
                matches!(
                    event,
                    DraftReferenceEvent::PrivateStackLoad { .. }
                        | DraftReferenceEvent::PrivateStackStore { .. }
                )
            }) {
            match flatten_reference_trace(trace, context, specialized_arguments, visiting) {
                Ok(composed) => {
                    if let Some(blocker) = composed.reference_blockers.iter().find(|blocker| {
                        blocker.starts_with("call-summary-flattening: ")
                            || blocker.starts_with("call-return-flattening: ")
                    }) {
                        incomplete_effects.insert(format!(
                            "path-local-composition: {blocker}; retained uncomposed path evidence"
                        ));
                        None
                    } else {
                        Some(composed)
                    }
                }
                Err(error) => {
                    incomplete_effects.insert(format!(
                        "path-local-composition: {error}; retained uncomposed path evidence"
                    ));
                    None
                }
            }
        } else {
            Some(trace)
        };
        normalized_paths.push(normalized.map(|trace| {
            normalized_dependencies.extend(trace.reference_dependencies.iter().cloned());
            ReferencePath {
                events: trace.reference_events.into(),
                return_value: trace.return_value,
            }
        }));
        paths.push(ReferencePath {
            events: uncomposed.reference_events.into(),
            return_value: uncomposed.return_value,
        });
        if paths.len() > max_complete_paths {
            return Err(format!(
                "symbolic CFG exceeds the exploration limit of {max_complete_paths} complete paths"
            ));
        }
    }

    let all_paths_normalized = normalized_paths.iter().all(Option::is_some);
    let paths = if all_paths_normalized {
        normalized_paths.into_iter().flatten().collect()
    } else {
        paths
    };
    Ok(ExploredReferenceFlow {
        flow: build_reference_flow(paths)?,
        incomplete_effects: incomplete_effects.into_iter().collect(),
        reference_dependencies: if all_paths_normalized {
            normalized_dependencies.into_iter().collect()
        } else {
            Vec::new()
        },
        located_events,
        located_reference_events,
    })
}

pub(super) fn resolve_reference_callee(
    target: u32,
    site: u32,
    arguments: &[SymbolicValue],
    context: &ReferenceCalleeContext<'_>,
    visiting: &mut BTreeSet<u32>,
) -> std::result::Result<(String, FunctionAnalysis), String> {
    let arguments: &Rv32CallArguments = arguments.try_into().map_err(|_| {
        format!(
            "call at {site:#010x} carries {} modeled arguments; RV32 requires {RV32_MODELED_ARGUMENT_COUNT}",
            arguments.len()
        )
    })?;
    let callee = context
        .symbols_by_address
        .get(&target)
        .ok_or_else(|| format!("unresolved-call at {site:#010x} to {target:#010x}"))?;
    if let Some(function) = context
        .pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.standard_memory_function)(&callee.name))
        && let Some(trace) = standard_memory_intrinsic_trace(function, callee, arguments)
    {
        return trace.map(|trace| (callee.name.clone(), trace));
    }
    if let Some(trace) = context.memo.get(target, arguments) {
        if trace.is_reference_eligible() {
            return Ok((callee.name.clone(), trace));
        }
        let causes = trace.reference_failure_reasons().join(" | ");
        return Err(format!(
            "callee-ineligible at {site:#010x}: {} [causes: {causes}]",
            callee.name,
        ));
    }
    if !visiting.insert(target) {
        return Err(format!("recursive-call at {site:#010x} to {}", callee.name));
    }
    let result = resolve_reference_trace_with_budget(callee, context, Some(arguments), visiting)
        .map_err(|error| format!("callee-decode at {site:#010x}: {}: {error}", callee.name));
    visiting.remove(&target);
    let trace = result?;
    let failure_reasons =
        (!trace.is_reference_eligible()).then(|| trace.reference_failure_reasons());
    context
        .memo
        .insert_completed(target, arguments, &trace, failure_reasons.as_deref());
    if let Some(causes) = failure_reasons {
        return Err(format!(
            "callee-ineligible at {site:#010x}: {} [causes: {}]",
            callee.name,
            causes.join(" | "),
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
        let event = match event {
            DraftReferenceEvent::BoundedPoll {
                maximum_attempts,
                body,
                repeat_while_mask,
                repeat_while_expected,
                on_exhausted,
            } => {
                events.push(DraftReferenceEvent::BoundedPoll {
                    maximum_attempts,
                    body: Box::new(compose_calls_in_reference_flow(
                        *body,
                        context,
                        visiting,
                        dependencies,
                    )?),
                    repeat_while_mask,
                    repeat_while_expected,
                    on_exhausted,
                });
                continue;
            }
            DraftReferenceEvent::PollFlow {
                body,
                exit_when_mask,
                exit_when_expected,
            } => {
                events.push(DraftReferenceEvent::PollFlow {
                    body: Box::new(compose_calls_in_reference_flow(
                        *body,
                        context,
                        visiting,
                        dependencies,
                    )?),
                    exit_when_mask,
                    exit_when_expected,
                });
                continue;
            }
            DraftReferenceEvent::SymmetricCalibrationSearch {
                token,
                attempts_per_direction,
                settle_micros,
                sample_shift,
                sample_mask,
                accepted_sample,
                initial_read,
                setup,
                write_candidate,
                sample,
            } => {
                events.push(DraftReferenceEvent::SymmetricCalibrationSearch {
                    token,
                    attempts_per_direction,
                    settle_micros,
                    sample_shift,
                    sample_mask,
                    accepted_sample,
                    initial_read: Box::new(compose_calls_in_reference_flow(
                        *initial_read,
                        context,
                        visiting,
                        dependencies,
                    )?),
                    setup: Box::new(compose_calls_in_reference_flow(
                        *setup,
                        context,
                        visiting,
                        dependencies,
                    )?),
                    write_candidate: Box::new(compose_calls_in_reference_flow(
                        *write_candidate,
                        context,
                        visiting,
                        dependencies,
                    )?),
                    sample: Box::new(compose_calls_in_reference_flow(
                        *sample,
                        context,
                        visiting,
                        dependencies,
                    )?),
                });
                continue;
            }
            DraftReferenceEvent::ScratchCall {
                token,
                site,
                target,
                direct,
                arguments,
                scratch_argument,
                scratch_size,
            } => {
                let (callee_name, callee_trace) =
                    resolve_reference_callee(target, site, &arguments, context, visiting)?;
                let result_modeled = callee_trace.reference_exit_return_modeled();
                dependencies.push(callee_name.clone());
                dependencies.extend(callee_trace.reference_dependencies.iter().cloned());
                events.push(DraftReferenceEvent::ComposedCallWithScratch {
                    token,
                    site,
                    symbol: callee_name,
                    direct,
                    arguments,
                    flow: Box::new(trace_into_reference_flow(callee_trace)),
                    result_modeled,
                    scratch_argument,
                    scratch_size,
                });
                continue;
            }
            other => other,
        };
        let (token, site, target, direct, tail, arguments) = match event {
            DraftReferenceEvent::Call {
                token,
                site,
                target,
                direct,
                arguments,
            } => (token, site, target, direct, false, arguments),
            DraftReferenceEvent::TailCall {
                token,
                site,
                target,
                direct,
                arguments,
            } => (token, site, target, direct, true, arguments),
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

        if let Some(callee) = context.symbols_by_address.get(&target)
            && context.pointer_context.summary_hooks.is_some_and(|hooks| {
                <&Rv32CallArguments>::try_from(arguments.as_ref())
                    .ok()
                    .is_some_and(|arguments| {
                        (hooks.wide_signed_divide)(callee, arguments).is_some()
                    })
            })
        {
            dependencies.push(callee.name.clone());
            events.push(DraftReferenceEvent::WideSignedDivide {
                token,
                dividend_low: arguments[0].clone(),
                dividend_high: arguments[1].clone(),
                divisor_low: arguments[2].clone(),
                divisor_high: arguments[3].clone(),
            });
            continue;
        }

        let (callee_name, callee_trace) = if arguments
            .iter()
            .any(|argument| argument.private_stack_offset().is_some())
        {
            return Err(format!(
                "call at {site:#010x} passes caller private stack through symbolic control flow; branch-aware memory composition is required"
            ));
        } else {
            resolve_reference_callee(target, site, &arguments, context, visiting)?
        };
        let result_modeled = callee_trace.reference_exit_return_modeled();
        dependencies.push(callee_name.clone());
        dependencies.extend(callee_trace.reference_dependencies.iter().cloned());
        events.push(DraftReferenceEvent::ComposedCall {
            token,
            site,
            symbol: callee_name,
            direct,
            tail,
            arguments,
            flow: Box::new(trace_into_reference_flow(callee_trace)),
            result_modeled,
        });
    }
    flow.events = events;
    flow.terminator = match flow.terminator {
        DraftReferenceTerminator::Return(value) => DraftReferenceTerminator::Return(value),
        terminator @ DraftReferenceTerminator::FailStop { .. } => terminator,
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
