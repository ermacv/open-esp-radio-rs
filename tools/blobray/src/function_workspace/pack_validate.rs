//! Structural, provenance, completeness, and review-coverage validation.

use std::collections::BTreeSet;

use super::validation::{ValidationError, ValidationResult};
use super::{FunctionFacts, FunctionPack, FunctionWorkspaceSummary};

mod contexts;
mod primitives;
mod types;

#[cfg(test)]
mod broker_tests;

use contexts::{validate_function, validate_inputs};
use primitives::validate_id;
use types::validate_types;

pub(super) fn validate(
    pack: &FunctionPack,
    facts: &FunctionFacts,
) -> ValidationResult<FunctionWorkspaceSummary> {
    validate_with_callback_facts(pack, facts, true)
}

pub(super) fn validate_summary(
    pack: &FunctionPack,
    facts: &FunctionFacts,
) -> ValidationResult<FunctionWorkspaceSummary> {
    validate_with_callback_facts(pack, facts, false)
}

fn validate_with_callback_facts(
    pack: &FunctionPack,
    facts: &FunctionFacts,
    exact_callback_facts: bool,
) -> ValidationResult<FunctionWorkspaceSummary> {
    validate_id(&pack.id, "function pack id")
        .map_err(|message| ValidationError::pack("id", message))?;
    validate_inputs(&pack.inputs, &facts.inputs)?;
    let mut summary = FunctionWorkspaceSummary {
        inputs: pack.inputs.len(),
        observed_functions: facts.root_functions().count(),
        ..FunctionWorkspaceSummary::default()
    };
    let mut keys = BTreeSet::new();
    let mut reviewed_names = BTreeSet::new();
    for reviewed in &pack.functions {
        let key = (&reviewed.profile, &reviewed.source, &reviewed.identity);
        if !keys.insert(key) {
            return Err(ValidationError::function(
                reviewed,
                "identity",
                format!(
                    "duplicate reviewed function {}:{}",
                    reviewed.profile, reviewed.identity
                ),
            ));
        }
        let fact = facts
            .function(&reviewed.profile, &reviewed.source, &reviewed.identity)
            .ok_or_else(|| {
                ValidationError::function(
                    reviewed,
                    "identity",
                    format!(
                        "stale reviewed function {}:{}:{}",
                        reviewed.profile, reviewed.source, reviewed.identity
                    ),
                )
            })?;
        validate_function(reviewed, fact, &mut summary, &mut reviewed_names)?;
    }
    for fact in facts.root_functions() {
        if !keys.contains(&(&fact.profile, &fact.source, &fact.identity)) {
            summary.unreviewed_functions += 1;
            summary.unreviewed_fields += fact.context_fields.len();
            summary.unreviewed_contexts += fact
                .context_fields
                .iter()
                .map(|field| field.argument)
                .collect::<BTreeSet<_>>()
                .len();
        }
    }
    validate_types(&pack.types, facts, &mut summary)?;
    validate_event_routes(pack, facts, &mut summary, exact_callback_facts)?;
    Ok(summary)
}

fn validate_event_routes(
    pack: &FunctionPack,
    facts: &FunctionFacts,
    summary: &mut FunctionWorkspaceSummary,
    exact_callback_facts: bool,
) -> ValidationResult<()> {
    let mut ids = BTreeSet::new();
    for reviewed in &pack.event_routes {
        validate_id(reviewed.id(), "event route id")
            .map_err(|message| ValidationError::pack("event-routes", message))?;
        if !ids.insert(reviewed.id()) {
            return Err(ValidationError::pack(
                "event-routes",
                format!("duplicate event route id {:?}", reviewed.id()),
            ));
        }
        let route = match reviewed {
            super::ReviewedEventRoute::SelectorDelivery(route) => route,
            super::ReviewedEventRoute::StaticEventCallback(route) => {
                if exact_callback_facts {
                    validate_static_event_callback(route, facts)?;
                } else {
                    validate_static_event_callback_shape(route)?;
                }
                summary.event_routes += 1;
                continue;
            }
            super::ReviewedEventRoute::BrokerSubscription(route) => {
                if exact_callback_facts {
                    validate_broker_subscription(route, facts)?;
                } else {
                    validate_broker_subscription_shape(route)?;
                }
                summary.event_routes += 1;
                continue;
            }
        };
        let dispatcher = facts
            .function(&route.profile, &route.source, &route.dispatcher)
            .ok_or_else(|| {
                ValidationError::pack(
                    "event-routes",
                    format!("event route {:?} refers to an unknown dispatcher", route.id),
                )
            })?;
        let selector = format!("const:{:#010x}", route.selector_value);
        let matching = dispatcher.event_dispatches.iter().any(|dispatch| {
            dispatch.mechanism == route.mechanism
                && dispatch.execution_context == route.execution_context
                && route
                    .receiver
                    .as_ref()
                    .is_none_or(|receiver| dispatch.receiver.as_ref() == Some(receiver))
                && dispatch.interface_complete
                && dispatch
                    .bindings
                    .iter()
                    .any(|(role, value)| role == &route.selector_role && value == &selector)
        });
        if !matching {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} is not backed by a complete {} dispatch with {}={}",
                    route.id, route.mechanism, route.selector_role, selector
                ),
            ));
        }
        let consumer = facts
            .function(
                &route.consumer_profile,
                &route.consumer_source,
                &route.consumer_entry,
            )
            .ok_or_else(|| {
                ValidationError::pack(
                    "event-routes",
                    format!(
                        "event route {:?} refers to an unknown consumer entry",
                        route.id
                    ),
                )
            })?;
        if !consumer.calls.iter().any(|call| {
            call.semantic_operation.as_deref() == Some(route.delivery.operation.as_str())
        }) && !consumer
            .semantic_operations
            .iter()
            .any(|operation| operation == &route.delivery.operation)
        {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} consumer does not call delivery operation {:?}",
                    route.id, route.delivery.operation
                ),
            ));
        }
        if route.delivery.output_role.trim().is_empty() {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} has an empty delivery output role",
                    route.id
                ),
            ));
        }
        if !matches!(route.delivery.selector_width, 8 | 16 | 32) {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} selector width must be 8, 16, or 32 bits",
                    route.id
                ),
            ));
        }
        let maximum = match route.delivery.selector_width {
            8 => u8::MAX.into(),
            16 => u16::MAX.into(),
            32 => u32::MAX,
            _ => unreachable!("validated selector width"),
        };
        if route.selector_value > maximum {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} selector does not fit its delivery width",
                    route.id
                ),
            ));
        }
        if route.delivery.encoding != "little-endian" {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} delivery encoding must be little-endian",
                    route.id
                ),
            ));
        }
        if let Some(case_handler) = &route.case_handler
            && facts
                .function(
                    &case_handler.profile,
                    &case_handler.source,
                    &case_handler.function,
                )
                .is_none()
        {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} refers to an unknown case handler",
                    route.id
                ),
            ));
        }
        if route.terminal.is_some() && route.case_handler.is_none() {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} terminal requires a case handler",
                    route.id
                ),
            ));
        }
        if let Some(terminal) = &route.terminal
            && facts
                .function(&terminal.profile, &terminal.source, &terminal.function)
                .is_none()
        {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} refers to an unknown terminal function",
                    route.id
                ),
            ));
        }
        if let Some(replay) = &route.replay
            && (replay.manifest.as_os_str().is_empty()
                || replay.source.trim().is_empty()
                || replay.evidence.as_os_str().is_empty()
                || replay.producer_phase.trim().is_empty()
                || replay.consumer_phase.trim().is_empty()
                || replay.state_observation.trim().is_empty()
                || replay.producer_phase == replay.consumer_phase)
        {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} replay requires manifest/evidence paths, a source, two distinct phase names, and a state observation",
                    route.id
                ),
            ));
        }
        if route.rationale.trim().is_empty() || route.rationale.contains(['\r', '\n']) {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} rationale must be one non-empty line",
                    route.id
                ),
            ));
        }
        summary.event_routes += 1;
    }
    Ok(())
}

fn validate_static_event_callback(
    route: &super::ReviewedStaticEventCallbackRoute,
    facts: &FunctionFacts,
) -> ValidationResult<()> {
    validate_static_event_callback_shape(route)?;
    for (index, edge) in route.upstream_chain.windows(2).enumerate() {
        let site = route.upstream_sites.as_ref().map(|sites| sites[index]);
        let owner = require_function(facts, &route.profile, &route.source, &edge[0], &route.id)?;
        let call = exact_call(
            owner,
            &super::ReviewedEventCallMatcher::Function(edge[1].clone()),
            site,
            &route.id,
            "upstream direct",
        )?;
        if call.kind != "internal" || !call.direct {
            return Err(event_route_error(
                &route.id,
                format!(
                    "upstream edge {:?} -> {:?} must be an internal direct call, got {:?}",
                    edge[0], edge[1], call.kind,
                ),
            ));
        }
    }
    let dispatcher = require_function(
        facts,
        &route.profile,
        &route.source,
        &route.dispatcher,
        &route.id,
    )?;
    let dispatches = exact_calls(
        dispatcher,
        &route.dispatch_call,
        route.dispatch_sites.as_deref(),
        &route.id,
        "dispatch",
    )?;
    let delivery = require_function(
        facts,
        &route.delivery_profile,
        &route.delivery_source,
        &route.delivery_entry,
        &route.id,
    )?;
    let receive = exact_call(
        delivery,
        &route.receive_call,
        route.receive_site,
        &route.id,
        "event receive",
    )?;
    require_modeled_result(receive, &route.id, "event receive")?;
    exact_argument(
        receive,
        route.receive_queue_argument,
        &route.id,
        "receive queue",
    )?;
    let run = exact_call(
        delivery,
        &route.run_call,
        route.run_site,
        &route.id,
        "event run",
    )?;
    if receive.site == run.site {
        return Err(event_route_error(
            &route.id,
            "event receive and run sites must be distinct",
        ));
    }
    if run
        .arguments
        .get(usize::from(route.run_event_argument))
        .is_none()
    {
        return Err(event_route_error(&route.id, "event run argument is absent"));
    }
    let callback = require_function(
        facts,
        &route.callback_profile,
        &route.callback_source,
        &route.callback_function,
        &route.id,
    )?;
    let binding = require_function(
        facts,
        &route.binding_profile,
        &route.binding_source,
        &route.binding_entry,
        &route.id,
    )?;
    let binding_call = exact_call(
        binding,
        &route.binding_call,
        route.binding_site,
        &route.id,
        "binding",
    )?;
    let object = exact_argument(
        dispatches[0],
        route.dispatch_object_argument,
        &route.id,
        "dispatch object",
    )?;
    for dispatch in &dispatches {
        if exact_argument(
            dispatch,
            route.dispatch_object_argument,
            &route.id,
            "dispatch object",
        )? != object
        {
            return Err(event_route_error(
                &route.id,
                "dispatch and binding calls do not carry one exact shared event object",
            ));
        }
        exact_argument(
            dispatch,
            route.dispatch_queue_argument,
            &route.id,
            "dispatch queue",
        )?;
    }
    if exact_argument(
        binding_call,
        route.binding_object_argument,
        &route.id,
        "binding object",
    )? != object
    {
        return Err(event_route_error(
            &route.id,
            "dispatch and binding calls do not carry one exact shared event object",
        ));
    }
    let callback_value = callback
        .address
        .map(|address| format!("const:{address:#010x}"))
        .ok_or_else(|| event_route_error(&route.id, "callback function has no linked address"))?;
    if exact_argument(
        binding_call,
        route.binding_callback_argument,
        &route.id,
        "binding callback",
    )? != callback_value
    {
        return Err(event_route_error(
            &route.id,
            "binding callback argument does not resolve to the reviewed callback function",
        ));
    }
    Ok(())
}

fn validate_static_event_callback_shape(
    route: &super::ReviewedStaticEventCallbackRoute,
) -> ValidationResult<()> {
    validate_route_label(&route.id, "mechanism", &route.mechanism)?;
    validate_route_label(&route.id, "execution context", &route.execution_context)?;
    if route.binding_object_argument == route.binding_callback_argument {
        return Err(event_route_error(
            &route.id,
            "binding object and callback arguments must be distinct",
        ));
    }
    if route.dispatch_object_argument == route.dispatch_queue_argument {
        return Err(event_route_error(
            &route.id,
            "dispatch queue and event-object arguments must be distinct",
        ));
    }
    if route.receive_site.is_some() && route.receive_site == route.run_site {
        return Err(event_route_error(
            &route.id,
            "event receive and run sites must be distinct",
        ));
    }
    if route.upstream_chain.len() < 2
        || route.upstream_chain.last() != Some(&route.dispatcher)
        || route
            .upstream_sites
            .as_ref()
            .is_some_and(|sites| sites.len() != route.upstream_chain.len() - 1)
        || route.dispatch_sites.as_ref().is_some_and(Vec::is_empty)
    {
        return Err(event_route_error(
            &route.id,
            "static event route requires an upstream chain ending at its dispatcher; optional upstream sites must cover every edge and optional dispatch sites must be non-empty",
        ));
    }
    validate_rationale(&route.id, &route.rationale)
}

fn validate_broker_subscription(
    route: &super::ReviewedBrokerSubscriptionRoute,
    facts: &FunctionFacts,
) -> ValidationResult<()> {
    validate_broker_subscription_shape(route)?;
    let dispatcher = require_function(
        facts,
        &route.profile,
        &route.source,
        &route.dispatcher,
        &route.id,
    )?;
    let dispatch = exact_call(
        dispatcher,
        &route.dispatch_call,
        route.dispatch_site,
        &route.id,
        "broker publish",
    )?;
    let domain_owner = require_function(
        facts,
        &route.domain.profile,
        &route.domain.source,
        &route.domain.entry,
        &route.id,
    )?;
    let domain = exact_call(
        domain_owner,
        &route.domain.call,
        route.domain.call_site,
        &route.id,
        "broker domain",
    )?;
    let binding = require_function(
        facts,
        &route.binding_profile,
        &route.binding_source,
        &route.binding_entry,
        &route.id,
    )?;
    let binding_call = exact_call(
        binding,
        &route.binding_call,
        route.binding_site,
        &route.id,
        "broker subscription",
    )?;
    let callback = require_function(
        facts,
        &route.callback_profile,
        &route.callback_source,
        &route.callback_function,
        &route.id,
    )?;
    require_function(
        facts,
        &route.case_handler.profile,
        &route.case_handler.source,
        &route.case_handler.function,
        &route.id,
    )?;
    let case = exact_call(
        callback,
        &super::ReviewedEventCallMatcher::Function(route.case_handler.function.clone()),
        route.case_handler_site,
        &route.id,
        "broker selector case",
    )?;
    if case.kind != "internal" {
        return Err(event_route_error(
            &route.id,
            format!(
                "broker callback selector case must be an internal direct call, got {:?}",
                case.kind
            ),
        ));
    }
    if let Some(terminal) = &route.terminal {
        require_function(
            facts,
            &terminal.profile,
            &terminal.source,
            &terminal.function,
            &route.id,
        )?;
    }
    let selector = format!("const:{:#010x}", route.selector_value);
    if exact_argument(
        dispatch,
        route.dispatch_selector_argument,
        &route.id,
        "broker selector",
    )? != selector
    {
        return Err(event_route_error(
            &route.id,
            "broker publish does not carry the reviewed selector",
        ));
    }
    let payload = exact_argument(
        dispatch,
        route.dispatch_payload_argument,
        &route.id,
        "broker payload",
    )?;
    if route
        .payload_value
        .as_deref()
        .is_some_and(|expected| payload != expected)
    {
        return Err(event_route_error(
            &route.id,
            "broker publish does not carry the exact reviewed payload",
        ));
    }
    let domain_selector = format!("const:{:#010x}", route.domain.selector_value);
    if exact_argument(
        domain,
        route.domain.call_selector_argument,
        &route.id,
        "broker domain selector",
    )? != domain_selector
        || exact_argument(
            dispatch,
            route.domain.dispatch_argument,
            &route.id,
            "broker publish domain",
        )? != exact_argument(
            domain,
            route.domain.call_object_argument,
            &route.id,
            "broker attached object",
        )?
    {
        return Err(event_route_error(
            &route.id,
            "broker publish object is not the exact attached reviewed source domain",
        ));
    }
    let binding_object = exact_argument(
        binding_call,
        route.binding_object_argument,
        &route.id,
        "broker binding object",
    )?;
    if !call_argument_establishes_constant(
        binding_call,
        route.binding_domain_argument,
        route.domain.selector_value,
        &route.id,
        "broker binding domain",
    )? {
        return Err(event_route_error(
            &route.id,
            "broker subscription is not proven to use the attached source domain",
        ));
    }
    let callback_value = callback
        .address
        .map(|address| format!("const:{address:#010x}"))
        .ok_or_else(|| event_route_error(&route.id, "callback function has no linked address"))?;
    let stores = binding
        .memory_writes
        .iter()
        .filter(|write| {
            route
                .binding_callback_store_site
                .is_none_or(|site| write.site == site)
                && write.offset == route.binding_callback_store_offset
                && write.width == 32
                && write.value.as_deref() == Some(&callback_value)
                && memory_object_value_expression(&write.object)
                    .is_some_and(|value| normalize_word_value(binding_object) == value)
        })
        .collect::<BTreeSet<_>>()
        .len();
    if stores != 1 {
        return Err(event_route_error(
            &route.id,
            format!(
                "expected one callback-pointer store into the subscribed object, found {stores}"
            ),
        ));
    }
    if !case.guard_paths.as_ref().is_some_and(|paths| {
        !paths.is_empty()
            && paths.iter().all(|path| {
                guard_establishes_selector(
                    path,
                    route.callback_selector_argument,
                    route.selector_value,
                )
            })
    }) {
        return Err(event_route_error(
            &route.id,
            "broker callback case is not guarded by the reviewed callback selector",
        ));
    }
    if let Some(terminal) = &route.terminal {
        match function_path_exists(
            facts,
            &route.case_handler.profile,
            &route.case_handler.source,
            &route.case_handler.function,
            &terminal.function,
            RouteGraphLimits {
                depth: 12,
                nodes: 128,
                edges: 1_024,
            },
        ) {
            Some(true) => {}
            Some(false) => {
                return Err(event_route_error(
                    &route.id,
                    "broker case does not reach the reviewed terminal in the generated call graph",
                ));
            }
            None => {
                return Err(event_route_error(
                    &route.id,
                    "broker terminal validation exceeded the 12-depth/128-node/1024-edge reviewed-route graph limit",
                ));
            }
        }
    }
    Ok(())
}

fn validate_broker_subscription_shape(
    route: &super::ReviewedBrokerSubscriptionRoute,
) -> ValidationResult<()> {
    validate_route_label(&route.id, "mechanism", &route.mechanism)?;
    validate_route_label(&route.id, "execution context", &route.execution_context)?;
    validate_route_label(&route.id, "selector role", &route.selector_role)?;
    validate_route_label(&route.id, "payload role", &route.payload_role)?;
    if let Some(value) = &route.payload_value {
        validate_route_label(&route.id, "payload value", value)?;
    }
    if route.dispatch_selector_argument == route.dispatch_payload_argument
        || route.binding_domain_argument == route.binding_object_argument
    {
        return Err(event_route_error(
            &route.id,
            "selector/payload and binding domain/object arguments must be distinct",
        ));
    }
    if let Some(terminal) = &route.terminal
        && (terminal.profile != route.case_handler.profile
            || terminal.source != route.case_handler.source)
    {
        return Err(event_route_error(
            &route.id,
            "broker terminal must share the case-handler profile and source",
        ));
    }
    validate_rationale(&route.id, &route.rationale)
}

fn exact_calls<'a>(
    function: &'a super::FunctionFact,
    matcher: &super::ReviewedEventCallMatcher,
    sites: Option<&[u32]>,
    route: &str,
    role: &str,
) -> ValidationResult<Vec<&'a super::FunctionCallFact>> {
    super::select_route_calls(&function.calls, matcher, sites, role)
        .map_err(|error| event_route_error(route, error))
}

fn exact_call<'a>(
    function: &'a super::FunctionFact,
    matcher: &super::ReviewedEventCallMatcher,
    site: Option<u32>,
    route: &str,
    role: &str,
) -> ValidationResult<&'a super::FunctionCallFact> {
    super::select_route_call(&function.calls, matcher, site, role)
        .map_err(|error| event_route_error(route, error))
}

fn exact_argument<'a>(
    call: &'a super::FunctionCallFact,
    argument: u8,
    route: &str,
    role: &str,
) -> ValidationResult<&'a str> {
    let value = call
        .arguments
        .get(usize::from(argument))
        .map(String::as_str)
        .ok_or_else(|| event_route_error(route, format!("{role} argument is absent")))?;
    if call.argument_exact.get(usize::from(argument)).copied() != Some(true) {
        return Err(event_route_error(
            route,
            format!("{role} argument is not exact: {value}"),
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

fn call_argument_establishes_constant(
    call: &super::FunctionCallFact,
    argument: u8,
    constant: u32,
    route: &str,
    role: &str,
) -> ValidationResult<bool> {
    let value = exact_argument(call, argument, route, role)?;
    if value == format!("const:{constant:#010x}") {
        return Ok(true);
    }
    Ok(call.guard_paths.as_ref().is_some_and(|paths| {
        !paths.is_empty()
            && paths
                .iter()
                .all(|path| guard_establishes_value(path, value, constant))
    }))
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

fn memory_object_value_expression(object: &super::FunctionMemoryObjectFact) -> Option<String> {
    let super::FunctionMemoryObjectFact::Dereferenced {
        pointer,
        pointer_offset,
    } = object
    else {
        return None;
    };
    Some(format!(
        "memory:{}{}",
        memory_address_expression(pointer)?,
        signed_offset(*pointer_offset)
    ))
}

fn memory_address_expression(object: &super::FunctionMemoryObjectFact) -> Option<String> {
    match object {
        super::FunctionMemoryObjectFact::Absolute { address, .. } => {
            Some(format!("absolute:{address:#010x}"))
        }
        super::FunctionMemoryObjectFact::Dereferenced {
            pointer,
            pointer_offset,
        } => Some(format!(
            "*({}{})",
            memory_address_expression(pointer)?,
            signed_offset(*pointer_offset)
        )),
        _ => None,
    }
}

fn signed_offset(offset: i64) -> String {
    if offset < 0 {
        format!("-{:#x}", offset.unsigned_abs())
    } else {
        format!("+{:#x}", offset as u64)
    }
}

fn function_path_exists(
    facts: &FunctionFacts,
    profile: &str,
    source: &str,
    start: &str,
    target: &str,
    limits: RouteGraphLimits,
) -> Option<bool> {
    // Visit shortest paths first: a deeper visit must not hide a later path
    // that reaches the same node with enough depth budget left.
    let mut frontier = std::collections::VecDeque::from([(start, 0usize)]);
    let mut visited = BTreeSet::from([start]);
    let mut examined_edges = 0usize;
    let mut depth_limited = false;
    while let Some((identity, depth)) = frontier.pop_front() {
        if identity == target {
            return Some(true);
        }
        let Some(function) = facts.function(profile, source, identity) else {
            continue;
        };
        for call in &function.calls {
            if examined_edges == limits.edges {
                return None;
            }
            examined_edges += 1;
            if call.kind != "internal"
                || !call.direct
                || call.site.is_none()
                || facts.function(profile, source, &call.target).is_none()
                || visited.contains(call.target.as_str())
            {
                continue;
            }
            if depth == limits.depth {
                depth_limited = true;
                continue;
            }
            if call.target == target {
                return Some(true);
            }
            if visited.len() >= limits.nodes {
                return None;
            }
            visited.insert(call.target.as_str());
            frontier.push_back((call.target.as_str(), depth + 1));
        }
    }
    (!depth_limited).then_some(false)
}

#[derive(Clone, Copy)]
struct RouteGraphLimits {
    depth: usize,
    nodes: usize,
    edges: usize,
}

fn require_function<'a>(
    facts: &'a FunctionFacts,
    profile: &str,
    source: &str,
    identity: &str,
    route: &str,
) -> ValidationResult<&'a super::FunctionFact> {
    facts.function(profile, source, identity).ok_or_else(|| {
        event_route_error(
            route,
            format!("refers to unknown function {profile}:{identity}"),
        )
    })
}

fn validate_rationale(route: &str, rationale: &str) -> ValidationResult<()> {
    if rationale.trim().is_empty() || rationale.contains(['\r', '\n']) {
        return Err(event_route_error(
            route,
            "rationale must be one non-empty line",
        ));
    }
    Ok(())
}

fn validate_route_label(route: &str, role: &str, value: &str) -> ValidationResult<()> {
    if value.trim().is_empty() || value.contains(['\r', '\n']) {
        return Err(event_route_error(
            route,
            format!("{role} must be one non-empty line"),
        ));
    }
    Ok(())
}

fn event_route_error(route: &str, message: impl Into<String>) -> ValidationError {
    ValidationError::pack(
        "event-routes",
        format!("event route {route:?} {}", message.into()),
    )
}

fn require_modeled_result(
    call: &super::FunctionCallFact,
    route: &str,
    role: &str,
) -> ValidationResult<()> {
    if call.result_modeled {
        Ok(())
    } else {
        Err(event_route_error(
            route,
            format!("{role} does not expose a modeled return value"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(arguments: Vec<&str>, exact: Vec<bool>) -> super::super::FunctionCallFact {
        super::super::FunctionCallFact {
            kind: "direct".to_owned(),
            target: "fixture::target".to_owned(),
            direct: true,
            result_modeled: true,
            result_provenance: None,
            semantic_operation: None,
            site: Some(0x1000),
            arguments: arguments.into_iter().map(str::to_owned).collect(),
            argument_exact: exact,
            argument_result_provenance: Vec::new(),
            guard_paths: None,
        }
    }

    fn graph(edges: &[(&str, &[&str])]) -> FunctionFacts {
        FunctionFacts {
            inputs: Vec::new(),
            functions: edges
                .iter()
                .map(|(identity, targets)| super::super::FunctionFact {
                    profile: "fixture".to_owned(),
                    source: "vendor".to_owned(),
                    identity: (*identity).to_owned(),
                    member: None,
                    symbol: (*identity).to_owned(),
                    address: None,
                    selection: "fixture".to_owned(),
                    body_complete: true,
                    call_targets_complete: true,
                    transitive_effects_complete: false,
                    executable_complete: false,
                    transitive_effects_materialized: false,
                    call_graph_closed: true,
                    context_projection_materialized: false,
                    context_projection_complete: false,
                    context_projection_blockers: Vec::new(),
                    decode_blockers: Vec::new(),
                    direct_calls: targets.len(),
                    calls: targets
                        .iter()
                        .map(|target| {
                            let mut edge = call(Vec::new(), Vec::new());
                            edge.target = (*target).to_owned();
                            edge.kind = "internal".to_owned();
                            edge
                        })
                        .collect(),
                    memory_writes: Vec::new(),
                    mmio_addresses: Vec::new(),
                    context_fields: Vec::new(),
                    memory_fields: Vec::new(),
                    semantic_operations: Vec::new(),
                    trampoline_calls: 0,
                    event_dispatches: Vec::new(),
                    scenario_suggestions: Vec::new(),
                    pseudo: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn route_reachability_preserves_shorter_paths_and_reports_budget_exhaustion() {
        let facts = graph(&[
            ("root", &["shared", "long"]),
            ("long", &["detour"]),
            ("detour", &["shared"]),
            ("shared", &["step"]),
            ("step", &["terminal"]),
            ("terminal", &[]),
        ]);
        let search =
            |limits| function_path_exists(&facts, "fixture", "vendor", "root", "terminal", limits);
        assert_eq!(
            search(RouteGraphLimits {
                depth: 3,
                nodes: 128,
                edges: 1024
            }),
            Some(true)
        );
        assert_eq!(
            search(RouteGraphLimits {
                depth: 2,
                nodes: 128,
                edges: 1024
            }),
            None
        );
        assert_eq!(
            search(RouteGraphLimits {
                depth: 12,
                nodes: 2,
                edges: 1024
            }),
            None
        );
        assert_eq!(
            search(RouteGraphLimits {
                depth: 12,
                nodes: 128,
                edges: 1
            }),
            None
        );
    }

    #[test]
    fn route_reachability_requires_observed_internal_direct_edges() {
        let facts = graph(&[("root", &["terminal"]), ("terminal", &[])]);
        // An observed edge already proves reachability; unrelated siblings
        // must not consume the remaining budget before we report the witness.
        let siblings = graph(&[
            ("root", &["terminal", "other"]),
            ("terminal", &[]),
            ("other", &[]),
        ]);
        assert_eq!(
            function_path_exists(
                &siblings,
                "fixture",
                "vendor",
                "root",
                "terminal",
                RouteGraphLimits {
                    depth: 1,
                    nodes: 1,
                    edges: 1
                }
            ),
            Some(true)
        );
        for change in 0..3 {
            let mut facts = facts.clone();
            let edge = &mut facts.functions[0].calls[0];
            match change {
                0 => edge.direct = false,
                1 => edge.site = None,
                _ => edge.kind = "external".to_owned(),
            }
            assert_eq!(
                function_path_exists(
                    &facts,
                    "fixture",
                    "vendor",
                    "root",
                    "terminal",
                    RouteGraphLimits {
                        depth: 12,
                        nodes: 128,
                        edges: 1024
                    }
                ),
                Some(false)
            );
        }
    }

    #[test]
    fn every_explicit_or_inferred_dispatch_requires_an_exact_shared_object() {
        let document = super::super::tests::CALLBACK_ROUTES.parse().unwrap();
        let pack = super::super::pack_parse::parse(&document).unwrap();
        let super::super::ReviewedEventRoute::StaticEventCallback(route) = &pack.event_routes[0]
        else {
            panic!("static route");
        };
        let mut facts = graph(&[
            ("vendor::isr", &["vendor::enqueue"]),
            ("vendor::enqueue", &[]),
            ("vendor::init", &[]),
            ("vendor::event_loop", &[]),
            ("vendor::callback", &[]),
        ]);
        for function in &mut facts.functions {
            function.profile = "controller".to_owned();
        }
        facts.functions[0].calls[0].site = Some(0x1010);
        let observed = |operation: &str, site, arguments: Vec<&str>| {
            let mut call = call(arguments.clone(), vec![true; arguments.len()]);
            call.semantic_operation = Some(operation.to_owned());
            call.site = Some(site);
            call
        };
        facts.functions[1].calls = vec![
            observed(
                "event.send",
                0x1010,
                vec!["const:0x00002000", "const:0x00003000"],
            ),
            observed(
                "event.send",
                0x1020,
                vec!["const:0x00002000", "const:0x00003000"],
            ),
        ];
        facts.functions[2].calls = vec![observed(
            "event.init",
            0x1030,
            vec!["const:0x00003000", "const:0x00004000"],
        )];
        facts.functions[3].calls = vec![
            observed("event.receive", 0x1040, vec!["const:0x00002000"]),
            observed("event.run", 0x1050, vec!["result:event.receive"]),
        ];
        facts.functions[4].address = Some(0x4000);
        for infer in [false, true] {
            let mut route = route.clone();
            if infer {
                route.dispatch_sites = None;
                route.upstream_sites = None;
                route.binding_site = None;
                route.receive_site = None;
                route.run_site = None;
            }
            validate_static_event_callback(&route, &facts).unwrap();
            let mut ambiguous = facts.clone();
            ambiguous.functions[1].calls[1].argument_exact[1] = false;
            assert!(
                validate_static_event_callback(&route, &ambiguous)
                    .unwrap_err()
                    .to_string()
                    .contains("dispatch object argument is not exact")
            );
        }
    }

    #[test]
    fn mandatory_route_arguments_fail_closed() {
        assert!(exact_argument(&call(Vec::new(), Vec::new()), 0, "fixture", "payload").is_err());
        assert!(
            exact_argument(&call(vec!["unknown"], vec![false]), 0, "fixture", "payload").is_err()
        );
        assert!(
            exact_argument(
                &call(vec!["varies-across-2-shapes"], vec![false]),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert!(
            exact_argument(
                &call(
                    vec!["symbol:<linked>::callback:hi+0x0:lo?:post+0x0"],
                    vec![false]
                ),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert!(
            exact_argument(
                &call(vec!["one-of(0x1,0x2)"], vec![false]),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert!(
            exact_argument(
                &call(vec!["expr:unknown+4"], vec![false]),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert!(
            exact_argument(
                &call(vec!["prefix-varies-across-2-shapes"], vec![false]),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert!(
            exact_argument(
                &call(vec!["bits:0=1,1=?"], vec![false]),
                0,
                "fixture",
                "payload"
            )
            .is_err()
        );
        assert_eq!(
            exact_argument(
                &call(vec!["const:0x00000004"], vec![true]),
                0,
                "fixture",
                "payload"
            )
            .unwrap(),
            "const:0x00000004"
        );
    }

    #[test]
    fn stateful_receive_requires_a_modeled_return_value() {
        let mut receive = call(vec!["const:0x00002000"], vec![true]);
        receive.result_modeled = false;
        let error = require_modeled_result(&receive, "fixture", "event receive")
            .expect_err("opaque receive result must fail closed");
        assert!(error.to_string().contains("modeled return value"));
    }

    #[test]
    fn reviewed_route_calls_require_direct_instruction_provenance() {
        let mut indirect = call(vec!["const:0x00000004"], vec![true]);
        indirect.direct = false;
        assert!(
            super::super::select_route_call(
                std::slice::from_ref(&indirect),
                &super::super::ReviewedEventCallMatcher::Function("fixture::target".to_owned()),
                Some(0x1000),
                "dispatch"
            )
            .is_err()
        );
        indirect.direct = true;
        assert_eq!(
            super::super::select_route_call(
                std::slice::from_ref(&indirect),
                &super::super::ReviewedEventCallMatcher::Function("fixture::target".to_owned()),
                Some(0x1000),
                "dispatch"
            )
            .unwrap()
            .target,
            "fixture::target"
        );
    }

    #[test]
    fn selector_guard_requires_an_exact_conjunct() {
        assert!(guard_establishes_selector(
            "ready && (arg0 == 0x80000004)",
            0,
            0x8000_0004
        ));
        assert!(!guard_establishes_selector(
            "arg0 != 0x00000000",
            0,
            0x8000_0004
        ));
    }

    #[test]
    fn selector_guard_must_hold_on_every_call_path() {
        let paths = ["(arg0 == 0x80000004)", "ready"];
        assert!(
            !paths
                .iter()
                .all(|path| guard_establishes_selector(path, 0, 0x8000_0004))
        );
    }

    #[test]
    fn broker_domain_result_must_be_zero_on_every_binding_path() {
        let value = "result_of_fixture_subscribe_0x00001000";
        let mut guarded = call(vec![value], vec![true]);
        guarded.guard_paths = Some(vec![format!("!({value} != 0x00000000)")]);
        assert!(call_argument_establishes_constant(&guarded, 0, 0, "fixture", "domain").unwrap());

        guarded
            .guard_paths
            .as_mut()
            .unwrap()
            .push("ready".to_owned());
        assert!(!call_argument_establishes_constant(&guarded, 0, 0, "fixture", "domain").unwrap());
        assert!(
            !call_argument_establishes_constant(
                &call(vec![value], vec![true]),
                0,
                0,
                "fixture",
                "domain"
            )
            .unwrap()
        );
    }
}
