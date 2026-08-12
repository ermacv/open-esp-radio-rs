use std::collections::{BTreeMap, BTreeSet};

use crate::{ProjectSpec, Result, artifacts};

use super::{
    EvidenceLevel, FlowBlocker, FlowClaims, FlowEffectEvidence, FlowInvestigationReport,
    FlowLimits, FlowStatus, FlowStepEvidence, FlowTargetRequest, MAX_EXAMINED_EDGES,
    MAX_LOADED_FUNCTIONS, MAX_VISITED_NODES, TargetFlowRequest,
    value::{compose_call_arguments, root_domains},
};

pub(super) fn investigate(
    request: TargetFlowRequest<'_>,
    project: &ProjectSpec,
) -> Result<FlowInvestigationReport> {
    let mut reports = Vec::new();
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| {
            profile
                .sources
                .iter()
                .any(|source| source == request.source)
        })
        .filter(|profile| profile.output.is_dir())
    {
        let reader = artifacts::LinkedIrReader::open(&profile.output)?;
        let roots = reader.function_identities(request.source, request.root_symbol);
        if roots.len() != 1 {
            continue;
        }
        let root = &roots[0];
        let targets = target_identities(&reader, &request.target)?;
        if targets.is_empty() {
            reports.push(not_reached(
                profile,
                root,
                &request.target,
                request.max_depth,
                "flow target is absent from the selected linked-IR profile",
            ));
            continue;
        }
        let search = reader.shortest_path_to_any(
            root,
            &targets,
            artifacts::GraphSearchLimits {
                max_depth: request.max_depth,
                max_visited_nodes: MAX_VISITED_NODES,
                max_examined_edges: MAX_EXAMINED_EDGES,
            },
        )?;
        let Some(path) = search.path.as_ref() else {
            let mut report = not_reached(
                profile,
                root,
                &request.target,
                request.max_depth,
                "no structural call-graph path reaches the requested target",
            );
            report.limits.visited_nodes = search.visited_nodes;
            report.limits.examined_edges = search.examined_edges;
            report.limits.reached = search.limit.map(str::to_owned);
            if let Some(limit) = search.limit {
                report.status = FlowStatus::Incomplete;
                report.blockers.push(limit_blocker(limit));
            }
            reports.push(report);
            continue;
        };
        reports.push(compose_path(
            profile,
            &reader,
            root,
            &request.target,
            path,
            request.max_depth,
            search.visited_nodes,
            search.examined_edges,
            search.limit,
        )?);
    }

    reports
        .into_iter()
        .min_by_key(|report| {
            (
                match report.status {
                    FlowStatus::Complete => 0,
                    FlowStatus::Incomplete => 1,
                    FlowStatus::NotReached => 2,
                },
                report.steps.len(),
                report.profile.clone(),
            )
        })
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "no generated linked-IR profile contains exactly one root {}:{}; run `project analyze` after selecting the function",
                request.source, request.root_symbol
            ))
        })
}

#[allow(clippy::too_many_arguments)]
fn compose_path(
    profile: &crate::project_ir::ProjectIrProfile,
    reader: &artifacts::LinkedIrReader,
    root: &str,
    target: &FlowTargetRequest<'_>,
    path: &[artifacts::StoredGraphEdge],
    max_depth: usize,
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
    if selected.len() > MAX_LOADED_FUNCTIONS {
        blockers.push(limit_blocker("max-loaded-functions"));
        selected = selected.into_iter().take(MAX_LOADED_FUNCTIONS).collect();
    }
    let mut functions = BTreeMap::new();
    for identity in selected {
        match reader.get_function_by_identity(&identity)? {
            Some(function) => {
                functions.insert(identity, function);
            }
            None => blockers.push(FlowBlocker::new(
                "missing-function-record",
                format!("graph identity {identity:?} has no indexed function record"),
                "regenerate the linked-IR profile",
            )),
        }
    }

    let mut domains = root_domains();
    let mut steps = Vec::new();
    for (ordinal, edge) in path.iter().enumerate() {
        let Some(caller) = functions.get(&edge.caller) else {
            blockers.push(FlowBlocker::new(
                "missing-caller-record",
                format!("cannot load caller {}", edge.caller),
                "regenerate the linked-IR profile or reduce the requested path",
            ));
            continue;
        };
        let Some(call) = caller.calls.iter().find(|call| {
            call.target == edge.callee && call.site == edge.site && call.kind == edge.kind
        }) else {
            blockers.push(FlowBlocker::new(
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
                blockers.push(FlowBlocker::new(
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
            blockers.push(FlowBlocker::new(
                "missing-abi-arguments",
                format!(
                    "{} -> {} has no recovered ABI arguments",
                    edge.caller, edge.callee
                ),
                "review the call boundary and its ABI model",
            ));
        }
        domains = next;
        steps.push(FlowStepEvidence {
            ordinal,
            evidence: EvidenceLevel::Observed,
            context: "synchronous".to_owned(),
            caller: edge.caller.clone(),
            callee: edge.callee.clone(),
            site: edge.site,
            kind: edge.kind.clone(),
            tail: call.tail(),
            argument_shapes: call.argument_shapes(),
            arguments,
            guards: call.guard_expressions(),
            origin: profile.output.display().to_string(),
        });
    }
    if let Some(limit) = reached_limit {
        blockers.push(limit_blocker(limit));
    }

    let mut effects = Vec::new();
    if let Some(function) = functions.get(&sink) {
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
        if !function.complete {
            blockers.push(FlowBlocker::new(
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
    Ok(FlowInvestigationReport {
        schema_version: 3,
        command: "inspect flow",
        mode: "target",
        status,
        profile: profile.id.clone(),
        linked_ir: profile.output.display().to_string(),
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
            visited_nodes,
            examined_edges,
            loaded_functions: functions.len(),
            reached: reached_limit.map(str::to_owned),
            ..FlowLimits::new(max_depth)
        },
        steps,
        effects,
        rust_boundaries: Vec::new(),
        blockers,
    })
}

fn not_reached(
    profile: &crate::project_ir::ProjectIrProfile,
    root: &str,
    target: &FlowTargetRequest<'_>,
    max_depth: usize,
    message: &str,
) -> FlowInvestigationReport {
    FlowInvestigationReport {
        schema_version: 3,
        command: "inspect flow",
        mode: "target",
        status: FlowStatus::NotReached,
        profile: profile.id.clone(),
        linked_ir: profile.output.display().to_string(),
        root: root.to_owned(),
        target_kind: Some(target_kind(target).to_owned()),
        target: Some(target_label(target)),
        route: None,
        claims: FlowClaims::default(),
        limits: FlowLimits::new(max_depth),
        steps: Vec::new(),
        effects: Vec::new(),
        rust_boundaries: Vec::new(),
        blockers: vec![FlowBlocker::new(
            "target-not-reached",
            message,
            "increase --max-depth, regenerate project analysis, or inspect the missing boundary",
        )],
    }
}

fn target_identities(
    reader: &artifacts::LinkedIrReader,
    target: &FlowTargetRequest<'_>,
) -> Result<BTreeSet<String>> {
    match target {
        FlowTargetRequest::Function(target) => Ok(reader.matching_function_identities(target)),
        FlowTargetRequest::Register(register) => {
            reader.mmio_function_identities(Some(register), None)
        }
        FlowTargetRequest::Address(address) => {
            reader.mmio_function_identities(None, Some(*address))
        }
    }
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
    FlowBlocker::new(
        "analysis-limit",
        format!("flow investigation reached {limit}"),
        "narrow the root/target or raise the explicit bound after measuring resource use",
    )
}
