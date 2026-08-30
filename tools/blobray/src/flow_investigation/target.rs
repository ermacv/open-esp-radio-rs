use std::collections::{BTreeMap, BTreeSet};

use crate::{ProjectSpec, Result, artifacts};

use super::{
    EvidenceLevel, FlowBlocker, FlowClaims, FlowEffectEvidence, FlowInvestigationReport,
    FlowLimits, FlowStatus, FlowStepEvidence, FlowTargetRequest, MAX_LOADED_FUNCTIONS,
    TargetFlowRequest,
    project_graph::{PROJECT_ASSOCIATED, ProjectGraph},
    value::{compose_call_arguments, root_domains},
};

pub(super) fn investigate(
    request: TargetFlowRequest<'_>,
    project: &ProjectSpec,
) -> Result<FlowInvestigationReport> {
    let graph = ProjectGraph::open(project)?;
    investigate_with_graph(request, &graph)
}

pub(super) fn investigate_with_graph(
    request: TargetFlowRequest<'_>,
    graph: &ProjectGraph<'_>,
) -> Result<FlowInvestigationReport> {
    let roots = graph.root_identities(request.source, request.root_symbol);
    if roots.len() != 1 {
        return Err(crate::Error::invalid(format!(
            "generated project IR contains {} roots for {}:{}; use an exact identity or regenerate project analysis",
            roots.len(),
            request.source,
            request.root_symbol
        )));
    }
    let root = roots.iter().next().expect("one root was checked");
    let targets = graph.target_identities(&request.target)?;
    if targets.is_empty() {
        return Ok(not_reached(
            graph,
            root,
            &request.target,
            request.max_depth,
            "flow target is absent from the generated project IR",
        ));
    }
    let search = graph.shortest_path(root, &targets, request.max_depth)?;
    let Some(path) = search.path.as_ref() else {
        let mut report = not_reached(
            graph,
            root,
            &request.target,
            request.max_depth,
            "no bounded project call-graph path reaches the requested target",
        );
        report.limits.visited_nodes = search.visited_nodes;
        report.limits.examined_edges = search.examined_edges;
        report.limits.reached = search.limit.map(str::to_owned);
        if let Some(limit) = search.limit {
            report.status = FlowStatus::Incomplete;
            report.blockers.push(limit_blocker(limit));
        }
        return Ok(report);
    };
    compose_path(
        graph,
        root,
        &request.target,
        path,
        request.max_depth,
        request.max_loaded_functions.min(MAX_LOADED_FUNCTIONS),
        search.visited_nodes,
        search.examined_edges,
        search.limit,
    )
}

#[allow(clippy::too_many_arguments)]
fn compose_path(
    graph: &ProjectGraph<'_>,
    root: &str,
    target: &FlowTargetRequest<'_>,
    path: &[artifacts::StoredGraphEdge],
    max_depth: usize,
    max_loaded_functions: usize,
    visited_nodes: usize,
    examined_edges: usize,
    reached_limit: Option<&'static str>,
) -> Result<FlowInvestigationReport> {
    let sink = path
        .last()
        .map_or(root, |edge| edge.callee.as_str())
        .to_owned();
    let mut selected = BTreeSet::from([root.to_owned(), sink.clone()]);
    selected.extend(path.iter().map(|edge| edge.caller.clone()));
    let mut blockers = Vec::new();
    if selected.len() > max_loaded_functions {
        blockers.push(limit_blocker("max-loaded-functions"));
        selected = selected.into_iter().take(max_loaded_functions).collect();
    }
    let mut functions = BTreeMap::new();
    for identity in selected {
        match graph.function(&identity)? {
            Some((function, profile)) => {
                functions.insert(identity, (function, profile.output.display().to_string()));
            }
            None => blockers.push(FlowBlocker::manual(
                "missing-function-record",
                format!("graph identity {identity:?} has no indexed function record"),
                "regenerate the linked-IR profile",
            )),
        }
    }

    let mut domains = root_domains();
    let mut steps = Vec::new();
    for (ordinal, edge) in path.iter().enumerate() {
        let Some((caller, origin)) = functions.get(&edge.caller) else {
            blockers.push(FlowBlocker::manual(
                "missing-caller-record",
                format!("cannot load caller {}", edge.caller),
                "regenerate the linked-IR profile or reduce the requested path",
            ));
            continue;
        };
        let Some(call) = caller
            .calls
            .iter()
            .find(|call| call_matches_edge(call, edge))
        else {
            blockers.push(FlowBlocker::manual(
                "missing-call-fact",
                format!(
                    "graph edge {} -> {} has no matching call fact",
                    edge.caller, edge.callee
                ),
                "regenerate linked IR and inspect the caller's lossless body",
            ));
            continue;
        };
        let (next, arguments) = compose_call_arguments(caller, call, &domains);
        for argument in &arguments {
            if argument.local.starts_with("private-stack:") && argument.pointee.is_empty() {
                blockers.push(FlowBlocker::manual(
                    "unresolved-stack-object",
                    format!(
                        "{} -> {} passes an unresolved private stack object in a{}",
                        edge.caller, edge.callee, argument.position
                    ),
                    "add a reviewed external output model or inspect the caller's stack stores",
                ));
            }
        }
        if call.arguments.is_empty() {
            blockers.push(FlowBlocker::manual(
                "missing-abi-arguments",
                format!(
                    "{} -> {} has no recovered ABI arguments",
                    edge.caller, edge.callee
                ),
                "review the call boundary and its ABI model",
            ));
        }
        if edge.kind == PROJECT_ASSOCIATED {
            blockers.push(FlowBlocker::manual(
                "project-associated-call",
                format!(
                    "{} -> {} is associated through one exported project symbol, not authoritative linker selection",
                    edge.caller, edge.callee
                ),
                "provide an authoritative linked ELF containing both functions to promote this edge",
            ));
        }
        domains = next;
        let evidence = if edge.kind == PROJECT_ASSOCIATED {
            EvidenceLevel::Reviewed
        } else {
            EvidenceLevel::Observed
        };
        steps.push(FlowStepEvidence {
            ordinal,
            evidence,
            context_evidence: evidence,
            context: "synchronous".to_owned(),
            caller: edge.caller.clone(),
            callee: edge.callee.clone(),
            site: edge.site,
            kind: edge.kind.clone(),
            tail: call.tail(),
            argument_shapes: call.argument_shapes(),
            arguments,
            guards: call.guard_expressions(),
            origin: origin.clone(),
        });
    }
    if let Some(limit) = reached_limit {
        blockers.push(limit_blocker(limit));
    }

    let mut effects = Vec::new();
    if let Some((function, _)) = functions.get(&sink) {
        for effect in &function.instruction_effects {
            let Some((access, width, address, register, value)) = effect.mmio() else {
                continue;
            };
            if !target_matches_mmio(target, address, register) {
                continue;
            }
            let evidence = FlowEffectEvidence {
                kind: "mmio".to_owned(),
                evidence: EvidenceLevel::Observed,
                function: sink.clone(),
                site: Some(effect.site()),
                operation: None,
                detail: format!("{access}{width} {register}"),
                constant: None,
                access: Some(access.to_owned()),
                width: Some(width),
                address: Some(address),
                register: Some(register.to_owned()),
                value: value.map(str::to_owned),
                origin_path: None,
            };
            effects.push(evidence);
        }
        for effect in &function.mmio_accesses {
            if !target_matches_effect(target, effect)
                || effects.iter().any(|candidate| {
                    candidate.access.as_deref() == Some(effect.access())
                        && candidate.width == Some(effect.width())
                        && candidate.address == Some(effect.address)
                        && candidate.register.as_deref() == Some(effect.register())
                        && candidate.value.as_deref() == effect.value()
                })
            {
                continue;
            }
            effects.push(FlowEffectEvidence {
                kind: "mmio".to_owned(),
                evidence: EvidenceLevel::Observed,
                function: sink.clone(),
                site: None,
                operation: None,
                detail: format!(
                    "{}{} {}",
                    effect.access(),
                    effect.width(),
                    effect.register()
                ),
                constant: None,
                access: Some(effect.access().to_owned()),
                width: Some(effect.width()),
                address: Some(effect.address),
                register: Some(effect.register().to_owned()),
                value: effect.value().map(str::to_owned),
                origin_path: None,
            });
        }
        if !function.completeness.executable_complete {
            blockers.push(FlowBlocker::manual(
                "incomplete-sink",
                format!("sink function {sink} has semantic blockers"),
                "run `inspect function` for the sink and satisfy its required models",
            ));
        }
    }
    effects.sort_by(|left, right| {
        (
            &left.function,
            left.site,
            &left.kind,
            left.address,
            &left.detail,
        )
            .cmp(&(
                &right.function,
                right.site,
                &right.kind,
                right.address,
                &right.detail,
            ))
    });
    effects.dedup();

    let status = if blockers.is_empty() {
        FlowStatus::Complete
    } else {
        FlowStatus::Incomplete
    };
    let (profile, linked_ir) = graph.profile_labels();
    Ok(FlowInvestigationReport {
        schema_version: 5,
        command: "inspect flow",
        mode: "target",
        status,
        profile,
        linked_ir,
        root: root.to_owned(),
        target_kind: Some(target_kind(target).to_owned()),
        target: Some(target_label(target)),
        route: None,
        claims: FlowClaims {
            structural_navigation: true,
            ..FlowClaims::default()
        },
        limits: FlowLimits {
            max_depth,
            max_loaded_functions,
            visited_nodes,
            examined_edges,
            loaded_functions: functions.len(),
            reached: reached_limit.map(str::to_owned),
            ..FlowLimits::new(max_depth)
        },
        steps,
        effects,
        publications: Vec::new(),
        memory_slice: Vec::new(),
        rust_boundaries: Vec::new(),
        blockers,
    })
}

fn not_reached(
    graph: &ProjectGraph<'_>,
    root: &str,
    target: &FlowTargetRequest<'_>,
    max_depth: usize,
    message: &str,
) -> FlowInvestigationReport {
    let (profile, linked_ir) = graph.profile_labels();
    FlowInvestigationReport {
        schema_version: 5,
        command: "inspect flow",
        mode: "target",
        status: FlowStatus::NotReached,
        profile,
        linked_ir,
        root: root.to_owned(),
        target_kind: Some(target_kind(target).to_owned()),
        target: Some(target_label(target)),
        route: None,
        claims: FlowClaims::default(),
        limits: FlowLimits::new(max_depth),
        steps: Vec::new(),
        effects: Vec::new(),
        publications: Vec::new(),
        memory_slice: Vec::new(),
        rust_boundaries: Vec::new(),
        blockers: vec![FlowBlocker::manual(
            "target-not-reached",
            message,
            "increase --max-depth, regenerate project analysis, or inspect the missing boundary",
        )],
    }
}

fn call_matches_edge(call: &artifacts::StoredCall, edge: &artifacts::StoredGraphEdge) -> bool {
    if call.site != edge.site {
        return false;
    }
    if edge.kind == PROJECT_ASSOCIATED {
        return call.kind == "unresolved" && call.project_symbol().is_some();
    }
    call.target == edge.callee && call.kind == edge.kind
}

fn target_matches_effect(
    target: &FlowTargetRequest<'_>,
    effect: &artifacts::StoredMmioAccess,
) -> bool {
    match target {
        FlowTargetRequest::Function(_) => true,
        FlowTargetRequest::Register(register) => effect.register() == *register,
        FlowTargetRequest::Address(address) => effect.address == *address,
    }
}

fn target_matches_mmio(target: &FlowTargetRequest<'_>, address: u32, register: &str) -> bool {
    match target {
        FlowTargetRequest::Function(_) => true,
        FlowTargetRequest::Register(candidate) => register == *candidate,
        FlowTargetRequest::Address(candidate) => address == *candidate,
    }
}

fn target_kind(target: &FlowTargetRequest<'_>) -> &'static str {
    match target {
        FlowTargetRequest::Function(_) => "function",
        FlowTargetRequest::Register(_) => "register",
        FlowTargetRequest::Address(_) => "address",
    }
}

fn target_label(target: &FlowTargetRequest<'_>) -> String {
    match target {
        FlowTargetRequest::Function(value) | FlowTargetRequest::Register(value) => {
            (*value).to_owned()
        }
        FlowTargetRequest::Address(value) => format!("{value:#010x}"),
    }
}

pub(super) fn limit_blocker(limit: &str) -> FlowBlocker {
    FlowBlocker::manual(
        "analysis-limit",
        format!("flow investigation reached {limit}"),
        "narrow the root/target or raise the explicit bound after measuring resource use",
    )
}
