//! Reviewed asynchronous route investigation.
//!
//! The route joins observed dispatch and delivery facts with an explicitly
//! reviewed selector mapping.  A reviewed mapping is useful navigation but is
//! never silently promoted to an executable queue or jump-table model.

use std::collections::BTreeSet;

use crate::{ProjectSpec, Result, artifacts, interfaces::SemanticCatalogs};

use super::{
    EventRouteFlowRequest, EvidenceLevel, FlowArgumentEvidence, FlowBlocker, FlowClaims,
    FlowEffectEvidence, FlowEffectKind, FlowInvestigationReport, FlowLimits, FlowPointeeEvidence,
    FlowRustBoundaryEvidence, FlowStatus, FlowStepEvidence,
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
    let mut blockers = Vec::new();
    if !dispatch_observed {
        blockers.push(FlowBlocker::new(
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
        blockers.push(FlowBlocker::new(
            "unknown-delivery-output-role",
            format!(
                "semantic operation {:?} does not define output role {:?}",
                route.delivery.operation, route.delivery.output_role
            ),
            "correct the reviewed route or add the role to the reusable semantic catalog",
        ));
    }
    let delivery_calls = consumer
        .calls
        .iter()
        .filter(|call| call.semantic_operation.as_deref() == Some(&route.delivery.operation))
        .collect::<Vec<_>>();
    let delivery = (delivery_calls.len() == 1).then(|| delivery_calls[0]);
    if delivery.is_none() {
        blockers.push(FlowBlocker::new(
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
                    blockers.push(FlowBlocker::new(
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
            let evidence = if delivery_modeled {
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
                site: delivery.site,
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
                function: route.consumer_entry.clone(),
                site: delivery.site,
                operation: delivery.semantic_operation.clone(),
                detail: delivery.target.clone(),
                constant: Some(u64::from(route.selector_value)),
                access: None,
                width: Some(route.delivery.selector_width),
                address: None,
                register: None,
                value: Some(selector.clone()),
                origin_path: None,
            });
            if delivery_modeled {
                blockers.push(FlowBlocker::new(
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
                blockers.push(FlowBlocker::new(
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
                evidence: EvidenceLevel::Reviewed,
                context: route.execution_context.clone(),
                caller: route.consumer_entry.clone(),
                callee: handler.function.clone(),
                site: None,
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
            rust_boundaries = crate::function_investigation::replacement_evidence(
                &handler_function.source,
                &handler_function.symbol,
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
            .collect();
            if rust_boundaries.is_empty() {
                blockers.push(FlowBlocker::new(
                    "rust-boundary-unmapped",
                    format!(
                        "case handler {:?} has no vendor-to-Rust replacement edge",
                        handler_function.symbol
                    ),
                    "add a reviewed disposition and production Rust component, then run project verification",
                ));
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
                )
                .path
                .is_some();
            if !observed_case {
                blockers.push(FlowBlocker::new(
                    "case-dispatch-not-executable",
                    format!(
                        "selector {:#x} to {:?} is reviewed, but the indirect ppTask jump is not resolved in generated IR",
                        route.selector_value, handler.function
                    ),
                    "add a reviewed indexed jump-table instance and replay this selector path",
                ));
            }
        } else {
            blockers.push(FlowBlocker::new(
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
    if depth_limited {
        blockers.push(FlowBlocker::new(
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
        schema_version: 2,
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
            .case_handler
            .as_ref()
            .map(|handler| handler.function.clone()),
        route: Some(route.id.clone()),
        claims: FlowClaims {
            structural_navigation,
            path_feasibility: false,
            event_delivery: false,
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
    let paths = project
        .interfaces
        .as_ref()
        .map(|interfaces| interfaces.semantic_catalogs.as_slice())
        .or_else(|| {
            project
                .platform_pack
                .as_ref()
                .map(|pack| pack.semantic_catalogs.as_slice())
        })
        .unwrap_or_default();
    SemanticCatalogs::load(paths)
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

    // Some reviewed direct-call contracts predate reusable semantic catalog
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
