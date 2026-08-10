//! Validation of reviewed logical types and their memory-object bindings.

use std::collections::BTreeSet;

use super::super::validation::{ValidationError, ValidationResult};
use super::super::{
    FunctionFacts, FunctionMemoryFieldFact, FunctionMemoryObjectFact, FunctionReviewStatus,
    FunctionWorkspaceSummary, ReviewedLogicalType, ReviewedMemoryObject, ReviewedTypeBinding,
};
use super::primitives::{
    validate_id, validate_identifier, validate_one_line, validate_optional_description,
};

pub(super) fn validate_types(
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
                FunctionReviewStatus::Ignored => {
                    if field.name.is_some()
                        || field.display_type.is_some()
                        || field.description.is_some()
                    {
                        return Err(ValidationError::pack(
                            "types",
                            "unreviewed or ignored logical type field cannot define reviewed claims",
                        ));
                    }
                    summary.ignored_type_fields += 1;
                }
            }
        }
        summary.unreviewed_type_fields += observed_keys.difference(&fields).count();
        summary.logical_types += 1;
        summary.type_bindings += logical_type.bindings.len();
        summary.type_fields += observed_keys.len();
    }
    Ok(())
}
fn observed_fields<'a>(
    binding: &ReviewedTypeBinding,
    facts: &'a FunctionFacts,
) -> ValidationResult<Vec<&'a FunctionMemoryFieldFact>> {
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
                    FunctionMemoryObjectFact::Argument { index: observed } if observed == *index
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
                            FunctionMemoryObjectFact::Global {
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
                            FunctionMemoryObjectFact::DereferencedGlobal {
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
                            FunctionMemoryObjectFact::Absolute {
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
