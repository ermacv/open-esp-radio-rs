//! Reviewed asynchronous route investigation.
//!
//! The route joins observed dispatch and delivery facts with an explicitly
//! reviewed selector mapping.  A reviewed mapping is useful navigation but is
//! never silently promoted to an executable queue or jump-table model.

use std::collections::BTreeSet;

use crate::{ProjectSpec, Result, artifacts, interfaces::SemanticCatalogs};

mod replay;

use replay::load_replay_proof;

use super::{
    EventRouteFlowRequest, EvidenceLevel, FlowArgumentEvidence, FlowBlocker, FlowClaims,
    FlowEffectEvidence, FlowEffectKind, FlowInvestigationReport, FlowLimits, FlowPointeeEvidence,
    FlowRustBoundaryEvidence, FlowStatus, FlowStepEvidence, FlowTargetRequest, TargetFlowRequest,
    effects::collect_function_effects,
    value::{compose_call_arguments, root_domains},
};

pub(super) fn investigate(
    request: EventRouteFlowRequest<'_>,
    project: &ProjectSpec,
) -> Result<FlowInvestigationReport> {
    let workspace = project.functions.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "inspect flow --event-route requires a configured [functions] workspace",
        )
    })?;
    let pack = crate::function_workspace::FunctionPack::load_reviewed(&workspace.pack)?;
    let matches = pack
        .event_routes
        .iter()
        .filter(|route| route.id == request.route)
        .collect::<Vec<_>>();
    let route = match matches.as_slice() {
        [] => {
            return Err(crate::Error::invalid(format!(
                "reviewed event route {:?} is not configured in {}",
                request.route,
                workspace.pack.display()
            )));
        }
        [route] => *route,
        _ => {
            return Err(crate::Error::invalid(format!(
                "reviewed event route {:?} is duplicated in {}",
                request.route,
                workspace.pack.display()
            )));
        }
    };
    let mut blockers = Vec::new();
    let replay = match load_replay_proof(route) {
        Ok(proof) => proof,
        Err(error) => {
            blockers.push(FlowBlocker::manual(
                "event-replay-invalid",
                error.to_string(),
                "rerun the configured replay against current inputs and retain its evidence artifact",
            ));
            None
        }
    };

    let dispatcher_profile = profile(project, &route.profile)?;
    let dispatcher_reader = artifacts::LinkedIrReader::open(&dispatcher_profile.output)?;
    let dispatcher = dispatcher_reader
        .get_function_by_identity(&route.dispatcher)?
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "event route {:?} dispatcher {:?} is absent from profile {:?}",
                route.id, route.dispatcher, route.profile
            ))
        })?;
    if dispatcher.source != route.source {
        return Err(crate::Error::invalid(format!(
            "event route {:?} dispatcher belongs to source {:?}, expected {:?}",
            route.id, dispatcher.source, route.source
        )));
    }

    let catalogs = semantic_catalogs(project)?;
    let selector = format!("const:{:#010x}", route.selector_value);
    let matching_dispatches = dispatcher
        .effect_summary
        .event_dispatches
        .iter()
        .filter(|dispatch| {
            dispatch.mechanism == route.mechanism
                && dispatch.execution_context == route.execution_context
                && route
                    .receiver
                    .as_ref()
                    .is_none_or(|receiver| dispatch.receiver.as_ref() == Some(receiver))
                && dispatch.interface_complete
                && dispatch.bindings.iter().any(|binding| {
                    binding.role == route.selector_role && binding.argument.value() == selector
                })
        })
        .collect::<Vec<_>>();
    let direct_dispatches = dispatcher
        .calls
        .iter()
        .filter(|call| direct_dispatch_matches(call, route, &selector, &catalogs))
        .collect::<Vec<_>>();
    let dispatch_observed = matching_dispatches.len() == 1 || direct_dispatches.len() == 1;
    if !dispatch_observed {
        blockers.push(FlowBlocker::manual(
            "dispatch-evidence-mismatch",
            format!(
                "expected one complete {} dispatch with {}={}, found {}",
                route.mechanism,
                route.selector_role,
                selector,
                matching_dispatches.len() + direct_dispatches.len()
            ),
            "regenerate linked IR or update the reviewed route after checking the vendor artifact",
        ));
    }

    let consumer_profile = profile(project, &route.consumer_profile)?;
    let consumer_reader = artifacts::LinkedIrReader::open(&consumer_profile.output)?;
    let consumer = consumer_reader
        .get_function_by_identity(&route.consumer_entry)?
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "event route {:?} consumer {:?} is absent from profile {:?}",
                route.id, route.consumer_entry, route.consumer_profile
            ))
        })?;
    if consumer.source != route.consumer_source {
        return Err(crate::Error::invalid(format!(
            "event route {:?} consumer belongs to source {:?}, expected {:?}",
            route.id, consumer.source, route.consumer_source
        )));
    }

    let output_argument = catalogs
        .get(&route.delivery.operation)
        .and_then(|operation| {
            operation
                .argument_roles
                .iter()
                .position(|role| role == &route.delivery.output_role)
        });
    if output_argument.is_none() {
        blockers.push(FlowBlocker::manual(
            "unknown-delivery-output-role",
            format!(
                "semantic operation {:?} does not define output role {:?}",
                route.delivery.operation, route.delivery.output_role
            ),
            "correct the reviewed route or add the role to the reusable knowledge pack",
        ));
    }
    let delivery_calls = consumer
        .calls
        .iter()
        .filter(|call| call.semantic_operation.as_deref() == Some(&route.delivery.operation))
        .collect::<Vec<_>>();
    let delivery = (delivery_calls.len() == 1).then(|| delivery_calls[0]);
    if delivery.is_none() {
        blockers.push(FlowBlocker::manual(
            "delivery-call-mismatch",
            format!(
                "consumer {:?} has {} direct calls annotated as {:?}",
                route.consumer_entry,
                delivery_calls.len(),
                route.delivery.operation
            ),
            "inspect the consumer and review an unambiguous delivery boundary",
        ));
    }

    let mut steps = Vec::new();
    let dispatch_site = matching_dispatches
        .first()
        .and_then(|dispatch| {
            dispatcher
                .effect_summary
                .semantic_actions
                .get(dispatch.semantic_action_index)
                .and_then(|action| action.site)
        })
        .or_else(|| direct_dispatches.first().and_then(|call| call.site));
    let mut effects = vec![FlowEffectEvidence {
        kind: "event".to_owned(),
        evidence: EvidenceLevel::Observed,
        function: route.dispatcher.clone(),
        site: dispatch_site,
        operation: Some(route.mechanism.clone()),
        detail: format!("{}={selector}", route.selector_role),
        constant: Some(u64::from(route.selector_value)),
        access: None,
        width: None,
        address: None,
        register: None,
        value: Some(selector.clone()),
        origin_path: None,
    }];
    if let Some(proof) = &replay {
        effects.push(FlowEffectEvidence {
            kind: "queue".to_owned(),
            evidence: EvidenceLevel::Executed,
            function: proof.producer_symbol.clone(),
            site: Some(proof.enqueue_site),
            operation: Some("enqueue".to_owned()),
            detail: format!(
                "{} receives selector {:#x}",
                proof.service_id, route.selector_value
            ),
            constant: Some(u64::from(route.selector_value)),
            access: None,
            width: Some(route.delivery.selector_width),
            address: None,
            register: None,
            value: Some(selector.clone()),
            origin_path: route
                .replay
                .as_ref()
                .map(|replay| replay.evidence.display().to_string()),
        });
        for (function, site, before, after, operation) in [
            (
                proof.producer_symbol.as_str(),
                proof.state.producer_write_site,
                proof.state.producer_before,
                proof.state.producer_after,
                "counted-latch-increment",
            ),
            (
                proof.consumer_symbol.as_str(),
                proof.state.consumer_write_site,
                proof.state.consumer_before,
                proof.state.consumer_after,
                "counted-latch-decrement",
            ),
        ] {
            effects.push(FlowEffectEvidence {
                kind: "state".to_owned(),
                evidence: EvidenceLevel::Executed,
                function: function.to_owned(),
                site: Some(site),
                operation: Some(operation.to_owned()),
                detail: format!(
                    "{} {}-bit state {before:#x} -> {after:#x}",
                    proof.state.id, proof.state.width
                ),
                constant: Some(u64::from(after)),
                access: Some("write".to_owned()),
                width: Some(proof.state.width),
                address: Some(proof.state.address),
                register: Some(proof.state.symbol.clone()),
                value: Some(format!("const:{after:#010x}")),
                origin_path: route
                    .replay
                    .as_ref()
                    .map(|replay| replay.evidence.display().to_string()),
            });
        }
    }
    let mut depth_limited = false;
    if request.max_depth >= 1 && dispatch_observed {
        steps.push(FlowStepEvidence {
            ordinal: steps.len(),
            evidence: EvidenceLevel::Reviewed,
            context: route.execution_context.clone(),
            caller: route.dispatcher.clone(),
            callee: route.consumer_entry.clone(),
            site: None,
            kind: "asynchronous-dispatch".to_owned(),
            tail: false,
            argument_shapes: 1,
            arguments: vec![selector_argument(
                route.selector_value,
                &route.selector_role,
            )],
            guards: Vec::new(),
            origin: workspace.pack.display().to_string(),
        });
    } else if dispatch_observed {
        depth_limited = true;
    }

    let mut delivery_modeled = false;
    if request.max_depth >= 2 {
        if let Some(delivery) = delivery {
            let (_, mut arguments) = compose_call_arguments(&consumer, delivery, &root_domains());
            if let Some(position) = output_argument {
                if let Some(argument) = arguments.get_mut(position) {
                    argument.pointee.push(FlowPointeeEvidence {
                        offset: route.delivery.selector_offset as i32,
                        width: route.delivery.selector_width,
                        local: format!(
                            "{}+{:#x}",
                            route.delivery.output_role, route.delivery.selector_offset
                        ),
                        resolved: selector.clone(),
                        constants: vec![route.selector_value],
                        provenance: "reviewed-route",
                    });
                } else {
                    blockers.push(FlowBlocker::manual(
                        "missing-delivery-argument",
                        format!(
                            "delivery role {:?} maps to argument {}, but the call exposes {} arguments",
                            route.delivery.output_role,
                            position,
                            arguments.len()
                        ),
                        "repair the ABI/interface pack and regenerate linked IR",
                    ));
                }
                delivery_modeled = delivery.models_output(position, route.delivery.selector_width);
            }
            let evidence = if replay.is_some() {
                EvidenceLevel::Executed
            } else if delivery_modeled {
                EvidenceLevel::Modeled
            } else {
                EvidenceLevel::Observed
            };
            steps.push(FlowStepEvidence {
                ordinal: steps.len(),
                evidence,
                context: route.execution_context.clone(),
                caller: route.consumer_entry.clone(),
                callee: delivery.target.clone(),
                site: replay
                    .as_ref()
                    .map_or(delivery.site, |proof| Some(proof.dequeue_site)),
                kind: "event-delivery".to_owned(),
                tail: delivery.tail(),
                argument_shapes: delivery.argument_shapes(),
                arguments,
                guards: delivery.guard_expressions(),
                origin: consumer_profile.output.display().to_string(),
            });
            effects.push(FlowEffectEvidence {
                kind: "queue".to_owned(),
                evidence,
                function: replay.as_ref().map_or_else(
                    || route.consumer_entry.clone(),
                    |proof| proof.consumer_symbol.clone(),
                ),
                site: replay
                    .as_ref()
                    .map_or(delivery.site, |proof| Some(proof.dequeue_site)),
                operation: delivery.semantic_operation.clone(),
                detail: replay.as_ref().map_or_else(
                    || delivery.target.clone(),
                    |proof| {
                        format!(
                            "{} delivers selector {:#x}",
                            proof.service_id, route.selector_value
                        )
                    },
                ),
                constant: Some(u64::from(route.selector_value)),
                access: None,
                width: Some(route.delivery.selector_width),
                address: None,
                register: None,
                value: Some(selector.clone()),
                origin_path: route
                    .replay
                    .as_ref()
                    .map(|replay| replay.evidence.display().to_string()),
            });
            if replay.is_some() {
                // Concrete FIFO lifecycle evidence closes the delivery
                // obligation; the structural output model remains its ABI
                // explanation.
            } else if delivery_modeled {
                blockers.push(FlowBlocker::manual(
                    "event-delivery-not-replayed",
                    format!(
                        "{} can write the reviewed {}-bit output, but no concrete queue instance has replayed selector {:#x}",
                        route.delivery.operation,
                        route.delivery.selector_width,
                        route.selector_value
                    ),
                    "bind a scenario-owned queue item, replay the consumer, and retain the execution trace",
                ));
            } else {
                blockers.push(FlowBlocker::manual(
                    "delivery-not-executable",
                    format!(
                        "{} is annotated but has no execution model writing {}-bit output argument {:?}",
                        route.delivery.operation,
                        route.delivery.selector_width,
                        route.delivery.output_role
                    ),
                    "add a generic external-call output model and a scenario-owned queue instance",
                ));
            }
        }
    } else if delivery.is_some() {
        depth_limited = true;
    }

    let mut loaded_functions = 2usize;
    let mut rust_boundaries = Vec::new();
    if request.max_depth >= 3 {
        if let Some(handler) = &route.case_handler {
            let handler_profile = profile(project, &handler.profile)?;
            let handler_reader = artifacts::LinkedIrReader::open(&handler_profile.output)?;
            let handler_function = handler_reader
                .get_function_by_identity(&handler.function)?
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "event route {:?} case handler {:?} is absent from profile {:?}",
                        route.id, handler.function, handler.profile
                    ))
                })?;
            if handler_function.source != handler.source {
                return Err(crate::Error::invalid(format!(
                    "event route {:?} case handler belongs to source {:?}, expected {:?}",
                    route.id, handler_function.source, handler.source
                )));
            }
            loaded_functions += 1;
            steps.push(FlowStepEvidence {
                ordinal: steps.len(),
                evidence: if replay.is_some() {
                    EvidenceLevel::Executed
                } else {
                    EvidenceLevel::Reviewed
                },
                context: route.execution_context.clone(),
                caller: route.consumer_entry.clone(),
                callee: handler.function.clone(),
                site: replay.as_ref().and_then(|proof| proof.handler_site),
                kind: "selector-case".to_owned(),
                tail: false,
                argument_shapes: 1,
                arguments: vec![selector_argument(
                    route.selector_value,
                    &route.selector_role,
                )],
                guards: vec![format!(
                    "{}+{:#x} == {selector}",
                    route.delivery.output_role, route.delivery.selector_offset
                )],
                origin: workspace.pack.display().to_string(),
            });
            collect_function_effects(&handler_function, FlowEffectKind::All, &mut effects);
            if route.terminal.is_none() {
                rust_boundaries = rust_boundary_evidence(&handler_function, project)?;
                if rust_boundaries.is_empty() {
                    blockers.push(FlowBlocker::manual(
                        "rust-boundary-unmapped",
                        format!(
                            "case handler {:?} has no vendor-to-Rust replacement edge",
                            handler_function.symbol
                        ),
                        "add a reviewed disposition and production Rust component, then run project verification",
                    ));
                }
            }

            let targets = BTreeSet::from([handler.function.clone()]);
            let observed_case = consumer_reader
                .shortest_path_to_any(
                    &route.consumer_entry,
                    &targets,
                    artifacts::GraphSearchLimits {
                        max_depth: 1,
                        max_visited_nodes: 64,
                        max_examined_edges: 256,
                    },
                )?
                .path
                .is_some();
            if !observed_case {
                blockers.push(FlowBlocker::manual(
                    "case-dispatch-not-executable",
                    format!(
                        "selector {:#x} to {:?} is reviewed, but the indirect ppTask jump is not resolved in generated IR",
                        route.selector_value, handler.function
                    ),
                    "add a reviewed indexed jump-table instance and replay this selector path",
                ));
            }
        } else {
            blockers.push(FlowBlocker::manual(
                "case-handler-unreviewed",
                format!(
                    "event route {:?} stops at consumer {:?}",
                    route.id, route.consumer_entry
                ),
                "review the selector-specific handler after confirming the vendor jump table",
            ));
        }
    } else if route.case_handler.is_some() {
        depth_limited = true;
    }

    if let Some(terminal) = &route.terminal {
        if let Some(handler) = route.case_handler.as_ref() {
            if request.max_depth >= 4 {
                let remaining_depth = request.max_depth.saturating_sub(3).max(1);
                let mut segment = super::target::investigate(
                    TargetFlowRequest {
                        source: &handler.source,
                        root_symbol: identity_symbol(&handler.function),
                        target: FlowTargetRequest::Function(&terminal.function),
                        max_depth: remaining_depth,
                    },
                    project,
                )?;
                let ordinal_base = steps.len();
                for (index, step) in segment.steps.iter_mut().enumerate() {
                    step.ordinal = ordinal_base + index;
                }
                steps.extend(segment.steps);
                effects.extend(segment.effects);
                blockers.extend(segment.blockers);
                loaded_functions += segment.limits.loaded_functions;

                let terminal_profile = profile(project, &terminal.profile)?;
                let terminal_reader = artifacts::LinkedIrReader::open(&terminal_profile.output)?;
                let terminal_function = terminal_reader
                    .get_function_by_identity(&terminal.function)?
                    .ok_or_else(|| {
                        crate::Error::invalid(format!(
                            "event route {:?} terminal {:?} is absent from profile {:?}",
                            route.id, terminal.function, terminal.profile
                        ))
                    })?;
                if terminal_function.source != terminal.source {
                    return Err(crate::Error::invalid(format!(
                        "event route {:?} terminal belongs to source {:?}, expected {:?}",
                        route.id, terminal_function.source, terminal.source
                    )));
                }
                rust_boundaries = rust_boundary_evidence(&terminal_function, project)?;
                if rust_boundaries.is_empty() {
                    blockers.push(FlowBlocker::manual(
                        "rust-boundary-unmapped",
                        format!(
                            "terminal {:?} has no vendor-to-Rust replacement edge",
                            terminal_function.symbol
                        ),
                        "add a reviewed disposition and production Rust component, then run project verification",
                    ));
                }
            } else {
                depth_limited = true;
            }
        } else {
            blockers.push(FlowBlocker::manual(
                "terminal-without-handler",
                format!(
                    "event route {:?} has a terminal but no case handler",
                    route.id
                ),
                "review the selector-specific case handler before its synchronous terminal",
            ));
        }
    }
    if depth_limited {
        blockers.push(FlowBlocker::manual(
            "max-depth",
            format!("event route exceeds --max-depth {}", request.max_depth),
            "increase --max-depth to include the remaining reviewed transition",
        ));
    }

    effects.sort_by(|left, right| {
        (&left.function, left.site, &left.kind, &left.detail).cmp(&(
            &right.function,
            right.site,
            &right.kind,
            &right.detail,
        ))
    });
    effects.dedup();
    let structural_navigation = dispatch_observed && delivery.is_some();
    Ok(FlowInvestigationReport {
        schema_version: 4,
        command: "inspect flow",
        mode: "event-route",
        status: if blockers.is_empty() {
            FlowStatus::Complete
        } else {
            FlowStatus::Incomplete
        },
        profile: route.profile.clone(),
        linked_ir: dispatcher_profile.output.display().to_string(),
        root: route.dispatcher.clone(),
        target_kind: Some("event-route".to_owned()),
        target: route
            .terminal
            .as_ref()
            .map(|terminal| terminal.function.clone())
            .or_else(|| {
                route
                    .case_handler
                    .as_ref()
                    .map(|handler| handler.function.clone())
            }),
        route: Some(route.id.clone()),
        claims: FlowClaims {
            structural_navigation,
            path_feasibility: false,
            event_delivery: replay.is_some(),
            executable_equivalence: false,
        },
        limits: FlowLimits {
            max_depth: request.max_depth,
            visited_nodes: loaded_functions,
            examined_edges: steps.len(),
            loaded_functions,
            reached: depth_limited.then(|| "max-depth".to_owned()),
            ..FlowLimits::new(request.max_depth)
        },
        steps,
        effects,
        rust_boundaries,
        blockers,
    })
}

fn identity_symbol(identity: &str) -> &str {
    identity
        .rsplit("::")
        .next()
        .unwrap_or(identity)
        .split('@')
        .next()
        .unwrap_or(identity)
}

fn rust_boundary_evidence(
    function: &artifacts::StoredFunction,
    project: &ProjectSpec,
) -> Result<Vec<FlowRustBoundaryEvidence>> {
    Ok(crate::function_investigation::replacement_evidence(
        &function.source,
        &function.symbol,
        project,
    )?
    .into_iter()
    .map(|replacement| FlowRustBoundaryEvidence {
        vendor_source: replacement.vendor_source,
        vendor_symbol: replacement.vendor_symbol,
        association: replacement.association.to_owned(),
        reviewed: replacement.reviewed,
        status: replacement.status,
        production_component: replacement.production_component,
        verification_probes: replacement.verification_probes,
        report: replacement.report,
        report_complete_project_run: replacement.report_complete_project_run,
        report_passed: replacement.report_passed,
        freshness_claim: replacement.freshness_claim,
    })
    .collect())
}

fn profile<'a>(
    project: &'a ProjectSpec,
    id: &str,
) -> Result<&'a crate::project_ir::ProjectIrProfile> {
    let profile = project
        .ir_profiles
        .iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| crate::Error::invalid(format!("unknown linked-IR profile {id:?}")))?;
    if !profile.output.is_dir() {
        return Err(crate::Error::invalid(format!(
            "linked-IR profile {id:?} has not been generated; run `project analyze`"
        )));
    }
    Ok(profile)
}

fn semantic_catalogs(project: &ProjectSpec) -> Result<SemanticCatalogs> {
    let paths = project.interfaces.as_ref().map_or_else(
        || {
            project
                .ecosystem_packs
                .iter()
                .flat_map(|pack| pack.knowledge_packs.iter().cloned())
                .chain(
                    project
                        .chip_pack
                        .iter()
                        .flat_map(|pack| pack.knowledge_packs.iter().cloned()),
                )
                .collect::<Vec<_>>()
        },
        |interfaces| interfaces.semantic_catalogs.clone(),
    );
    SemanticCatalogs::load(&paths)
}

fn direct_dispatch_matches(
    call: &artifacts::StoredCall,
    route: &crate::function_workspace::ReviewedEventRoute,
    selector: &str,
    catalogs: &SemanticCatalogs,
) -> bool {
    let Some(contract) = call
        .semantic_contract
        .as_ref()
        .and_then(|contract| contract.event_dispatch.as_ref())
    else {
        return false;
    };
    if contract.mechanism != route.mechanism
        || contract.execution_context != route.execution_context
        || !route
            .receiver
            .as_ref()
            .is_none_or(|receiver| contract.receiver.as_ref() == Some(receiver))
    {
        return false;
    }
    let Some(binding) = contract
        .argument_roles
        .iter()
        .find(|binding| binding.role == route.selector_role)
    else {
        return false;
    };
    if let Some(operation) = call
        .semantic_operation
        .as_deref()
        .and_then(|operation| catalogs.get(operation))
        && let Some(position) = operation
            .argument_roles
            .iter()
            .position(|role| role == &binding.argument)
    {
        return call
            .arguments
            .get(position)
            .is_some_and(|value| value == selector);
    }

    // Some reviewed direct-call contracts predate reusable knowledge pack
    // entries.  They still name the event role, and the recovered ABI values
    // can locate an exact selector when that value occurs at one position.
    // Ambiguous equal constants fail closed.
    let matching_values = call
        .arguments
        .iter()
        .filter(|value| value.as_str() == selector)
        .count();
    matching_values == 1
}

fn selector_argument(value: u32, role: &str) -> FlowArgumentEvidence {
    FlowArgumentEvidence {
        position: 0,
        local: role.to_owned(),
        resolved: format!("const:{value:#010x}"),
        constants: vec![value],
        provenance: "reviewed-route",
        pointee: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::function_workspace::{
        ReviewedEventCaseHandler, ReviewedEventDelivery, ReviewedEventReplay, ReviewedEventRoute,
        ReviewedEventStateModel,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_REPLAY_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn replay_test_directory() -> std::path::PathBuf {
        loop {
            let sequence = NEXT_REPLAY_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "blobray-event-replay-{}-{sequence}",
                std::process::id()
            ));
            match std::fs::create_dir(&directory) {
                Ok(()) => return directory,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!(
                    "cannot create replay test directory {}: {error}",
                    directory.display()
                ),
            }
        }
    }

    #[test]
    fn replay_proof_requires_fresh_ordered_fifo_delivery_and_handler_goal() {
        let directory = replay_test_directory();
        let manifest = directory.join("route.toml");
        let artifact = directory.join("vendor.elf");
        let evidence = directory.join("route.json");
        std::fs::write(&manifest, "schema = 2\n# changed\n").unwrap();
        std::fs::write(&artifact, b"linked-image").unwrap();
        let document = serde_json::json!({
            "schema_version": 3,
            "command": "execute replay",
            "manifest": {
                "path": std::fs::canonicalize(&manifest).unwrap(),
                "sha256": crate::artifact_sha256(&manifest).unwrap(),
            },
            "artifact": {
                "path": std::fs::canonicalize(&artifact).unwrap(),
                "sha256": crate::artifact_sha256(&artifact).unwrap(),
            },
            "diagnostic_contracts": { "calls": [] },
            "phases": [
                {
                    "name": "post",
                    "symbol": "post_event",
                    "completion": { "kind": "returned" },
                    "steps": 10,
                    "calls": [],
                    "fifo_lifecycle": [{
                        "kind": "enqueued",
                        "service_id": "events",
                        "site": 4096,
                        "value": 25,
                        "depth_before": 0,
                        "depth_after": 1,
                        "woke_receiver": true,
                    }],
                    "memory_observations": [{
                        "id": "pending-count",
                        "symbol": "pending_count",
                        "address": 12288,
                        "width": 8,
                        "before": 0,
                        "after": 1,
                        "writes": [{ "site": 4100, "value": 1 }],
                    }],
                },
                {
                    "name": "dispatch",
                    "symbol": "worker",
                    "completion": {
                        "kind": "goal-reached",
                        "goal": { "kind": "reach-symbol", "symbol": "handler" },
                    },
                    "steps": 20,
                    "calls": [{
                        "site": 8192,
                        "symbol": "handler",
                        "arguments": [25, 0, 0, 0, 0, 0, 0, 0],
                    }],
                    "fifo_lifecycle": [{
                        "kind": "dequeued",
                        "service_id": "events",
                        "site": 6144,
                        "value": 25,
                        "depth_before": 1,
                        "depth_after": 0,
                    }],
                    "memory_observations": [{
                        "id": "pending-count",
                        "symbol": "pending_count",
                        "address": 12288,
                        "width": 8,
                        "before": 1,
                        "after": 0,
                        "writes": [{ "site": 6150, "value": 0 }],
                    }],
                },
            ],
            "complete": true,
        });
        std::fs::write(&evidence, serde_json::to_vec(&document).unwrap()).unwrap();
        let mut route = ReviewedEventRoute {
            id: "route".to_owned(),
            profile: "linked".to_owned(),
            source: "vendor".to_owned(),
            dispatcher: "vendor::post".to_owned(),
            mechanism: "event".to_owned(),
            selector_role: "selector".to_owned(),
            selector_value: 25,
            receiver: None,
            execution_context: "task".to_owned(),
            consumer_profile: "linked".to_owned(),
            consumer_source: "vendor".to_owned(),
            consumer_entry: "vendor::worker".to_owned(),
            delivery: ReviewedEventDelivery {
                operation: "queue.receive".to_owned(),
                output_role: "item".to_owned(),
                selector_offset: 0,
                selector_width: 32,
                encoding: "little-endian".to_owned(),
            },
            case_handler: Some(ReviewedEventCaseHandler {
                profile: "linked".to_owned(),
                source: "vendor".to_owned(),
                function: "vendor::handler@0x2000".to_owned(),
            }),
            terminal: None,
            replay: Some(ReviewedEventReplay {
                manifest: manifest.clone(),
                source: "fixture-replay".to_owned(),
                evidence,
                producer_phase: "post".to_owned(),
                consumer_phase: "dispatch".to_owned(),
                state_observation: "pending-count".to_owned(),
                state_model: ReviewedEventStateModel::CountedLatch,
            }),
            rationale: "fixture".to_owned(),
        };
        let proof = load_replay_proof(&route).unwrap().unwrap();
        assert_eq!(proof.service_id, "events");
        assert_eq!(proof.enqueue_site, 4096);
        assert_eq!(proof.dequeue_site, 6144);
        assert_eq!(proof.handler_site, Some(8192));

        route.case_handler.as_mut().unwrap().function = "vendor::other_handler@0x2100".to_owned();
        assert!(
            load_replay_proof(&route)
                .unwrap_err()
                .to_string()
                .contains("did not complete by reaching handler")
        );
        route.case_handler.as_mut().unwrap().function = "vendor::handler@0x2000".to_owned();

        let other_manifest = directory.join("other-route.toml");
        std::fs::write(&other_manifest, "schema = 2\n# changed\n").unwrap();
        route.replay.as_mut().unwrap().manifest = other_manifest;
        assert!(
            load_replay_proof(&route)
                .unwrap_err()
                .to_string()
                .contains("reviewed route binds")
        );
        route.replay.as_mut().unwrap().manifest = manifest.clone();

        std::fs::write(&manifest, "schema = 2\n").unwrap();
        assert!(
            load_replay_proof(&route)
                .unwrap_err()
                .to_string()
                .contains("changed since execution")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
