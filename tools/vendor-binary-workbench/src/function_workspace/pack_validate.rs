//! Structural, provenance, completeness, and review-coverage validation.

use std::collections::BTreeSet;

use super::{
    FunctionContextFieldFact, FunctionFact, FunctionFacts, FunctionInputFact, FunctionPack,
    FunctionReviewStatus, FunctionWorkspaceSummary, ReviewedContextField, ReviewedFunction,
    ReviewedFunctionInput,
};
use crate::Result;

pub(super) fn validate(
    pack: &FunctionPack,
    facts: &FunctionFacts,
) -> Result<FunctionWorkspaceSummary> {
    validate_id(&pack.id, "function pack id")?;
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
            return Err(crate::Error::invalid(format!(
                "duplicate reviewed function {}:{}",
                reviewed.profile, reviewed.identity
            )));
        }
        let fact = facts
            .function(&reviewed.profile, &reviewed.source, &reviewed.identity)
            .ok_or_else(|| {
                format!(
                    "stale reviewed function {}:{}:{}",
                    reviewed.profile, reviewed.source, reviewed.identity
                )
            })
            .map_err(crate::Error::invalid)?;
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
    Ok(summary)
}

fn validate_inputs(reviewed: &[ReviewedFunctionInput], facts: &[FunctionInputFact]) -> Result<()> {
    let mut keys = BTreeSet::new();
    for input in reviewed {
        validate_id(&input.profile, "function input profile")?;
        validate_id(&input.source, "function input source")?;
        validate_sha256(&input.sha256, "function input")?;
        if !keys.insert((&input.profile, &input.source)) {
            return Err(crate::Error::invalid(format!(
                "duplicate function input guard {}:{}",
                input.profile, input.source
            )));
        }
        let Some(fact) = facts
            .iter()
            .find(|fact| fact.profile == input.profile && fact.source == input.source)
        else {
            return Err(crate::Error::invalid(format!(
                "stale function input guard {}:{}",
                input.profile, input.source
            )));
        };
        if fact.sha256 != input.sha256 {
            return Err(crate::Error::invalid(format!(
                "stale function input digest {}:{}; re-review the updated artifact",
                input.profile, input.source
            )));
        }
    }
    if reviewed.len() != facts.len() {
        return Err(crate::Error::invalid(format!(
            "function pack guards {} inputs but generated facts contain {}; reinitialize or add reviewed guards",
            reviewed.len(),
            facts.len()
        )));
    }
    Ok(())
}

fn validate_function(
    reviewed: &ReviewedFunction,
    fact: &FunctionFact,
    summary: &mut FunctionWorkspaceSummary,
    reviewed_names: &mut BTreeSet<String>,
) -> Result<()> {
    match reviewed.status {
        FunctionReviewStatus::Unreviewed => {
            if reviewed.name.is_some()
                || reviewed.role.is_some()
                || reviewed.summary.is_some()
                || reviewed.accept_incomplete
            {
                return Err(crate::Error::invalid(format!(
                    "unreviewed function {}:{} cannot make reviewed claims",
                    reviewed.profile, reviewed.identity
                )));
            }
            summary.unreviewed_functions += usize::from(fact.is_root());
        }
        FunctionReviewStatus::Ignored => {
            if reviewed.name.is_some()
                || reviewed.role.is_some()
                || reviewed.summary.is_some()
                || !reviewed.contexts.is_empty()
            {
                return Err(crate::Error::invalid(format!(
                    "ignored function {}:{} cannot define names or contexts",
                    reviewed.profile, reviewed.identity
                )));
            }
            summary.ignored_functions += usize::from(fact.is_root());
            summary.ignored_fields += fact.context_fields.len();
            summary.ignored_contexts += fact
                .context_fields
                .iter()
                .map(|field| field.argument)
                .collect::<BTreeSet<_>>()
                .len();
            return Ok(());
        }
        FunctionReviewStatus::Reviewed => {
            let name = required_claim(&reviewed.name, "name", reviewed)?;
            validate_identifier(name, "reviewed function name")?;
            if !reviewed_names.insert(name.to_owned()) {
                return Err(crate::Error::invalid(format!(
                    "duplicate reviewed function name {name:?}"
                )));
            }
            validate_id(
                required_claim(&reviewed.role, "role", reviewed)?,
                "function role",
            )?;
            let function_summary = required_claim(&reviewed.summary, "summary", reviewed)?;
            if function_summary.trim().is_empty() || function_summary.contains(['\r', '\n']) {
                return Err(crate::Error::invalid(format!(
                    "reviewed function {}:{} summary must be one line",
                    reviewed.profile, reviewed.identity
                )));
            }
            if !fact.review_complete() && !reviewed.accept_incomplete {
                return Err(crate::Error::invalid(format!(
                    "reviewed function {}:{} has incomplete generated evidence; set accept-incomplete = true after reviewing blockers",
                    reviewed.profile, reviewed.identity
                )));
            }
            summary.reviewed_functions += usize::from(fact.is_root());
            summary.accepted_incomplete += usize::from(!fact.review_complete());
        }
    }
    validate_contexts(reviewed, fact, summary)
}

fn validate_contexts(
    reviewed: &ReviewedFunction,
    fact: &FunctionFact,
    summary: &mut FunctionWorkspaceSummary,
) -> Result<()> {
    let mut contexts = BTreeSet::new();
    let mut context_names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let observed_arguments = fact
        .context_fields
        .iter()
        .map(|field| field.argument)
        .collect::<BTreeSet<_>>();
    for context in &reviewed.contexts {
        if !contexts.insert(context.argument) || context.argument >= 8 {
            return Err(crate::Error::invalid(format!(
                "function {}:{} has a duplicate or invalid context argument",
                reviewed.profile, reviewed.identity
            )));
        }
        if !observed_arguments.contains(&context.argument) {
            return Err(crate::Error::invalid(format!(
                "stale context argument {} in {}:{}",
                context.argument, reviewed.profile, reviewed.identity
            )));
        }
        match context.status {
            FunctionReviewStatus::Reviewed => {
                let name = context
                    .name
                    .as_deref()
                    .ok_or("reviewed context requires name")
                    .map_err(crate::Error::invalid)?;
                validate_identifier(name, "reviewed context name")?;
                if !context_names.insert(name) {
                    return Err(crate::Error::invalid(format!(
                        "function {}:{} has duplicate reviewed context name {name:?}",
                        reviewed.profile, reviewed.identity
                    )));
                }
                validate_identifier(
                    context
                        .type_name
                        .as_deref()
                        .ok_or("reviewed context requires type-name")
                        .map_err(crate::Error::invalid)?,
                    "reviewed context type-name",
                )?;
                summary.reviewed_contexts += 1;
            }
            FunctionReviewStatus::Unreviewed => {
                if context.name.is_some() || context.type_name.is_some() {
                    return Err(crate::Error::invalid(
                        "unreviewed context cannot define a name or type-name",
                    ));
                }
                summary.unreviewed_contexts += 1;
            }
            FunctionReviewStatus::Ignored => {
                if context.name.is_some()
                    || context.type_name.is_some()
                    || !context.fields.is_empty()
                {
                    return Err(crate::Error::invalid(
                        "ignored context cannot define names or fields",
                    ));
                }
                summary.ignored_contexts += 1;
                for observed in fact
                    .context_fields
                    .iter()
                    .filter(|field| field.argument == context.argument)
                {
                    fields.insert((observed.argument, observed.offset, observed.width));
                    summary.ignored_fields += 1;
                }
            }
        }
        let mut field_names = BTreeSet::new();
        for field in &context.fields {
            let key = (context.argument, field.offset, field.width);
            if !fields.insert(key) {
                return Err(crate::Error::invalid(format!(
                    "function {}:{} has a duplicate reviewed context field",
                    reviewed.profile, reviewed.identity
                )));
            }
            fact.context_fields
                .iter()
                .find(|observed| field_matches(context.argument, field, observed))
                .ok_or_else(|| {
                    format!(
                        "stale context field arg{} {:+#x}/{} in {}:{}",
                        context.argument,
                        field.offset,
                        field.width,
                        reviewed.profile,
                        reviewed.identity
                    )
                })
                .map_err(crate::Error::invalid)?;
            if field.status == FunctionReviewStatus::Reviewed {
                let name = field
                    .name
                    .as_deref()
                    .ok_or("reviewed field requires name")
                    .map_err(crate::Error::invalid)?;
                if !field_names.insert(name) {
                    return Err(crate::Error::invalid(format!(
                        "function {}:{} context arg{} has duplicate reviewed field name {name:?}",
                        reviewed.profile, reviewed.identity, context.argument
                    )));
                }
            }
            validate_field(field, summary)?;
        }
    }
    summary.unreviewed_contexts += observed_arguments.difference(&contexts).count();
    for observed in &fact.context_fields {
        if !fields.contains(&(observed.argument, observed.offset, observed.width)) {
            summary.unreviewed_fields += 1;
        }
    }
    Ok(())
}

fn validate_field(
    field: &ReviewedContextField,
    summary: &mut FunctionWorkspaceSummary,
) -> Result<()> {
    match field.status {
        FunctionReviewStatus::Reviewed => {
            validate_identifier(
                field
                    .name
                    .as_deref()
                    .ok_or("reviewed field requires name")
                    .map_err(crate::Error::invalid)?,
                "reviewed context field name",
            )?;
            let display_type = field
                .display_type
                .as_deref()
                .ok_or("reviewed field requires display-type")
                .map_err(crate::Error::invalid)?;
            if display_type.trim().is_empty() || display_type.contains(['\r', '\n']) {
                return Err(crate::Error::invalid(
                    "reviewed field display-type must be one non-empty line",
                ));
            }
            if field.description.as_deref().is_some_and(|description| {
                description.trim().is_empty() || description.contains(['\r', '\n'])
            }) {
                return Err(crate::Error::invalid(
                    "reviewed field description must be one non-empty line",
                ));
            }
            summary.reviewed_fields += 1;
        }
        FunctionReviewStatus::Unreviewed => {
            if field.name.is_some() || field.display_type.is_some() || field.description.is_some() {
                return Err(crate::Error::invalid(
                    "unreviewed context field cannot define reviewed claims",
                ));
            }
            summary.unreviewed_fields += 1;
        }
        FunctionReviewStatus::Ignored => {
            if field.name.is_some() || field.display_type.is_some() || field.description.is_some() {
                return Err(crate::Error::invalid(
                    "ignored context field cannot define reviewed claims",
                ));
            }
            summary.ignored_fields += 1;
        }
    }
    Ok(())
}

fn field_matches(
    argument: u8,
    reviewed: &ReviewedContextField,
    observed: &FunctionContextFieldFact,
) -> bool {
    observed.argument == argument
        && observed.offset == reviewed.offset
        && observed.width == reviewed.width
}

fn required_claim<'a>(
    value: &'a Option<String>,
    name: &str,
    function: &ReviewedFunction,
) -> Result<&'a str> {
    value
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "reviewed function {}:{} requires {name}",
                function.profile, function.identity
            ))
        })
}

fn validate_id(value: &str, context: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(crate::Error::invalid(format!(
            "invalid {context} {value:?}"
        )));
    }
    Ok(())
}

fn validate_identifier(value: &str, context: &str) -> Result<()> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(crate::Error::invalid(format!(
            "invalid {context} {value:?}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, context: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(crate::Error::invalid(format!(
            "{context} has invalid lowercase SHA-256 {value:?}"
        )));
    }
    Ok(())
}
