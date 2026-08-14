//! Validation of input guards, reviewed functions, contexts, and context fields.

use std::collections::BTreeSet;

use super::super::validation::{ValidationError, ValidationResult};
use super::super::{
    FunctionContextFieldFact, FunctionFact, FunctionInputFact, FunctionReviewStatus,
    FunctionWorkspaceSummary, ReviewedContext, ReviewedContextField, ReviewedFunction,
    ReviewedFunctionInput,
};
use super::primitives::{validate_id, validate_identifier, validate_sha256};

pub(super) fn validate_inputs(
    reviewed: &[ReviewedFunctionInput],
    facts: &[FunctionInputFact],
) -> ValidationResult<()> {
    let mut keys = BTreeSet::new();
    for input in reviewed {
        validate_id(&input.profile, "function input profile")
            .map_err(|message| ValidationError::input(input, "profile", message))?;
        validate_id(&input.source, "function input source")
            .map_err(|message| ValidationError::input(input, "source", message))?;
        validate_sha256(&input.sha256, "function input")
            .map_err(|message| ValidationError::input(input, "artifact-sha256", message))?;
        if !keys.insert((&input.profile, &input.source)) {
            return Err(ValidationError::input(
                input,
                "profile",
                format!(
                    "duplicate function input guard {}:{}",
                    input.profile, input.source
                ),
            ));
        }
        let Some(fact) = facts
            .iter()
            .find(|fact| fact.profile == input.profile && fact.source == input.source)
        else {
            return Err(ValidationError::input(
                input,
                "profile",
                format!(
                    "stale function input guard {}:{}",
                    input.profile, input.source
                ),
            ));
        };
        if fact.sha256 != input.sha256 {
            return Err(ValidationError::input(
                input,
                "artifact-sha256",
                format!(
                    "stale function input digest {}:{}; re-review the updated artifact",
                    input.profile, input.source
                ),
            ));
        }
    }
    if reviewed.len() != facts.len() {
        return Err(ValidationError::pack(
            "inputs",
            format!(
                "function pack guards {} inputs but generated facts contain {}; reinitialize or add reviewed guards",
                reviewed.len(),
                facts.len()
            ),
        ));
    }
    Ok(())
}
pub(super) fn validate_function(
    reviewed: &ReviewedFunction,
    fact: &FunctionFact,
    summary: &mut FunctionWorkspaceSummary,
    reviewed_names: &mut BTreeSet<String>,
) -> ValidationResult<()> {
    match reviewed.status {
        FunctionReviewStatus::Ignored => {
            if reviewed.name.is_some()
                || reviewed.role.is_some()
                || reviewed.summary.is_some()
                || reviewed.signature.is_some()
                || !reviewed.preconditions.is_empty()
                || !reviewed.paths.is_empty()
                || !reviewed.contexts.is_empty()
            {
                return Err(ValidationError::function(
                    reviewed,
                    "status",
                    format!(
                        "ignored function {}:{} cannot define names or contexts",
                        reviewed.profile, reviewed.identity
                    ),
                ));
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
            validate_identifier(name, "reviewed function name")
                .map_err(|message| ValidationError::function(reviewed, "name", message))?;
            if !reviewed_names.insert(name.to_owned()) {
                return Err(ValidationError::function(
                    reviewed,
                    "name",
                    format!("duplicate reviewed function name {name:?}"),
                ));
            }
            validate_id(
                required_claim(&reviewed.role, "role", reviewed)?,
                "function role",
            )
            .map_err(|message| ValidationError::function(reviewed, "role", message))?;
            let function_summary = required_claim(&reviewed.summary, "summary", reviewed)?;
            if function_summary.trim().is_empty() || function_summary.contains(['\r', '\n']) {
                return Err(ValidationError::function(
                    reviewed,
                    "summary",
                    format!(
                        "reviewed function {}:{} summary must be one line",
                        reviewed.profile, reviewed.identity
                    ),
                ));
            }
            if !fact.review_complete() && !reviewed.accept_incomplete {
                return Err(ValidationError::function(
                    reviewed,
                    "accept-incomplete",
                    format!(
                        "reviewed function {}:{} has incomplete generated evidence; set accept-incomplete = true after reviewing blockers",
                        reviewed.profile, reviewed.identity
                    ),
                ));
            }
            summary.reviewed_functions += usize::from(fact.is_root());
            summary.accepted_incomplete += usize::from(!fact.review_complete());
            validate_reviewed_paths(reviewed)?;
            validate_signature(reviewed)?;
        }
    }
    validate_contexts(reviewed, fact, summary)
}

fn validate_signature(reviewed: &ReviewedFunction) -> ValidationResult<()> {
    let Some(signature) = &reviewed.signature else {
        return Ok(());
    };
    if let Some(return_abi) = &signature.return_abi {
        crate::interfaces::validate_abi_type(return_abi, true, "reviewed function return")
            .map_err(|message| ValidationError::function(reviewed, "signature", message))?;
    }
    if signature.return_abi.as_deref() == Some("void") && signature.return_role.is_some() {
        return Err(ValidationError::function(
            reviewed,
            "signature",
            "void reviewed function cannot define a return role",
        ));
    }
    if let Some(role) = &signature.return_role {
        if signature.return_abi.is_none() {
            return Err(ValidationError::function(
                reviewed,
                "signature",
                "reviewed function return role requires a reviewed return ABI",
            ));
        }
        validate_id(role, "reviewed function return role")
            .map_err(|message| ValidationError::function(reviewed, "signature", message))?;
    }
    let mut names = BTreeSet::new();
    for (expected, argument) in signature.arguments.iter().enumerate() {
        if usize::from(argument.index) != expected
            || argument.index >= super::super::MAX_CONTEXT_ARGUMENTS
        {
            return Err(ValidationError::function(
                reviewed,
                "signature",
                "reviewed function argument indices must be contiguous from zero and fit the modeled ABI",
            ));
        }
        validate_identifier(&argument.name, "reviewed function argument name")
            .map_err(|message| ValidationError::function(reviewed, "signature", message))?;
        if !names.insert(argument.name.as_str()) {
            return Err(ValidationError::function(
                reviewed,
                "signature",
                format!(
                    "duplicate reviewed function argument name {:?}",
                    argument.name
                ),
            ));
        }
        crate::interfaces::validate_abi_type(&argument.abi, false, "reviewed function argument")
            .map_err(|message| ValidationError::function(reviewed, "signature", message))?;
        if let Some(role) = &argument.role {
            validate_id(role, "reviewed function argument role")
                .map_err(|message| ValidationError::function(reviewed, "signature", message))?;
        }
    }
    Ok(())
}

fn validate_reviewed_paths(reviewed: &ReviewedFunction) -> ValidationResult<()> {
    let mut preconditions = BTreeSet::new();
    for precondition in &reviewed.preconditions {
        if !preconditions.insert(precondition.id.as_str()) {
            return Err(ValidationError::function(
                reviewed,
                "preconditions",
                format!("duplicate reviewed precondition id {:?}", precondition.id),
            ));
        }
        validate_id(&precondition.id, "precondition id")
            .map_err(|message| ValidationError::function(reviewed, "preconditions", message))?;
        for (field, value) in [
            ("expression", precondition.expression.as_str()),
            ("rationale", precondition.rationale.as_str()),
        ] {
            if value.trim().is_empty() || value.contains(['\r', '\n']) {
                return Err(ValidationError::function(
                    reviewed,
                    "preconditions",
                    format!(
                        "precondition {} {field} must be one non-empty line",
                        precondition.id
                    ),
                ));
            }
        }
    }
    let mut paths = BTreeSet::new();
    for path in &reviewed.paths {
        if !paths.insert(path.id.as_str()) {
            return Err(ValidationError::function(
                reviewed,
                "paths",
                format!("duplicate reviewed path id {:?}", path.id),
            ));
        }
        validate_id(&path.id, "path id")
            .map_err(|message| ValidationError::function(reviewed, "paths", message))?;
        if !matches!(
            path.class.as_str(),
            "operational" | "diagnostic" | "timeout" | "error" | "recovery"
        ) {
            return Err(ValidationError::function(
                reviewed,
                "paths",
                format!(
                    "reviewed path {} has unsupported class {:?}",
                    path.id, path.class
                ),
            ));
        }
        if [path.summary.as_str(), path.evidence.as_str()]
            .iter()
            .any(|value| value.trim().is_empty() || value.contains(['\r', '\n']))
        {
            return Err(ValidationError::function(
                reviewed,
                "paths",
                format!(
                    "reviewed path {} summary/evidence must be one non-empty line",
                    path.id
                ),
            ));
        }
    }
    Ok(())
}
fn validate_contexts(
    reviewed: &ReviewedFunction,
    fact: &FunctionFact,
    summary: &mut FunctionWorkspaceSummary,
) -> ValidationResult<()> {
    let mut contexts = BTreeSet::new();
    let mut context_names = BTreeSet::new();
    let mut fields = BTreeSet::new();
    let observed_arguments = fact
        .context_fields
        .iter()
        .map(|field| field.argument)
        .collect::<BTreeSet<_>>();
    for context in &reviewed.contexts {
        if !contexts.insert(context.argument)
            || context.argument >= super::super::MAX_CONTEXT_ARGUMENTS
        {
            return Err(ValidationError::context(
                reviewed,
                context,
                "argument",
                format!(
                    "function {}:{} has a duplicate or invalid context argument",
                    reviewed.profile, reviewed.identity
                ),
            ));
        }
        if !observed_arguments.contains(&context.argument) {
            return Err(ValidationError::context(
                reviewed,
                context,
                "argument",
                format!(
                    "stale context argument {} in {}:{}",
                    context.argument, reviewed.profile, reviewed.identity
                ),
            ));
        }
        match context.status {
            FunctionReviewStatus::Reviewed => {
                let name = context.name.as_deref().ok_or_else(|| {
                    ValidationError::context(
                        reviewed,
                        context,
                        "name",
                        "reviewed context requires name",
                    )
                })?;
                validate_identifier(name, "reviewed context name").map_err(|message| {
                    ValidationError::context(reviewed, context, "name", message)
                })?;
                if !context_names.insert(name) {
                    return Err(ValidationError::context(
                        reviewed,
                        context,
                        "name",
                        format!(
                            "function {}:{} has duplicate reviewed context name {name:?}",
                            reviewed.profile, reviewed.identity
                        ),
                    ));
                }
                validate_identifier(
                    context.type_name.as_deref().ok_or_else(|| {
                        ValidationError::context(
                            reviewed,
                            context,
                            "type-name",
                            "reviewed context requires type-name",
                        )
                    })?,
                    "reviewed context type-name",
                )
                .map_err(|message| {
                    ValidationError::context(reviewed, context, "type-name", message)
                })?;
                summary.reviewed_contexts += 1;
            }
            FunctionReviewStatus::Ignored => {
                if context.name.is_some()
                    || context.type_name.is_some()
                    || !context.fields.is_empty()
                {
                    return Err(ValidationError::context(
                        reviewed,
                        context,
                        "status",
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
                return Err(ValidationError::field(
                    reviewed,
                    context,
                    field,
                    "offset",
                    format!(
                        "function {}:{} has a duplicate reviewed context field",
                        reviewed.profile, reviewed.identity
                    ),
                ));
            }
            fact.context_fields
                .iter()
                .find(|observed| field_matches(context.argument, field, observed))
                .ok_or_else(|| {
                    ValidationError::field(
                        reviewed,
                        context,
                        field,
                        "offset",
                        format!(
                            "stale context field arg{} {:+#x}/{} in {}:{}",
                            context.argument,
                            field.offset,
                            field.width,
                            reviewed.profile,
                            reviewed.identity
                        ),
                    )
                })?;
            if field.status == FunctionReviewStatus::Reviewed {
                let name = field.name.as_deref().ok_or_else(|| {
                    ValidationError::field(
                        reviewed,
                        context,
                        field,
                        "name",
                        "reviewed field requires name",
                    )
                })?;
                if !field_names.insert(name) {
                    return Err(ValidationError::field(
                        reviewed,
                        context,
                        field,
                        "name",
                        format!(
                            "function {}:{} context arg{} has duplicate reviewed field name {name:?}",
                            reviewed.profile, reviewed.identity, context.argument
                        ),
                    ));
                }
            }
            validate_field(reviewed, context, field, summary)?;
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
    function: &ReviewedFunction,
    context: &ReviewedContext,
    field: &ReviewedContextField,
    summary: &mut FunctionWorkspaceSummary,
) -> ValidationResult<()> {
    match field.status {
        FunctionReviewStatus::Reviewed => {
            validate_identifier(
                field.name.as_deref().ok_or_else(|| {
                    ValidationError::field(
                        function,
                        context,
                        field,
                        "name",
                        "reviewed field requires name",
                    )
                })?,
                "reviewed context field name",
            )
            .map_err(|message| ValidationError::field(function, context, field, "name", message))?;
            let display_type = field.display_type.as_deref().ok_or_else(|| {
                ValidationError::field(
                    function,
                    context,
                    field,
                    "display-type",
                    "reviewed field requires display-type",
                )
            })?;
            if display_type.trim().is_empty() || display_type.contains(['\r', '\n']) {
                return Err(ValidationError::field(
                    function,
                    context,
                    field,
                    "display-type",
                    "reviewed field display-type must be one non-empty line",
                ));
            }
            if field.description.as_deref().is_some_and(|description| {
                description.trim().is_empty() || description.contains(['\r', '\n'])
            }) {
                return Err(ValidationError::field(
                    function,
                    context,
                    field,
                    "description",
                    "reviewed field description must be one non-empty line",
                ));
            }
            summary.reviewed_fields += 1;
        }
        FunctionReviewStatus::Ignored => {
            if field.name.is_some() || field.display_type.is_some() || field.description.is_some() {
                return Err(ValidationError::field(
                    function,
                    context,
                    field,
                    "status",
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
    name: &'static str,
    function: &ReviewedFunction,
) -> ValidationResult<&'a str> {
    value
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ValidationError::function(
                function,
                name,
                format!(
                    "reviewed function {}:{} requires {name}",
                    function.profile, function.identity
                ),
            )
        })
}
