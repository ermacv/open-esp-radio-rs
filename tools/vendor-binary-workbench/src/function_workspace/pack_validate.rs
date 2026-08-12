//! Structural, provenance, completeness, and review-coverage validation.

use std::collections::BTreeSet;

use super::validation::{ValidationError, ValidationResult};
use super::{FunctionFacts, FunctionPack, FunctionWorkspaceSummary};

mod contexts;
mod primitives;
mod types;

use contexts::{validate_function, validate_inputs};
use primitives::validate_id;
use types::validate_types;

pub(super) fn validate(
    pack: &FunctionPack,
    facts: &FunctionFacts,
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
    validate_event_routes(pack, facts, &mut summary)?;
    Ok(summary)
}

fn validate_event_routes(
    pack: &FunctionPack,
    facts: &FunctionFacts,
    summary: &mut FunctionWorkspaceSummary,
) -> ValidationResult<()> {
    let mut ids = BTreeSet::new();
    for route in &pack.event_routes {
        validate_id(&route.id, "event route id")
            .map_err(|message| ValidationError::pack("event-routes", message))?;
        if !ids.insert(route.id.as_str()) {
            return Err(ValidationError::pack(
                "event-routes",
                format!("duplicate event route id {:?}", route.id),
            ));
        }
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
        if let Some(replay) = &route.replay
            && (replay.evidence.as_os_str().is_empty()
                || replay.producer_phase.trim().is_empty()
                || replay.consumer_phase.trim().is_empty()
                || replay.producer_phase == replay.consumer_phase)
        {
            return Err(ValidationError::pack(
                "event-routes",
                format!(
                    "event route {:?} replay requires a path and two distinct non-empty phase names",
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
