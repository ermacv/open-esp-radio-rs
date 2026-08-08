//! Structural, provenance, completeness, and review-coverage validation.

use std::collections::BTreeSet;

use super::validation::{ValidationError, ValidationResult};
use super::{
    FunctionContextFieldFact, FunctionFact, FunctionFacts, FunctionInputFact, FunctionPack,
    FunctionReviewStatus, FunctionWorkspaceSummary, ReviewedContextField, ReviewedFunction,
    ReviewedFunctionInput, ReviewedLogicalType, ReviewedMemoryObject,
};

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
    Ok(summary)
}

fn validate_types(
    types: &[ReviewedLogicalType],
    facts: &FunctionFacts,
    summary: &mut FunctionWorkspaceSummary,
) -> ValidationResult<()> {
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut bound_objects = BTreeSet::new();
    for logical_type in types {
        validate_id(&logical_type.id, "logical type id")
            .map_err(|message| ValidationError::pack("types", message))?;
        validate_identifier(&logical_type.name, "logical type name")
            .map_err(|message| ValidationError::pack("types", message))?;
        if !ids.insert(&logical_type.id) || !names.insert(&logical_type.name) {
            return Err(ValidationError::pack(
                "types",
                format!(
                    "duplicate logical type id or name for {:?}",
                    logical_type.id
                ),
            ));
        }
        validate_optional_description(logical_type.description.as_deref(), "logical type")?;
        if logical_type.bindings.is_empty() {
            return Err(ValidationError::pack(
                "types",
                format!(
                    "logical type {:?} requires at least one binding",
                    logical_type.id
                ),
            ));
        }
        let mut binding_names = BTreeSet::new();
        let mut observed = Vec::new();
        for binding in &logical_type.bindings {
            validate_id(&binding.profile, "logical type binding profile")
                .map_err(|message| ValidationError::pack("types", message))?;
            validate_id(&binding.source, "logical type binding source")
                .map_err(|message| ValidationError::pack("types", message))?;
            validate_identifier(&binding.name, "logical type binding name")
                .map_err(|message| ValidationError::pack("types", message))?;
            if !binding_names.insert(&binding.name) {
                return Err(ValidationError::pack(
                    "types",
                    format!(
                        "logical type {:?} has duplicate binding name {:?}",
                        logical_type.id, binding.name
                    ),
                ));
            }
            let key = (
                binding.profile.clone(),
                binding.source.clone(),
                binding.object.clone(),
            );
            if !bound_objects.insert(key) {
                return Err(ValidationError::pack(
                    "types",
                    format!(
                        "memory object bound more than once: {}:{} {:?}",
                        binding.profile, binding.source, binding.object
                    ),
                ));
            }
            observed.extend(observed_fields(binding, facts)?);
        }
        if observed.is_empty() {
            return Err(ValidationError::pack(
                "types",
                format!(
                    "logical type {:?} bindings have no generated memory-object evidence",
                    logical_type.id
                ),
            ));
        }
        let mut widths_by_offset = std::collections::BTreeMap::<i64, BTreeSet<u8>>::new();
        let observed_keys = observed
            .iter()
            .map(|field| {
                widths_by_offset
                    .entry(field.offset)
                    .or_default()
                    .insert(field.width);
                (field.offset, field.width)
            })
            .collect::<BTreeSet<_>>();
        if let Some((offset, widths)) = widths_by_offset
            .iter()
            .find(|(_, widths)| widths.len() != 1)
        {
            return Err(ValidationError::pack(
                "types",
                format!(
                    "logical type {:?} has conflicting observed widths at {offset:+#x}: {widths:?}",
                    logical_type.id
                ),
            ));
        }
        let mut fields = BTreeSet::new();
        let mut field_names = BTreeSet::new();
        for field in &logical_type.fields {
            let key = (field.offset, field.width);
            if !fields.insert(key) || !observed_keys.contains(&key) {
                return Err(ValidationError::pack(
                    "types",
                    format!(
                        "logical type {:?} has duplicate or unobserved field {:+#x}/{}",
                        logical_type.id, field.offset, field.width
                    ),
                ));
            }
            match field.status {
                FunctionReviewStatus::Reviewed => {
                    summary.reviewed_type_fields += 1;
                    let name = field.name.as_deref().ok_or_else(|| {
                        ValidationError::pack("types", "reviewed logical type field requires name")
                    })?;
                    validate_identifier(name, "logical type field name")
                        .map_err(|message| ValidationError::pack("types", message))?;
                    if !field_names.insert(name) {
                        return Err(ValidationError::pack(
                            "types",
                            format!(
                                "logical type {:?} has duplicate field name {name:?}",
                                logical_type.id
                            ),
                        ));
                    }
                    let display_type = field.display_type.as_deref().ok_or_else(|| {
                        ValidationError::pack(
                            "types",
                            "reviewed logical type field requires display-type",
                        )
                    })?;
                    validate_one_line(display_type, "logical type field display-type")?;
                    validate_optional_description(
                        field.description.as_deref(),
                        "logical type field",
                    )?;
                }
                FunctionReviewStatus::Unreviewed | FunctionReviewStatus::Ignored => {
                    if field.name.is_some()
                        || field.display_type.is_some()
                        || field.description.is_some()
                    {
                        return Err(ValidationError::pack(
                            "types",
                            "unreviewed or ignored logical type field cannot define reviewed claims",
                        ));
                    }
                    match field.status {
                        FunctionReviewStatus::Unreviewed => summary.unreviewed_type_fields += 1,
                        FunctionReviewStatus::Ignored => summary.ignored_type_fields += 1,
                        FunctionReviewStatus::Reviewed => unreachable!(),
                    }
                }
            }
        }
        if fields != observed_keys {
            let missing = observed_keys.difference(&fields).collect::<Vec<_>>();
            return Err(ValidationError::pack(
                "types",
                format!(
                    "logical type {:?} does not classify observed fields {missing:?}",
                    logical_type.id
                ),
            ));
        }
        summary.logical_types += 1;
        summary.type_bindings += logical_type.bindings.len();
        summary.type_fields += logical_type.fields.len();
    }
    Ok(())
}

fn observed_fields<'a>(
    binding: &super::ReviewedTypeBinding,
    facts: &'a FunctionFacts,
) -> ValidationResult<Vec<&'a super::FunctionMemoryFieldFact>> {
    let mut observed = Vec::new();
    match &binding.object {
        ReviewedMemoryObject::Argument { function, index } => {
            if *index >= 8 {
                return Err(ValidationError::pack(
                    "types",
                    "logical type argument binding must address arg0..arg7",
                ));
            }
            let fact = facts
                .function(&binding.profile, &binding.source, function)
                .ok_or_else(|| {
                    ValidationError::pack(
                        "types",
                        format!(
                            "stale logical type binding {}:{}:{function}",
                            binding.profile, binding.source
                        ),
                    )
                })?;
            observed.extend(fact.memory_fields.iter().filter(|field| {
                matches!(
                    field.object,
                    super::FunctionMemoryObjectFact::Argument { index: observed } if observed == *index
                )
            }));
        }
        ReviewedMemoryObject::Global { member, symbol } => {
            observed.extend(
                facts
                    .functions
                    .iter()
                    .filter(|function| {
                        function.profile == binding.profile && function.source == binding.source
                    })
                    .flat_map(|function| &function.memory_fields)
                    .filter(|field| {
                        matches!(
                            &field.object,
                            super::FunctionMemoryObjectFact::Global {
                                member: observed_member,
                                symbol: observed_symbol,
                            } if observed_member == member && observed_symbol == symbol
                        )
                    }),
            );
        }
        ReviewedMemoryObject::DereferencedGlobal {
            member,
            symbol,
            pointer_offset,
        } => {
            observed.extend(
                facts
                    .functions
                    .iter()
                    .filter(|function| {
                        function.profile == binding.profile && function.source == binding.source
                    })
                    .flat_map(|function| &function.memory_fields)
                    .filter(|field| {
                        matches!(
                            &field.object,
                            super::FunctionMemoryObjectFact::DereferencedGlobal {
                                member: observed_member,
                                symbol: observed_symbol,
                                pointer_offset: observed_offset,
                            } if observed_member == member
                                && observed_symbol == symbol
                                && observed_offset == pointer_offset
                        )
                    }),
            );
        }
        ReviewedMemoryObject::Absolute {
            address_space,
            address,
        } => {
            observed.extend(
                facts
                    .functions
                    .iter()
                    .filter(|function| {
                        function.profile == binding.profile && function.source == binding.source
                    })
                    .flat_map(|function| &function.memory_fields)
                    .filter(|field| {
                        matches!(
                            &field.object,
                            super::FunctionMemoryObjectFact::Absolute {
                                address_space: observed_space,
                                address: observed_address,
                            } if observed_space == address_space && observed_address == address
                        )
                    }),
            );
        }
    }
    if observed.is_empty() {
        return Err(ValidationError::pack(
            "types",
            format!(
                "stale logical type binding {}:{} {:?}",
                binding.profile, binding.source, binding.object
            ),
        ));
    }
    Ok(observed)
}

fn validate_one_line(value: &str, label: &str) -> ValidationResult<()> {
    if value.trim().is_empty() || value.contains(['\r', '\n']) {
        return Err(ValidationError::pack(
            "types",
            format!("{label} must be one non-empty line"),
        ));
    }
    Ok(())
}

fn validate_optional_description(value: Option<&str>, label: &str) -> ValidationResult<()> {
    if let Some(value) = value {
        validate_one_line(value, &format!("{label} description"))?;
    }
    Ok(())
}

fn validate_inputs(
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

fn validate_function(
    reviewed: &ReviewedFunction,
    fact: &FunctionFact,
    summary: &mut FunctionWorkspaceSummary,
    reviewed_names: &mut BTreeSet<String>,
) -> ValidationResult<()> {
    match reviewed.status {
        FunctionReviewStatus::Unreviewed => {
            if reviewed.name.is_some()
                || reviewed.role.is_some()
                || reviewed.summary.is_some()
                || reviewed.accept_incomplete
            {
                return Err(ValidationError::function(
                    reviewed,
                    "status",
                    format!(
                        "unreviewed function {}:{} cannot make reviewed claims",
                        reviewed.profile, reviewed.identity
                    ),
                ));
            }
            summary.unreviewed_functions += usize::from(fact.is_root());
        }
        FunctionReviewStatus::Ignored => {
            if reviewed.name.is_some()
                || reviewed.role.is_some()
                || reviewed.summary.is_some()
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
        }
    }
    validate_contexts(reviewed, fact, summary)
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
        if !contexts.insert(context.argument) || context.argument >= 8 {
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
            FunctionReviewStatus::Unreviewed => {
                if context.name.is_some() || context.type_name.is_some() {
                    return Err(ValidationError::context(
                        reviewed,
                        context,
                        "status",
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
    context: &super::ReviewedContext,
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
        FunctionReviewStatus::Unreviewed => {
            if field.name.is_some() || field.display_type.is_some() || field.description.is_some() {
                return Err(ValidationError::field(
                    function,
                    context,
                    field,
                    "status",
                    "unreviewed context field cannot define reviewed claims",
                ));
            }
            summary.unreviewed_fields += 1;
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

fn validate_id(value: &str, context: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(format!("invalid {context} {value:?}"));
    }
    Ok(())
}

fn validate_identifier(value: &str, context: &str) -> std::result::Result<(), String> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(format!("invalid {context} {value:?}"));
    }
    Ok(())
}

fn validate_sha256(value: &str, context: &str) -> std::result::Result<(), String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("{context} has invalid lowercase SHA-256 {value:?}"));
    }
    Ok(())
}
