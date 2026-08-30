use crate::{ProjectSpec, Result, artifacts};

use super::{
    EffectFlowRequest, EvidenceLevel, FlowBlocker, FlowClaims, FlowEffectEvidence, FlowEffectKind,
    FlowInvestigationReport, FlowLimits, FlowStatus, MAX_EXAMINED_EDGES, MAX_LOADED_FUNCTIONS,
    MAX_VISITED_NODES, target::limit_blocker,
};

pub(super) fn investigate(
    request: EffectFlowRequest<'_>,
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
        let reachability = reader.reachable_from(
            root,
            artifacts::GraphSearchLimits {
                max_depth: request.max_depth,
                max_visited_nodes: MAX_VISITED_NODES,
                max_examined_edges: MAX_EXAMINED_EDGES,
            },
        )?;
        let mut blockers = Vec::new();
        if let Some(limit) = reachability.limit {
            blockers.push(limit_blocker(limit));
        }
        let identities = reachability
            .identities
            .iter()
            .take(MAX_LOADED_FUNCTIONS)
            .cloned()
            .collect::<Vec<_>>();
        if reachability.identities.len() > identities.len() {
            blockers.push(limit_blocker("max-loaded-functions"));
        }
        let mut effects = Vec::new();
        let mut loaded = 0usize;
        let mut root_closed = false;
        for identity in identities {
            let Some(function) = reader.get_function_by_identity(&identity)? else {
                blockers.push(FlowBlocker::manual(
                    "missing-function-record",
                    format!("reachable identity {identity:?} has no indexed function record"),
                    "regenerate the linked-IR profile",
                ));
                continue;
            };
            loaded += 1;
            if identity == *root {
                root_closed = function.effect_summary.call_graph_closed;
            }
            collect_function_effects(&function, request.kind, &mut effects);
        }
        if !root_closed {
            blockers.push(FlowBlocker::manual(
                "open-call-graph",
                format!("reachable effect inventory for {root} has unresolved call boundaries"),
                "inspect the root blockers and add reviewed interface or external-call models",
            ));
        }
        effects.sort_by(|left, right| {
            (
                &left.function,
                left.site,
                &left.kind,
                &left.operation,
                left.address,
                &left.detail,
            )
                .cmp(&(
                    &right.function,
                    right.site,
                    &right.kind,
                    &right.operation,
                    right.address,
                    &right.detail,
                ))
        });
        effects.dedup();
        reports.push(FlowInvestigationReport {
            schema_version: 5,
            command: "inspect flow",
            mode: "effects",
            status: if blockers.is_empty() {
                FlowStatus::Complete
            } else {
                FlowStatus::Incomplete
            },
            profile: profile.id.clone(),
            linked_ir: profile.output.display().to_string(),
            root: root.clone(),
            target_kind: Some("effects".to_owned()),
            target: Some(request.kind.label().to_owned()),
            route: None,
            claims: FlowClaims {
                structural_navigation: true,
                ..FlowClaims::default()
            },
            limits: FlowLimits {
                max_depth: request.max_depth,
                visited_nodes: reachability.visited_nodes,
                examined_edges: reachability.examined_edges,
                loaded_functions: loaded,
                reached: reachability.limit.map(str::to_owned),
                ..FlowLimits::new(request.max_depth)
            },
            steps: Vec::new(),
            effects,
            publications: Vec::new(),
            memory_slice: Vec::new(),
            rust_boundaries: Vec::new(),
            blockers,
        });
    }
    reports
        .into_iter()
        .min_by_key(|report| {
            (
                report.blockers.len(),
                std::cmp::Reverse(report.effects.len()),
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

pub(super) fn collect_function_effects(
    function: &artifacts::StoredFunction,
    requested: FlowEffectKind,
    output: &mut Vec<FlowEffectEvidence>,
) {
    if matches!(requested, FlowEffectKind::Delay | FlowEffectKind::All) {
        output.extend(function.delays.iter().map(|delay| FlowEffectEvidence {
            kind: "delay".to_owned(),
            evidence: EvidenceLevel::Observed,
            function: function.identity.clone(),
            site: None,
            operation: None,
            detail: delay.micros.clone(),
            constant: delay.constant_micros.map(u64::from),
            access: None,
            width: None,
            address: None,
            register: None,
            value: None,
            origin_path: Some(delay.path.clone()),
        }));
    }

    for call in &function.calls {
        let selected = match requested {
            FlowEffectKind::Delay => call
                .semantic_operation
                .as_deref()
                .is_some_and(is_delay_operation),
            FlowEffectKind::Timer => call
                .semantic_operation
                .as_deref()
                .is_some_and(is_timer_operation),
            FlowEffectKind::Event => call
                .semantic_operation
                .as_deref()
                .is_some_and(is_event_operation),
            FlowEffectKind::Queue => call
                .semantic_operation
                .as_deref()
                .is_some_and(|operation| operation.starts_with("rtos.queue.")),
            FlowEffectKind::Call | FlowEffectKind::All => true,
            FlowEffectKind::Mmio | FlowEffectKind::Memory => false,
        };
        if !selected {
            continue;
        }
        output.push(FlowEffectEvidence {
            kind: semantic_kind(call.semantic_operation.as_deref()).to_owned(),
            evidence: if call.execution_model_id().is_some() {
                EvidenceLevel::Modeled
            } else {
                EvidenceLevel::Observed
            },
            function: function.identity.clone(),
            site: call.site,
            operation: call.semantic_operation.clone(),
            detail: call.target.clone(),
            constant: None,
            access: None,
            width: None,
            address: None,
            register: None,
            value: None,
            origin_path: None,
        });
    }

    if matches!(
        requested,
        FlowEffectKind::Mmio | FlowEffectKind::Memory | FlowEffectKind::All
    ) {
        for effect in &function.instruction_effects {
            let (kind, access, width, detail, paths, _, value) = effect.investigation_fields();
            if (requested == FlowEffectKind::Mmio && kind != "mmio")
                || (requested == FlowEffectKind::Memory && kind != "memory")
            {
                continue;
            }
            let (address, register) = effect
                .mmio()
                .map_or((None, None), |(_, _, address, name, _)| {
                    (Some(address), Some(name.to_owned()))
                });
            output.push(FlowEffectEvidence {
                kind: kind.to_owned(),
                evidence: EvidenceLevel::Observed,
                function: function.identity.clone(),
                site: Some(effect.site()),
                operation: None,
                detail,
                constant: None,
                access: Some(access.to_owned()),
                width: Some(width),
                address,
                register,
                value: value.map(str::to_owned),
                origin_path: (!paths.is_empty()).then(|| paths.join(" || ")),
            });
        }
    }
}

fn is_delay_operation(operation: &str) -> bool {
    matches!(operation, "time.blocking-delay" | "time.busy-loop")
}

fn is_timer_operation(operation: &str) -> bool {
    operation.starts_with("timer.") || operation == "time.timer-arm"
}

fn is_event_operation(operation: &str) -> bool {
    operation.starts_with("rtos.event.")
        || operation == "wifi.internal-signal.post"
        || operation == "rtos.task.yield-from-isr"
}

fn semantic_kind(operation: Option<&str>) -> &'static str {
    match operation {
        Some(operation) if is_delay_operation(operation) => "delay",
        Some(operation) if is_timer_operation(operation) => "timer",
        Some(operation) if is_event_operation(operation) => "event",
        Some(operation) if operation.starts_with("rtos.queue.") => "queue",
        _ => "call",
    }
}
