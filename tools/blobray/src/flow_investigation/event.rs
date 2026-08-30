//! Reviewed asynchronous route investigation.
//!
//! The route joins observed dispatch and delivery facts with an explicitly
//! reviewed selector mapping.  A reviewed mapping is useful navigation but is
//! never silently promoted to an executable queue or jump-table model.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{ProjectSpec, Result, artifact, artifacts, interfaces::SemanticCatalogs};

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
    let workspace_paths = project.functions.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "inspect flow --event-route requires a configured [functions] workspace",
        )
    })?;
    let reports = project.function_ir_reports()?;
    let workspace = crate::function_workspace::FunctionWorkspace::load_with_callback_facts(
        &reports,
        &workspace_paths.pack,
    )?;
    let mut context = EventRouteEvaluationContext::new(project, &workspace);
    context.investigate(request)
}

pub(super) fn investigate_many_with_workspace(
    route_ids: &[String],
    max_depth: usize,
    project: &ProjectSpec,
    workspace: &crate::function_workspace::FunctionWorkspace,
) -> Result<Vec<FlowInvestigationReport>> {
    let mut context = EventRouteEvaluationContext::new(project, workspace);
    route_ids
        .iter()
        .map(|route| context.investigate(EventRouteFlowRequest { route, max_depth }))
        .collect()
}

struct EventRouteEvaluationContext<'project> {
    project: &'project ProjectSpec,
    workspace: &'project crate::function_workspace::FunctionWorkspace,
    artifacts: EventRouteArtifacts,
    graph: Option<super::project_graph::ProjectGraph<'project>>,
}

#[derive(Default)]
struct EventRouteArtifacts {
    readers: BTreeMap<PathBuf, Arc<artifacts::LinkedIrReader>>,
    catalogs: Option<Arc<SemanticCatalogs>>,
}

impl EventRouteArtifacts {
    fn reader(&mut self, path: &Path) -> Result<Arc<artifacts::LinkedIrReader>> {
        if let Some(reader) = self.readers.get(path) {
            return Ok(Arc::clone(reader));
        }
        let reader = Arc::new(artifacts::LinkedIrReader::open(path)?);
        self.readers.insert(path.to_owned(), Arc::clone(&reader));
        Ok(reader)
    }

    fn catalogs(&mut self, project: &ProjectSpec) -> Result<Arc<SemanticCatalogs>> {
        self.catalogs_with(|| load_semantic_catalogs(project))
    }

    fn catalogs_with(
        &mut self,
        load: impl FnOnce() -> Result<SemanticCatalogs>,
    ) -> Result<Arc<SemanticCatalogs>> {
        if let Some(catalogs) = &self.catalogs {
            return Ok(Arc::clone(catalogs));
        }
        let catalogs = Arc::new(load()?);
        self.catalogs = Some(Arc::clone(&catalogs));
        Ok(catalogs)
    }
}

impl<'project> EventRouteEvaluationContext<'project> {
    fn new(
        project: &'project ProjectSpec,
        workspace: &'project crate::function_workspace::FunctionWorkspace,
    ) -> Self {
        Self {
            project,
            workspace,
            artifacts: EventRouteArtifacts::default(),
            graph: None,
        }
    }

    fn reader(&mut self, path: &Path) -> Result<Arc<artifacts::LinkedIrReader>> {
        self.artifacts.reader(path)
    }

    fn catalogs(&mut self) -> Result<Arc<SemanticCatalogs>> {
        self.artifacts.catalogs(self.project)
    }

    fn investigate_target(
        &mut self,
        request: TargetFlowRequest<'_>,
    ) -> Result<FlowInvestigationReport> {
        if self.graph.is_none() {
            let graph = super::project_graph::ProjectGraph::open_with(self.project, |path| {
                self.artifacts.reader(path)
            })?;
            self.graph = Some(graph);
        }
        super::target::investigate_with_graph(
            request,
            self.graph.as_ref().expect("graph initialized above"),
        )
    }

    fn investigate(
        &mut self,
        request: EventRouteFlowRequest<'_>,
    ) -> Result<FlowInvestigationReport> {
        let workspace_path = &self
            .project
            .functions
            .as_ref()
            .ok_or_else(|| {
                crate::Error::invalid(
                    "inspect flow --event-route requires a configured [functions] workspace",
                )
            })?
            .pack;
        let matches = self
            .workspace
            .pack
            .event_routes
            .iter()
            .filter(|route| route.id() == request.route)
            .collect::<Vec<_>>();
        let route = match matches.as_slice() {
            [] => {
                return Err(crate::Error::invalid(format!(
                    "reviewed event route {:?} is not configured in {}",
                    request.route,
                    workspace_path.display()
                )));
            }
            [route] => *route,
            _ => {
                return Err(crate::Error::invalid(format!(
                    "reviewed event route {:?} is duplicated in {}",
                    request.route,
                    workspace_path.display()
                )));
            }
        };
        match route {
            crate::function_workspace::ReviewedEventRoute::SelectorDelivery(route) => {
                investigate_selector(request, self, route)
            }
            crate::function_workspace::ReviewedEventRoute::StaticEventCallback(route) => {
                investigate_static_event_callback(request, self, route)
            }
            crate::function_workspace::ReviewedEventRoute::BrokerSubscription(route) => {
                investigate_broker_subscription(request, self, route)
            }
        }
    }
}

fn investigate_selector(
    request: EventRouteFlowRequest<'_>,
    context: &mut EventRouteEvaluationContext<'_>,
    route: &crate::function_workspace::ReviewedSelectorEventRoute,
) -> Result<FlowInvestigationReport> {
    let project = context.project;
    let workspace = project.functions.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "inspect flow --event-route requires a configured [functions] workspace",
        )
    })?;
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
    let dispatcher_reader = context.reader(&dispatcher_profile.output)?;
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

    let catalogs = context.catalogs()?;
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
    let consumer_reader = context.reader(&consumer_profile.output)?;
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
        // The selector value and exact call site are generated observations,
        // but assigning the call its event mechanism is reviewed knowledge.
        evidence: EvidenceLevel::Reviewed,
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
            context_evidence: EvidenceLevel::Reviewed,
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
                // The direct call edge is observed.  Its delivery role comes
                // from the reviewed semantic catalog and must not inherit the
                // stronger-looking observed label.
                EvidenceLevel::Reviewed
            };
            steps.push(FlowStepEvidence {
                ordinal: steps.len(),
                evidence,
                context_evidence: EvidenceLevel::Reviewed,
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
            let handler_reader = context.reader(&handler_profile.output)?;
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
                context_evidence: EvidenceLevel::Reviewed,
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
                let mut segment = context.investigate_target(TargetFlowRequest {
                    source: &handler.source,
                    root_symbol: identity_symbol(&handler.function),
                    target: FlowTargetRequest::Function(&terminal.function),
                    max_depth: remaining_depth,
                    max_loaded_functions: super::MAX_LOADED_FUNCTIONS
                        .saturating_sub(loaded_functions),
                })?;
                let ordinal_base = steps.len();
                for (index, step) in segment.steps.iter_mut().enumerate() {
                    step.ordinal = ordinal_base + index;
                }
                steps.extend(segment.steps);
                effects.extend(segment.effects);
                blockers.extend(segment.blockers);
                loaded_functions += segment.limits.loaded_functions;

                let terminal_profile = profile(project, &terminal.profile)?;
                let terminal_reader = context.reader(&terminal_profile.output)?;
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
    finalize_event_route_semantic_evidence(&route.id, &mut effects)?;
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

fn investigate_static_event_callback(
    request: EventRouteFlowRequest<'_>,
    context: &mut EventRouteEvaluationContext<'_>,
    route: &crate::function_workspace::ReviewedStaticEventCallbackRoute,
) -> Result<FlowInvestigationReport> {
    let project = context.project;
    let _workspace = project.functions.as_ref().ok_or_else(|| {
        crate::Error::invalid("callback event route requires a [functions] workspace")
    })?;
    if route.upstream_chain.len() < 2
        || route.upstream_chain.last() != Some(&route.dispatcher)
        || route.upstream_sites.len() != route.upstream_chain.len() - 1
        || route.dispatch_sites.is_empty()
    {
        return Err(route_mismatch(
            &route.id,
            "requires a non-empty dispatch-site set and an upstream chain ending at the dispatcher",
        ));
    }
    let dispatch_profile = profile(project, &route.profile)?;
    let reader = context.reader(&dispatch_profile.output)?;
    let dispatcher = route_function(
        &reader,
        &route.id,
        &route.source,
        &route.dispatcher,
        "dispatcher",
    )?;
    let binding_profile = profile(project, &route.binding_profile)?;
    let binding_reader = context.reader(&binding_profile.output)?;
    let binding = route_function(
        &binding_reader,
        &route.id,
        &route.binding_source,
        &route.binding_entry,
        "binding entry",
    )?;
    let callback_profile = profile(project, &route.callback_profile)?;
    let callback_reader = context.reader(&callback_profile.output)?;
    let callback = route_function(
        &callback_reader,
        &route.id,
        &route.callback_source,
        &route.callback_function,
        "callback",
    )?;
    let callback_address = callback.address.ok_or_else(|| {
        crate::Error::invalid(format!(
            "event route {:?} callback has no linked address",
            route.id
        ))
    })?;
    let binding_call = stored_exact_call(
        &binding,
        &route.binding_call,
        route.binding_site,
        &route.id,
        "binding",
    )?;
    let bound_object = stored_exact_argument(
        binding_call,
        route.binding_object_argument,
        &route.id,
        "binding object",
    )?;
    let expected_callback = format!("const:{callback_address:#010x}");
    if stored_exact_argument(
        binding_call,
        route.binding_callback_argument,
        &route.id,
        "binding callback",
    )? != expected_callback
    {
        return Err(route_mismatch(
            &route.id,
            "event initializer callback no longer resolves to the reviewed function",
        ));
    }
    let delivery_profile = profile(project, &route.delivery_profile)?;
    let delivery_reader = context.reader(&delivery_profile.output)?;
    let delivery = route_function(
        &delivery_reader,
        &route.id,
        &route.delivery_source,
        &route.delivery_entry,
        "event delivery loop",
    )?;
    let receive_call = stored_exact_call(
        &delivery,
        &route.receive_call,
        route.receive_site,
        &route.id,
        "event receive",
    )?;
    if !receive_call.result_modeled() {
        return Err(route_mismatch(
            &route.id,
            "event receive does not expose a modeled return pointer",
        ));
    }
    let receive_queue = stored_exact_argument(
        receive_call,
        route.receive_queue_argument,
        &route.id,
        "receive queue",
    )?;
    let run_call = stored_exact_call(
        &delivery,
        &route.run_call,
        route.run_site,
        &route.id,
        "event run",
    )?;
    let run_event = run_call
        .arguments
        .get(usize::from(route.run_event_argument))
        .ok_or_else(|| route_mismatch(&route.id, "event run argument is absent"))?;
    let delivery_artifact =
        delivery_reader.authenticated_source_artifact(&route.delivery_source)?;
    let delivery_body = artifact::inspect_function_body_at_data(
        &delivery_artifact.path,
        &delivery_artifact.bytes,
        delivery.member.as_deref(),
        &delivery.symbol,
        delivery.address.map(u64::from),
    )?;
    let delivery_order =
        super::cfg::must_execute_before(&delivery_body, route.receive_site, route.run_site);
    let run_event_exact = run_call.argument_is_exact(usize::from(route.run_event_argument));
    let receive_result_flow_proven = delivery_order.as_ref().is_some_and(|order| {
        order.earlier_block == order.later_block
            && run_call.argument_is_result_of(usize::from(route.run_event_argument), receive_call)
    });
    let mut steps = Vec::new();
    for (edge, site) in route.upstream_chain.windows(2).zip(&route.upstream_sites) {
        let owner = route_function(
            &reader,
            &route.id,
            &route.source,
            &edge[0],
            "upstream stage",
        )?;
        let call = stored_exact_direct_call(&owner, &edge[1], *site, &route.id, "upstream direct")?;
        if call.kind != "internal" || !call.direct() {
            return Err(route_mismatch(
                &route.id,
                format!(
                    "upstream edge {:?} -> {:?} at {site:#010x} is not an internal direct call",
                    edge[0], edge[1]
                ),
            ));
        }
        steps.push(observed_direct_call_step(
            &route.execution_context,
            &edge[0],
            &edge[1],
            call,
            &dispatch_profile.output,
        ));
    }
    steps.push(observed_direct_call_step(
        &route.execution_context,
        &route.binding_entry,
        &binding_call.target,
        binding_call,
        &binding_profile.output,
    ));
    let mut effects = vec![FlowEffectEvidence {
        kind: "event".to_owned(),
        evidence: EvidenceLevel::Reviewed,
        function: route.binding_entry.clone(),
        site: binding_call.site,
        operation: binding_call.semantic_operation.clone(),
        detail: format!(
            "reviewed event-init role; exact call binds static object {bound_object} to callback pointer {expected_callback} ({})",
            route.callback_function
        ),
        constant: Some(u64::from(callback_address)),
        access: Some("bind".to_owned()),
        width: Some(32),
        address: Some(callback_address),
        register: None,
        value: Some(expected_callback.clone()),
        origin_path: None,
    }];
    let mut dispatch_queues = BTreeSet::new();
    for site in &route.dispatch_sites {
        let call = stored_exact_call(
            &dispatcher,
            &route.dispatch_call,
            *site,
            &route.id,
            "event enqueue",
        )?;
        let object = stored_exact_argument(
            call,
            route.dispatch_object_argument,
            &route.id,
            "dispatch object",
        )?;
        if object != bound_object {
            return Err(route_mismatch(
                &route.id,
                "event enqueue object differs from initialized callback object",
            ));
        }
        dispatch_queues.insert(
            stored_exact_argument(
                call,
                route.dispatch_queue_argument,
                &route.id,
                "dispatch queue",
            )?
            .to_owned(),
        );
        steps.push(observed_direct_call_step(
            &route.execution_context,
            &route.dispatcher,
            &call.target,
            call,
            &dispatch_profile.output,
        ));
        effects.push(FlowEffectEvidence {
            kind: "queue".to_owned(),
            evidence: EvidenceLevel::Reviewed,
            function: route.dispatcher.clone(),
            site: call.site,
            operation: call.semantic_operation.clone(),
            detail: format!(
                "reviewed source124/event-enqueue association; exact R9 call chain reaches a call carrying static object {object}"
            ),
            constant: parse_constant(object).map(u64::from),
            access: None,
            width: Some(32),
            address: parse_constant(object),
            register: None,
            value: Some(object.to_owned()),
            origin_path: None,
        });
    }
    steps.push(observed_direct_call_step(
        &route.execution_context,
        &route.delivery_entry,
        &receive_call.target,
        receive_call,
        &delivery_profile.output,
    ));
    steps.push(observed_direct_call_step(
        &route.execution_context,
        &route.delivery_entry,
        &run_call.target,
        run_call,
        &delivery_profile.output,
    ));
    steps.push(reviewed_stateful_callback_step(
        route,
        run_call.site,
        run_event,
        run_event_exact,
        run_call.argument_shapes(),
        receive_result_flow_proven,
        &delivery_profile.output,
    ));
    effects.extend([
        FlowEffectEvidence {
            kind: "queue".to_owned(),
            evidence: EvidenceLevel::Reviewed,
            function: route.delivery_entry.clone(),
            site: receive_call.site,
            operation: receive_call.semantic_operation.clone(),
            detail: format!(
                "reviewed stateful delivery contract receives an event pointer from queue {receive_queue}"
            ),
            constant: parse_constant(receive_queue).map(u64::from),
            access: Some("receive".to_owned()),
            width: Some(32),
            address: parse_constant(receive_queue),
            register: None,
            value: Some(receive_queue.to_owned()),
            origin_path: None,
        },
        FlowEffectEvidence {
            kind: "event".to_owned(),
            evidence: EvidenceLevel::Reviewed,
            function: route.delivery_entry.clone(),
            site: run_call.site,
            operation: run_call.semantic_operation.clone(),
            detail: delivery_order.as_ref().map_or_else(
                || format!(
                    "reviewed receive/run contract dispatches the callback stored by event.init; current CFG does not prove receive before run, and generated run argument is {run_event}"
                ),
                |order| format!(
                    "reviewed receive/run contract dispatches the callback stored by event.init; {} proves receive block {} precedes run block {}, and generated run argument is {run_event}",
                    order.proof,
                    order.earlier_block,
                    order.later_block,
                ),
            ),
            constant: None,
            access: Some("dispatch".to_owned()),
            width: Some(32),
            address: Some(callback_address),
            register: None,
            value: Some(expected_callback.clone()),
            origin_path: None,
        },
    ]);
    let mut blockers = vec![stateful_delivery_blocker(&dispatch_queues, receive_queue)];
    if let Some(blocker) =
        receive_result_flow_blocker(run_event, run_event_exact, receive_result_flow_proven)
    {
        blockers.push(blocker);
    }
    if let Some(blocker) = receive_run_order_blocker(delivery_order.is_some()) {
        blockers.push(blocker);
    }
    let examined_edges = steps.len();
    let loaded_functions = route.upstream_chain.len() + 3;
    let analysis_limited =
        enforce_reviewed_route_limit(&mut steps, &mut effects, request.max_depth, &mut blockers);
    finalize_event_route_semantic_evidence(&route.id, &mut effects)?;
    for (ordinal, step) in steps.iter_mut().enumerate() {
        step.ordinal = ordinal;
    }
    let target = if analysis_limited {
        steps.last().map(|step| step.callee.clone())
    } else {
        Some(route.callback_function.clone())
    };
    Ok(FlowInvestigationReport {
        schema_version: 4,
        command: "inspect flow",
        mode: "event-route",
        status: FlowStatus::Incomplete,
        profile: route.profile.clone(),
        linked_ir: dispatch_profile.output.display().to_string(),
        root: route.upstream_chain[0].clone(),
        target_kind: Some("static-event-callback".to_owned()),
        target,
        route: Some(route.id.clone()),
        claims: reviewed_callback_route_claims(!analysis_limited),
        limits: FlowLimits {
            max_depth: request.max_depth,
            visited_nodes: loaded_functions,
            examined_edges,
            loaded_functions,
            reached: analysis_limited.then(|| "max-depth".to_owned()),
            ..FlowLimits::new(request.max_depth)
        },
        steps,
        effects,
        rust_boundaries: Vec::new(),
        blockers,
    })
}

fn investigate_broker_subscription(
    request: EventRouteFlowRequest<'_>,
    context: &mut EventRouteEvaluationContext<'_>,
    route: &crate::function_workspace::ReviewedBrokerSubscriptionRoute,
) -> Result<FlowInvestigationReport> {
    let project = context.project;
    let dispatch_profile = profile(project, &route.profile)?;
    let reader = context.reader(&dispatch_profile.output)?;
    let dispatcher = route_function(
        &reader,
        &route.id,
        &route.source,
        &route.dispatcher,
        "broker publisher",
    )?;
    let publish = stored_exact_call(
        &dispatcher,
        &route.dispatch_call,
        route.dispatch_site,
        &route.id,
        "broker publish",
    )?;
    let selector = format!("const:{:#010x}", route.selector_value);
    if stored_exact_argument(
        publish,
        route.dispatch_selector_argument,
        &route.id,
        "broker selector",
    )? != selector
    {
        return Err(route_mismatch(&route.id, "broker selector changed"));
    }
    let payload = stored_exact_argument(
        publish,
        route.dispatch_payload_argument,
        &route.id,
        "broker payload",
    )?;
    if payload != route.payload_value {
        return Err(route_mismatch(&route.id, "broker payload changed"));
    }
    let domain_profile = profile(project, &route.domain.profile)?;
    let domain_reader = context.reader(&domain_profile.output)?;
    let domain_owner = route_function(
        &domain_reader,
        &route.id,
        &route.domain.source,
        &route.domain.entry,
        "broker domain owner",
    )?;
    let domain = stored_exact_call(
        &domain_owner,
        &route.domain.call,
        route.domain.call_site,
        &route.id,
        "broker attach",
    )?;
    if stored_exact_argument(
        publish,
        route.domain.dispatch_argument,
        &route.id,
        "published broker object",
    )? != stored_exact_argument(
        domain,
        route.domain.call_object_argument,
        &route.id,
        "attached broker object",
    )? || stored_exact_argument(
        domain,
        route.domain.call_selector_argument,
        &route.id,
        "attached source id",
    )? != format!("const:{:#010x}", route.domain.selector_value)
    {
        return Err(route_mismatch(
            &route.id,
            "broker publish is not tied to the reviewed attached source domain",
        ));
    }
    let binding_profile = profile(project, &route.binding_profile)?;
    let binding_reader = context.reader(&binding_profile.output)?;
    let binding = route_function(
        &binding_reader,
        &route.id,
        &route.binding_source,
        &route.binding_entry,
        "broker subscription owner",
    )?;
    let subscribe = stored_exact_call(
        &binding,
        &route.binding_call,
        route.binding_site,
        &route.id,
        "broker subscribe",
    )?;
    let subscriber_object = stored_exact_argument(
        subscribe,
        route.binding_object_argument,
        &route.id,
        "subscriber object",
    )?;
    if !stored_call_argument_establishes_constant(
        subscribe,
        route.binding_domain_argument,
        route.domain.selector_value,
        &route.id,
        "subscriber domain",
    )? {
        return Err(route_mismatch(
            &route.id,
            "broker subscription is not proven to use the attached source domain",
        ));
    }
    let callback_profile = profile(project, &route.callback_profile)?;
    let callback_reader = context.reader(&callback_profile.output)?;
    let callback = route_function(
        &callback_reader,
        &route.id,
        &route.callback_source,
        &route.callback_function,
        "broker callback",
    )?;
    let callback_address = callback
        .address
        .ok_or_else(|| route_mismatch(&route.id, "broker callback has no linked address"))?;
    let callback_value = format!("const:{callback_address:#010x}");
    let callback_stores = binding
        .instruction_effects
        .iter()
        .filter_map(|effect| {
            callback_write_matches(
                effect,
                route.binding_callback_store_site,
                route.binding_callback_store_offset,
                &callback_value,
                subscriber_object,
            )
        })
        .collect::<BTreeSet<_>>()
        .len();
    if callback_stores != 1 {
        return Err(route_mismatch(
            &route.id,
            format!(
                "expected one callback-pointer store at {:#010x}, found {callback_stores}",
                route.binding_callback_store_site
            ),
        ));
    }
    let binding_artifact = binding_reader.authenticated_source_artifact(&route.binding_source)?;
    let binding_body = artifact::inspect_function_body_at_data(
        &binding_artifact.path,
        &binding_artifact.bytes,
        binding.member.as_deref(),
        &binding.symbol,
        binding.address.map(u64::from),
    )?;
    let callback_store_order = super::cfg::must_execute_before(
        &binding_body,
        route.binding_callback_store_site,
        route.binding_site,
    );
    let case = stored_exact_direct_call(
        &callback,
        &route.case_handler.function,
        route.case_handler_site,
        &route.id,
        "selector case",
    )?;
    let case_guards = case.guard_expressions();
    if case_guards.is_empty()
        || !case_guards.iter().all(|path| {
            guard_establishes_selector(path, route.callback_selector_argument, route.selector_value)
        })
    {
        return Err(route_mismatch(&route.id, "callback selector guard changed"));
    }
    let mut steps = vec![
        observed_direct_call_step(
            &route.execution_context,
            &route.domain.entry,
            &domain.target,
            domain,
            &domain_profile.output,
        ),
        observed_direct_call_step(
            &route.execution_context,
            &route.binding_entry,
            &subscribe.target,
            subscribe,
            &binding_profile.output,
        ),
        observed_direct_call_step(
            &route.execution_context,
            &route.dispatcher,
            &publish.target,
            publish,
            &dispatch_profile.output,
        ),
        observed_direct_call_step(
            &route.execution_context,
            &route.callback_function,
            &route.case_handler.function,
            case,
            &callback_profile.output,
        ),
    ];
    let mut effects = vec![
        FlowEffectEvidence {
            kind: "memory".to_owned(),
            evidence: EvidenceLevel::Reviewed,
            function: route.binding_entry.clone(),
            site: Some(route.binding_callback_store_site),
            operation: Some("callback-pointer-store".to_owned()),
            detail: callback_store_order.as_ref().map_or_else(
                || {
                    format!(
                        "store {} into the exact subscribed object at offset {:#x}",
                        route.callback_function, route.binding_callback_store_offset
                    )
                },
                |order| {
                    format!(
                        "store {} into the exact subscribed object at offset {:#x}; {} proves store block {} precedes subscription block {}",
                        route.callback_function,
                        route.binding_callback_store_offset,
                        order.proof,
                        order.earlier_block,
                        order.later_block,
                    )
                },
            ),
            constant: Some(u64::from(callback_address)),
            access: Some("write".to_owned()),
            width: Some(32),
            address: None,
            register: None,
            value: Some(callback_value),
            origin_path: None,
        },
        FlowEffectEvidence {
            kind: "event".to_owned(),
            evidence: EvidenceLevel::Reviewed,
            function: route.dispatcher.clone(),
            site: publish.site,
            operation: publish
                .semantic_operation
                .clone()
                .or_else(|| Some(publish.target.clone())),
            detail: format!(
                "reviewed {} publication: {}={selector}, {}={payload}",
                route.mechanism, route.selector_role, route.payload_role
            ),
            constant: Some(u64::from(route.selector_value)),
            access: None,
            width: Some(32),
            address: None,
            register: None,
            value: Some(payload.to_owned()),
            origin_path: None,
        },
    ];
    let mut target = Some(route.case_handler.function.clone());
    let mut loaded_functions = 4;
    let mut blockers = Vec::new();
    let mut analysis_limited = false;
    if let Some(terminal) = &route.terminal {
        let remaining_depth = request.max_depth.saturating_sub(steps.len());
        if remaining_depth == 0 {
            analysis_limited = true;
            blockers.push(FlowBlocker::manual(
                "analysis-limit",
                format!(
                    "reviewed broker route consumes the requested {} edges before its terminal continuation",
                    request.max_depth
                ),
                "increase --max-depth to inspect the terminal continuation",
            ));
        } else {
            let mut segment = context.investigate_target(TargetFlowRequest {
                source: &route.case_handler.source,
                root_symbol: identity_symbol(&route.case_handler.function),
                target: FlowTargetRequest::Function(&terminal.function),
                max_depth: remaining_depth,
                max_loaded_functions: super::MAX_LOADED_FUNCTIONS.saturating_sub(loaded_functions),
            })?;
            let base = steps.len();
            for (index, step) in segment.steps.iter_mut().enumerate() {
                step.ordinal = base + index;
            }
            steps.extend(segment.steps);
            effects.extend(segment.effects);
            analysis_limited |= segment
                .blockers
                .iter()
                .any(|blocker| blocker.kind == "analysis-limit");
            blockers.extend(segment.blockers);
            loaded_functions += segment.limits.loaded_functions;
            target = Some(terminal.function.clone());
        }
    }
    if let Some(blocker) = callback_store_dominance_blocker(callback_store_order.is_some()) {
        blockers.push(blocker);
    }
    let examined_edges = steps.len();
    analysis_limited |=
        enforce_reviewed_route_limit(&mut steps, &mut effects, request.max_depth, &mut blockers);
    finalize_event_route_semantic_evidence(&route.id, &mut effects)?;
    if analysis_limited {
        target = steps.last().map(|step| step.callee.clone());
    }
    blockers.extend(broker_delivery_blockers());
    for (ordinal, step) in steps.iter_mut().enumerate() {
        step.ordinal = ordinal;
    }
    Ok(FlowInvestigationReport {
        schema_version: 4,
        command: "inspect flow",
        mode: "event-route",
        status: FlowStatus::Incomplete,
        profile: route.profile.clone(),
        linked_ir: dispatch_profile.output.display().to_string(),
        root: route.dispatcher.clone(),
        target_kind: Some("broker-subscription".to_owned()),
        target,
        route: Some(route.id.clone()),
        claims: reviewed_callback_route_claims(!analysis_limited),
        limits: FlowLimits {
            max_depth: request.max_depth,
            visited_nodes: loaded_functions,
            examined_edges,
            loaded_functions,
            reached: analysis_limited.then(|| "max-depth".to_owned()),
            ..FlowLimits::new(request.max_depth)
        },
        steps,
        effects,
        rust_boundaries: Vec::new(),
        blockers,
    })
}

fn broker_delivery_blockers() -> [FlowBlocker; 2] {
    [
        FlowBlocker::manual(
            "broker-subscriber-lifetime-unproven",
            "attach, callback store and subscription are exact, but initialization, insertion lifetime and unsubscribe ordering are not joined into one broker epoch",
            "join the exact subscribe and unsubscribe paths to the publisher epoch, or replay that broker epoch",
        ),
        FlowBlocker::manual(
            "broker-prior-listener-result-unproven",
            "the selector guard and callback continuation are exact, but earlier listeners in the broker walk can stop delivery and their selector-specific return values are not modeled",
            "prove every preceding listener continues for this selector, or replay the exact listener ordering and return values",
        ),
    ]
}

fn route_function(
    reader: &artifacts::LinkedIrReader,
    route: &str,
    source: &str,
    identity: &str,
    role: &str,
) -> Result<artifacts::StoredFunction> {
    match reader.get_function_by_identity(identity)? {
        Some(function) if function.source == source => Ok(function),
        Some(_) => Err(route_mismatch(route, format!("{role} source changed"))),
        None => Err(route_mismatch(
            route,
            format!("{role} {identity:?} is absent"),
        )),
    }
}

fn stored_exact_call<'a>(
    function: &'a artifacts::StoredFunction,
    matcher: &crate::function_workspace::ReviewedEventCallMatcher,
    site: u32,
    route: &str,
    role: &str,
) -> Result<&'a artifacts::StoredCall> {
    let matches = function
        .calls
        .iter()
        .filter(|call| {
            call.site == Some(site)
                && match matcher {
                    crate::function_workspace::ReviewedEventCallMatcher::Operation(operation) => {
                        call.semantic_operation.as_deref() == Some(operation)
                    }
                    crate::function_workspace::ReviewedEventCallMatcher::Function(target) => {
                        call.target == *target
                    }
                }
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [call] if call.direct() => Ok(*call),
        [_call] => Err(route_mismatch(
            route,
            format!("{role} call at {site:#010x} is indirect"),
        )),
        _ => Err(route_mismatch(
            route,
            format!(
                "expected one {role} call at {site:#010x}, found {}",
                matches.len()
            ),
        )),
    }
}

fn stored_exact_direct_call<'a>(
    function: &'a artifacts::StoredFunction,
    target: &str,
    site: u32,
    route: &str,
    role: &str,
) -> Result<&'a artifacts::StoredCall> {
    let call = stored_exact_call(
        function,
        &crate::function_workspace::ReviewedEventCallMatcher::Function(target.to_owned()),
        site,
        route,
        role,
    )?;
    if call.kind != "internal" {
        return Err(route_mismatch(
            route,
            format!("{role} call at {site:#010x} is not internal"),
        ));
    }
    Ok(call)
}

fn stored_exact_argument<'a>(
    call: &'a artifacts::StoredCall,
    argument: u8,
    route: &str,
    role: &str,
) -> Result<&'a str> {
    let value = call
        .arguments
        .get(usize::from(argument))
        .map(String::as_str)
        .ok_or_else(|| route_mismatch(route, format!("{role} argument is absent")))?;
    if !call.argument_is_exact(usize::from(argument)) {
        return Err(route_mismatch(
            route,
            format!("{role} is unresolved: {value}"),
        ));
    }
    Ok(value)
}

fn guard_establishes_selector(path: &str, argument: u8, selector: u32) -> bool {
    let left = format!("arg{argument} == {selector:#010x}");
    let right = format!("{selector:#010x} == arg{argument}");
    path.split(" && ").any(|clause| {
        let clause = clause.trim().trim_start_matches('(').trim_end_matches(')');
        clause == left || clause == right
    })
}

fn stored_call_argument_establishes_constant(
    call: &artifacts::StoredCall,
    argument: u8,
    constant: u32,
    route: &str,
    role: &str,
) -> Result<bool> {
    let value = stored_exact_argument(call, argument, route, role)?;
    if value == format!("const:{constant:#010x}") {
        return Ok(true);
    }
    let paths = call.guard_expressions();
    Ok(!paths.is_empty()
        && paths
            .iter()
            .all(|path| guard_establishes_value(path, value, constant)))
}

fn guard_establishes_value(path: &str, value: &str, constant: u32) -> bool {
    let equal = format!("({value} == {constant:#010x})");
    let reverse_equal = format!("({constant:#010x} == {value})");
    let negated_unequal = format!("!({value} != {constant:#010x})");
    let reverse_negated_unequal = format!("!({constant:#010x} != {value})");
    path.split(" && ").any(|clause| {
        let clause = clause.trim();
        clause == equal
            || clause == reverse_equal
            || clause == negated_unequal
            || clause == reverse_negated_unequal
    })
}

fn normalize_word_value(value: &str) -> &str {
    value
        .strip_suffix("&0xffffffff|0x00000000")
        .unwrap_or(value)
}

fn callback_write_matches<'a>(
    effect: &'a artifacts::StoredInstructionEffect,
    site: u32,
    offset: i64,
    callback: &str,
    subscriber_object: &str,
) -> Option<(u32, i64, &'a str)> {
    let artifacts::StoredInstructionEffect::Memory {
        site: observed_site,
        access,
        width: 32,
        object,
        offset: observed_offset,
        value: Some(value),
        ..
    } = effect
    else {
        return None;
    };
    (*observed_site == site
        && access == "write"
        && *observed_offset == offset
        && value == callback
        && stored_memory_object_value_expression(object)
            .is_some_and(|object| object == normalize_word_value(subscriber_object)))
    .then_some((*observed_site, *observed_offset, value.as_str()))
}

fn stored_memory_object_value_expression(object: &artifacts::StoredMemoryObject) -> Option<String> {
    let artifacts::StoredMemoryObject::Dereferenced {
        pointer,
        pointer_offset,
    } = object
    else {
        return None;
    };
    Some(format!(
        "memory:{}{}",
        stored_memory_address_expression(pointer)?,
        signed_offset(*pointer_offset)
    ))
}

fn stored_memory_address_expression(object: &artifacts::StoredMemoryObject) -> Option<String> {
    match object {
        artifacts::StoredMemoryObject::Absolute { address, .. } => {
            Some(format!("absolute:{address:#010x}"))
        }
        artifacts::StoredMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => Some(format!(
            "*({}{})",
            stored_memory_address_expression(pointer)?,
            signed_offset(*pointer_offset)
        )),
        _ => None,
    }
}

fn enforce_reviewed_route_limit(
    steps: &mut Vec<FlowStepEvidence>,
    effects: &mut Vec<FlowEffectEvidence>,
    max_depth: usize,
    blockers: &mut Vec<FlowBlocker>,
) -> bool {
    if steps.len() <= max_depth {
        return false;
    }
    steps.truncate(max_depth);
    let retained_sites = steps
        .iter()
        .filter_map(|step| step.site)
        .collect::<BTreeSet<_>>();
    effects.retain(|effect| {
        effect
            .site
            .is_some_and(|site| retained_sites.contains(&site))
    });
    if !blockers
        .iter()
        .any(|blocker| blocker.kind == "analysis-limit")
    {
        blockers.push(FlowBlocker::manual(
            "analysis-limit",
            format!("reviewed route requires more than the requested {max_depth} edges"),
            "increase --max-depth to inspect the remaining reviewed route stages",
        ));
    }
    true
}

fn signed_offset(offset: i64) -> String {
    if offset < 0 {
        format!("-{:#x}", offset.unsigned_abs())
    } else {
        format!("+{:#x}", offset as u64)
    }
}

/// Preserve the generated exact call edge without projecting a reviewed route
/// role (enqueue, subscription, callback, and so on) into observed evidence.
fn observed_direct_call_step(
    context: &str,
    caller: &str,
    callee: &str,
    call: &artifacts::StoredCall,
    origin: &std::path::Path,
) -> FlowStepEvidence {
    FlowStepEvidence {
        ordinal: 0,
        evidence: EvidenceLevel::Observed,
        context_evidence: EvidenceLevel::Reviewed,
        context: context.to_owned(),
        caller: caller.to_owned(),
        callee: callee.to_owned(),
        site: call.site,
        kind: "direct-call".to_owned(),
        tail: call.tail(),
        argument_shapes: call.argument_shapes(),
        arguments: call
            .arguments
            .iter()
            .enumerate()
            .map(|(position, value)| FlowArgumentEvidence {
                position,
                local: format!("a{position}"),
                resolved: value.clone(),
                constants: parse_constant(value).into_iter().collect(),
                provenance: "generated-linked-ir",
                pointee: Vec::new(),
            })
            .collect(),
        guards: call.guard_expressions(),
        origin: origin.display().to_string(),
    }
}

/// Project the reviewed NPL state transition without pretending that the
/// opaque platform boundary is an observed indirect call. `event.init` owns
/// the callback binding, while the generated IR currently exposes the
/// `event.run` argument. Queue identity, receive-result dataflow, and delivery
/// remain separate obligations handled by the caller.
fn reviewed_stateful_callback_step(
    route: &crate::function_workspace::ReviewedStaticEventCallbackRoute,
    site: Option<u32>,
    run_event: &str,
    run_event_exact: bool,
    run_argument_shapes: usize,
    receive_result_flow_proven: bool,
    origin: &std::path::Path,
) -> FlowStepEvidence {
    FlowStepEvidence {
        ordinal: 0,
        evidence: EvidenceLevel::Reviewed,
        context_evidence: EvidenceLevel::Reviewed,
        context: route.execution_context.clone(),
        caller: route.delivery_entry.clone(),
        callee: route.callback_function.clone(),
        site,
        kind: "stateful-callback-dispatch".to_owned(),
        tail: false,
        argument_shapes: run_argument_shapes,
        arguments: vec![FlowArgumentEvidence {
            position: usize::from(route.run_event_argument),
            local: "event-run-argument".to_owned(),
            resolved: run_event.to_owned(),
            constants: Vec::new(),
            provenance: if receive_result_flow_proven {
                "generated-linked-ir typed direct receive-result provenance; same-basic-block CFG witness"
            } else if run_event_exact {
                "generated-linked-ir-exact; receive-result relation unproven"
            } else {
                "generated-linked-ir-non-exact; receive-result relation unproven"
            },
            pointee: Vec::new(),
        }],
        guards: Vec::new(),
        origin: origin.display().to_string(),
    }
}

fn receive_result_flow_blocker(
    run_event: &str,
    run_event_exact: bool,
    proven: bool,
) -> Option<FlowBlocker> {
    if proven {
        return None;
    }
    let precision = if run_event_exact {
        "exact"
    } else {
        "non-exact"
    };
    Some(FlowBlocker::manual(
        "event-receive-result-flow-unproven",
        format!(
            "event.run argument is generated {precision} value {run_event}, but linked IR and the local CFG do not prove that it is the direct result returned by eventq_get"
        ),
        "preserve typed result identity through eventq_get and event.run and require a same-basic-block CFG witness for the receive-to-run relation",
    ))
}

fn receive_run_order_blocker(proven: bool) -> Option<FlowBlocker> {
    (!proven).then(|| {
        FlowBlocker::manual(
            "event-receive-run-order-unproven",
            "the current conservative CFG does not prove that eventq_get executes before the reached event.run call",
            "preserve a complete common CFG witness for eventq_get and event.run",
        )
    })
}

fn reviewed_callback_route_claims(structural_navigation: bool) -> FlowClaims {
    FlowClaims {
        structural_navigation,
        path_feasibility: false,
        event_delivery: false,
        executable_equivalence: false,
    }
}

fn stateful_delivery_blocker(
    dispatch_queues: &BTreeSet<String>,
    receive_queue: &str,
) -> FlowBlocker {
    if dispatch_queues.len() == 1 && dispatch_queues.contains(receive_queue) {
        FlowBlocker::manual(
            "event-delivery-not-replayed",
            format!(
                "the stateful callback contract and exact queue value {receive_queue} join enqueue to receive/run, but no execution replay proves this queue instance"
            ),
            "replay this exact queue instance and retain enqueue, receive, run, and callback evidence",
        )
    } else {
        FlowBlocker::manual(
            "event-queue-instance-unresolved",
            format!(
                "stateful event.init/receive/run callback dispatch is modeled, but enqueue queue values {:?} do not resolve to receive queue {receive_queue}",
                dispatch_queues
            ),
            "resolve the enqueue-side queue producer to the exact receive queue value, then replay that queue instance",
        )
    }
}

fn callback_store_dominance_blocker(proven: bool) -> Option<FlowBlocker> {
    (!proven).then(|| {
        FlowBlocker::manual(
            "callback-store-dominance-unproven",
            "the callback store and subscription object are exact, but the current conservative CFG does not prove that the store executes before every reached subscription",
            "preserve a complete common CFG witness for the callback store and subscription call",
        )
    })
}

/// Semantic operation names in an event-route report originate in reviewed
/// interface/route knowledge. Generated IR can independently prove an exact
/// call edge or raw instruction effect, but it cannot promote that reviewed
/// interpretation to `Observed`.
fn finalize_event_route_semantic_evidence(
    route: &str,
    effects: &mut [FlowEffectEvidence],
) -> Result<()> {
    for effect in effects.iter_mut() {
        if effect.operation.is_some() && effect.evidence == EvidenceLevel::Observed {
            effect.evidence = EvidenceLevel::Reviewed;
        }
    }
    validate_event_route_semantic_evidence(route, effects)
}

fn validate_event_route_semantic_evidence(
    route: &str,
    effects: &[FlowEffectEvidence],
) -> Result<()> {
    if let Some(effect) = effects
        .iter()
        .find(|effect| effect.operation.is_some() && effect.evidence == EvidenceLevel::Observed)
    {
        return Err(route_mismatch(
            route,
            format!(
                "semantic operation {:?} at {:?} is incorrectly labeled observed",
                effect.operation, effect.site
            ),
        ));
    }
    Ok(())
}

fn parse_constant(value: &str) -> Option<u32> {
    value
        .strip_prefix("const:0x")
        .and_then(|value| u32::from_str_radix(value, 16).ok())
}

fn route_mismatch(route: &str, message: impl std::fmt::Display) -> crate::Error {
    crate::Error::invalid(format!("event route {route:?} {message}"))
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

fn load_semantic_catalogs(project: &ProjectSpec) -> Result<SemanticCatalogs> {
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
    route: &crate::function_workspace::ReviewedSelectorEventRoute,
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
        ReviewedEventCaseHandler, ReviewedEventDelivery, ReviewedEventReplay,
        ReviewedEventStateModel, ReviewedSelectorEventRoute,
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

    fn fixture_step(site: u32) -> FlowStepEvidence {
        FlowStepEvidence {
            ordinal: 0,
            evidence: EvidenceLevel::Observed,
            context_evidence: EvidenceLevel::Reviewed,
            context: "fixture".to_owned(),
            caller: "fixture::caller".to_owned(),
            callee: "fixture::callee".to_owned(),
            site: Some(site),
            kind: "direct-call".to_owned(),
            tail: false,
            argument_shapes: 1,
            arguments: Vec::new(),
            guards: Vec::new(),
            origin: "fixture.ir".to_owned(),
        }
    }

    fn fixture_effect(operation: Option<&str>, evidence: EvidenceLevel) -> FlowEffectEvidence {
        FlowEffectEvidence {
            kind: "event".to_owned(),
            evidence,
            function: "fixture::caller".to_owned(),
            site: Some(0x1000),
            operation: operation.map(str::to_owned),
            detail: "fixture".to_owned(),
            constant: None,
            access: None,
            width: None,
            address: None,
            register: None,
            value: None,
            origin_path: None,
        }
    }

    #[test]
    fn bulk_event_route_artifacts_open_each_linked_ir_bundle_once() {
        let directory = replay_test_directory();
        let bundle = directory.join("linked-ir");
        crate::artifacts::write_fixture_bundle(
            &bundle,
            &crate::artifacts::render_linked_ir_fixture(Vec::new(), Vec::new()),
        )
        .unwrap();
        let mut artifacts = EventRouteArtifacts::default();

        let first = artifacts.reader(&bundle).unwrap();
        let second = artifacts.reader(&bundle).unwrap();
        let catalogs = artifacts
            .catalogs_with(|| Ok(SemanticCatalogs::default()))
            .unwrap();
        let cached_catalogs = artifacts
            .catalogs_with(|| panic!("cached semantic catalogs must not be loaded again"))
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&catalogs, &cached_catalogs));
        assert_eq!(artifacts.readers.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn event_route_rejects_observed_semantic_operation_claims() {
        let effects = vec![fixture_effect(
            Some("rtos.event-queue.send"),
            EvidenceLevel::Observed,
        )];

        let error = validate_event_route_semantic_evidence("fixture-route", &effects)
            .expect_err("reviewed semantic operation must not be observed evidence");
        assert!(error.to_string().contains("incorrectly labeled observed"));
    }

    #[test]
    fn event_route_output_separates_reviewed_semantics_from_observed_raw_effects() {
        let mut effects = vec![
            fixture_effect(Some("rtos.event-queue.send"), EvidenceLevel::Observed),
            fixture_effect(None, EvidenceLevel::Observed),
        ];

        finalize_event_route_semantic_evidence("fixture-route", &mut effects).unwrap();

        assert_eq!(effects[0].evidence, EvidenceLevel::Reviewed);
        assert_eq!(effects[1].evidence, EvidenceLevel::Observed);
        let output = serde_json::json!({
            "steps": [fixture_step(0x1000)],
            "effects": effects,
        });
        assert_eq!(output["steps"][0]["evidence"], "observed");
        assert_eq!(output["steps"][0]["kind"], "direct-call");
        assert_eq!(output["effects"][0]["evidence"], "reviewed");
        assert_eq!(output["effects"][1]["evidence"], "observed");
    }

    #[test]
    fn reviewed_route_steps_obey_the_requested_depth() {
        let mut steps = vec![fixture_step(0x1000), fixture_step(0x1004)];
        let mut effects = Vec::new();
        let mut blockers = Vec::new();
        assert!(enforce_reviewed_route_limit(
            &mut steps,
            &mut effects,
            1,
            &mut blockers
        ));
        assert_eq!(steps.len(), 1);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "analysis-limit");
    }

    #[test]
    fn stateful_delivery_fails_closed_when_queue_instances_do_not_join() {
        let dispatch_queues = BTreeSet::from([
            "result_of_fixture__queue_0x00001000".to_owned(),
            "result_of_fixture__queue_0x00001004".to_owned(),
        ]);

        let blocker = stateful_delivery_blocker(&dispatch_queues, "memory:fixture::queue+0x8");

        assert_eq!(blocker.kind, "event-queue-instance-unresolved");
    }

    #[test]
    fn stateful_delivery_requires_replay_after_exact_queue_join() {
        let queue = "memory:fixture::queue+0x8";
        let dispatch_queues = BTreeSet::from([queue.to_owned()]);

        let blocker = stateful_delivery_blocker(&dispatch_queues, queue);

        assert_eq!(blocker.kind, "event-delivery-not-replayed");
    }

    #[test]
    fn stateful_delivery_requires_typed_receive_result_flow() {
        let exact = receive_result_flow_blocker("memory:fixture::event", true, false)
            .expect("missing provenance must retain blocker");
        assert_eq!(exact.kind, "event-receive-result-flow-unproven");
        assert!(exact.message.contains("generated exact value"));

        let non_exact = receive_result_flow_blocker("varies-across-2-shapes", false, false)
            .expect("missing provenance must retain blocker");
        assert_eq!(non_exact.kind, "event-receive-result-flow-unproven");
        assert!(non_exact.message.contains("generated non-exact value"));

        assert!(receive_result_flow_blocker("varies-across-146-shapes", false, true).is_none());
    }

    #[test]
    fn stateful_delivery_order_closes_only_after_a_cfg_witness() {
        assert!(receive_run_order_blocker(true).is_none());
        assert_eq!(
            receive_run_order_blocker(false)
                .expect("missing witness must retain a blocker")
                .kind,
            "event-receive-run-order-unproven"
        );
    }

    #[test]
    fn reviewed_callback_routes_never_claim_delivery_or_path_feasibility() {
        for structural_navigation in [false, true] {
            let claims = reviewed_callback_route_claims(structural_navigation);
            assert_eq!(claims.structural_navigation, structural_navigation);
            assert!(!claims.path_feasibility);
            assert!(!claims.event_delivery);
            assert!(!claims.executable_equivalence);
        }
    }

    #[test]
    fn broker_dominance_blocker_closes_only_after_a_cfg_witness() {
        assert!(callback_store_dominance_blocker(true).is_none());
        assert_eq!(
            callback_store_dominance_blocker(false)
                .expect("missing witness must retain a blocker")
                .kind,
            "callback-store-dominance-unproven"
        );
    }

    #[test]
    fn broker_delivery_reports_lifetime_and_prior_listener_obligations_separately() {
        let blockers = broker_delivery_blockers();
        assert_eq!(blockers[0].kind, "broker-subscriber-lifetime-unproven");
        assert_eq!(blockers[1].kind, "broker-prior-listener-result-unproven");
    }

    #[test]
    fn callback_store_must_target_the_subscribed_object() {
        let effect = artifacts::StoredInstructionEffect::Memory {
            site: 0x1000,
            block: Some(1),
            access: "write".to_owned(),
            width: 32,
            object: artifacts::StoredMemoryObject::Dereferenced {
                pointer: Box::new(artifacts::StoredMemoryObject::Dereferenced {
                    pointer: Box::new(artifacts::StoredMemoryObject::Absolute {
                        address_space: "ram".to_owned(),
                        address: 0x2000,
                    }),
                    pointer_offset: 0,
                }),
                pointer_offset: 8,
            },
            offset: 0,
            paths: Vec::new(),
            value: Some("const:0x00003000".to_owned()),
            value_pseudo: None,
            write_mask: Some(u32::MAX),
            preserved_mask: None,
            forced_zero_mask: None,
            forced_one_mask: None,
        };
        let subscribed = "memory:*(absolute:0x00002000+0x0)+0x8&0xffffffff|0x00000000";
        assert!(
            callback_write_matches(&effect, 0x1000, 0, "const:0x00003000", subscribed).is_some()
        );
        assert!(
            callback_write_matches(
                &effect,
                0x1000,
                0,
                "const:0x00003000",
                "memory:*(absolute:0x00002000+0x0)+0xc&0xffffffff|0x00000000",
            )
            .is_none()
        );
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
        let mut route = ReviewedSelectorEventRoute {
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
