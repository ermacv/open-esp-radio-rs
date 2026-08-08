//! Syntactic, layout, ABI, and semantic-link validation for interface packs.

use std::collections::BTreeSet;

use super::validation::{ValidationError, ValidationResult};
use super::{
    InterfaceAnchor, InterfaceFactStep, InterfaceGuard, InterfaceRootSelector, InterfaceSlot,
    PackOrigin, ReviewStatus, SemanticCatalogs, validate_dotted_id, validate_sha256,
};

pub(super) fn validate_anchor_shape(
    anchor: &InterfaceAnchor,
    catalogs: &SemanticCatalogs,
) -> ValidationResult<()> {
    validate_dotted_id(&anchor.id, "interface anchor id")
        .map_err(|error| ValidationError::anchor(anchor, "id", error.to_string()))?;
    validate_source_id(&anchor.source)
        .map_err(|message| ValidationError::anchor(anchor, "source", message))?;
    if let Some(contract) = &anchor.execution_contract {
        validate_dotted_id(contract, "interface execution contract id").map_err(|error| {
            ValidationError::anchor(anchor, "execution-contract", error.to_string())
        })?;
    }
    if anchor.origin == PackOrigin::Manual && anchor.status != ReviewStatus::Reviewed {
        return Err(ValidationError::anchor(
            anchor,
            "status",
            format!(
                "manual interface anchor {:?} must have status = \"reviewed\"",
                anchor.id
            ),
        ));
    }
    validate_selector(&anchor.root, anchor)?;
    validate_steps(
        &anchor.container_path,
        &format!("anchor {:?} container path", anchor.id),
    )
    .map_err(|message| ValidationError::anchor(anchor, "container-path", message))?;
    let artifact_guards = anchor
        .guards
        .iter()
        .filter(|guard| matches!(guard, InterfaceGuard::ArtifactSha256 { .. }))
        .count();
    if artifact_guards > 1 {
        return Err(ValidationError::anchor(
            anchor,
            "guards",
            format!("anchor {:?} has multiple artifact digest guards", anchor.id),
        ));
    }
    for (index, guard) in anchor.guards.iter().enumerate() {
        validate_guard(guard, anchor, index)?;
    }
    validate_index_domains(anchor)?;
    match anchor.status {
        ReviewStatus::Reviewed => validate_reviewed_layout(anchor)?,
        ReviewStatus::Ignored
            if !anchor.slots.is_empty() || anchor.execution_contract.is_some() =>
        {
            return Err(ValidationError::anchor(
                anchor,
                "status",
                format!("ignored anchor {:?} cannot define slots", anchor.id),
            ));
        }
        ReviewStatus::Unreviewed => {
            if anchor.execution_contract.is_some()
                || anchor.slots.iter().any(|slot| {
                    slot.status != ReviewStatus::Unreviewed
                        || slot.name.is_some()
                        || slot.semantic.is_some()
                        || slot.execution_model.is_some()
                })
            {
                return Err(ValidationError::anchor(
                    anchor,
                    "status",
                    format!(
                        "unreviewed anchor {:?} cannot contain reviewed slot claims",
                        anchor.id
                    ),
                ));
            }
        }
        ReviewStatus::Ignored => {}
    }
    let mut slot_keys = BTreeSet::new();
    let mut slot_names = BTreeSet::new();
    for slot in &anchor.slots {
        if !slot_keys.insert((slot.offset, slot.width)) {
            return Err(ValidationError::slot(
                anchor,
                slot,
                "offset",
                format!("anchor {:?} has a duplicate slot", anchor.id),
            ));
        }
        validate_slot(slot, anchor, catalogs)?;
        if let Some(name) = &slot.name
            && !slot_names.insert(name.as_str())
        {
            return Err(ValidationError::slot(
                anchor,
                slot,
                "name",
                format!(
                    "anchor {:?} has duplicate reviewed slot name {name:?}",
                    anchor.id
                ),
            ));
        }
    }
    Ok(())
}

fn validate_index_domains(anchor: &InterfaceAnchor) -> ValidationResult<()> {
    let mut arguments = BTreeSet::new();
    for domain in &anchor.index_domains {
        if anchor.status != ReviewStatus::Reviewed {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                "index domains are reviewed control-flow contracts",
            ));
        }
        if domain.argument >= 8 {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                "index-domain argument must be one of a0..a7",
            ));
        }
        if !arguments.insert(domain.argument) {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                format!("duplicate index domain for argument {}", domain.argument),
            ));
        }
        if domain.min > domain.max {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                "index-domain minimum exceeds maximum",
            ));
        }
        if u64::from(domain.max) - u64::from(domain.min) + 1 > 4_096 {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                "index domain exceeds the 4096-value fail-closed limit",
            ));
        }
        if domain.evidence.trim().is_empty() || domain.evidence.contains(['\r', '\n']) {
            return Err(ValidationError::anchor(
                anchor,
                "index-domains",
                "index-domain evidence must be one non-empty line",
            ));
        }
    }
    Ok(())
}

fn validate_selector(
    selector: &InterfaceRootSelector,
    anchor: &InterfaceAnchor,
) -> ValidationResult<()> {
    match selector {
        InterfaceRootSelector::RelocatedSymbol {
            member,
            symbol,
            addressing,
            ..
        } => {
            if symbol.is_empty() {
                return Err(ValidationError::anchor(
                    anchor,
                    "symbol",
                    format!("anchor {:?} has an empty symbol selector", anchor.id),
                ));
            }
            if member.as_deref() == Some("") {
                return Err(ValidationError::anchor(
                    anchor,
                    "member",
                    format!("anchor {:?} has an empty member selector", anchor.id),
                ));
            }
            if !matches!(addressing.as_str(), "absolute" | "pc-relative" | "got") {
                return Err(ValidationError::anchor(
                    anchor,
                    "addressing",
                    format!(
                        "anchor {:?} has unsupported symbol addressing {addressing:?}",
                        anchor.id
                    ),
                ));
            }
        }
        InterfaceRootSelector::FunctionArgument { argument } if *argument >= 8 => {
            return Err(ValidationError::anchor(
                anchor,
                "argument",
                format!(
                    "anchor {:?} argument root exceeds RV32 ILP32 a0..a7",
                    anchor.id
                ),
            ));
        }
        InterfaceRootSelector::FunctionArgument { .. }
        | InterfaceRootSelector::AbsoluteAddress { .. } => {}
    }
    Ok(())
}

fn validate_reviewed_layout(anchor: &InterfaceAnchor) -> ValidationResult<()> {
    let version = anchor.layout_version.as_deref().ok_or_else(|| {
        ValidationError::anchor(
            anchor,
            "layout-version",
            format!("reviewed anchor {:?} requires layout-version", anchor.id),
        )
    })?;
    if version.trim().is_empty() || version == "unreviewed" {
        return Err(ValidationError::anchor(
            anchor,
            "layout-version",
            format!(
                "reviewed anchor {:?} requires a non-placeholder layout-version",
                anchor.id
            ),
        ));
    }
    let pointer_width = anchor.pointer_width.ok_or_else(|| {
        ValidationError::anchor(
            anchor,
            "pointer-width",
            format!("reviewed anchor {:?} requires pointer-width", anchor.id),
        )
    })?;
    if !matches!(pointer_width, 16 | 32 | 64) {
        return Err(ValidationError::anchor(
            anchor,
            "pointer-width",
            format!(
                "reviewed anchor {:?} has unsupported pointer width {pointer_width}",
                anchor.id
            ),
        ));
    }
    let size = anchor.layout_size.ok_or_else(|| {
        ValidationError::anchor(
            anchor,
            "layout-size",
            format!("reviewed anchor {:?} requires layout-size", anchor.id),
        )
    })?;
    if size == 0 {
        return Err(ValidationError::anchor(
            anchor,
            "layout-size",
            format!("reviewed anchor {:?} has an empty layout", anchor.id),
        ));
    }
    let stride = anchor.slot_stride.ok_or_else(|| {
        ValidationError::anchor(
            anchor,
            "slot-stride",
            format!("reviewed anchor {:?} requires slot-stride", anchor.id),
        )
    })?;
    if stride == 0 {
        return Err(ValidationError::anchor(
            anchor,
            "slot-stride",
            format!("reviewed anchor {:?} has zero slot stride", anchor.id),
        ));
    }
    if anchor.guards.is_empty() {
        return Err(ValidationError::anchor(
            anchor,
            "guards",
            format!(
                "reviewed anchor {:?} requires an artifact-sha256 or runtime-value guard",
                anchor.id
            ),
        ));
    }
    Ok(())
}

fn validate_guard(
    guard: &InterfaceGuard,
    anchor: &InterfaceAnchor,
    index: usize,
) -> ValidationResult<()> {
    match guard {
        InterfaceGuard::ArtifactSha256 { sha256 } => {
            validate_sha256(sha256, &format!("anchor {:?} guard", anchor.id))
                .map_err(|error| ValidationError::guard(anchor, index, "sha256", error.to_string()))
        }
        InterfaceGuard::RuntimeValue {
            purpose,
            offset,
            width,
            mask,
            value,
        } => {
            validate_dotted_id(purpose, "runtime guard purpose").map_err(|error| {
                ValidationError::guard(anchor, index, "purpose", error.to_string())
            })?;
            if *offset < 0 || !matches!(width, 8 | 16 | 32 | 64) {
                let key = if *offset < 0 { "offset" } else { "width" };
                return Err(ValidationError::guard(
                    anchor,
                    index,
                    key,
                    format!("anchor {:?} has an invalid runtime guard", anchor.id),
                ));
            }
            let width_mask = width_mask(*width);
            if *mask == 0 || mask & !width_mask != 0 || value & !mask != 0 {
                return Err(ValidationError::guard(
                    anchor,
                    index,
                    "mask",
                    format!(
                        "anchor {:?} runtime guard mask/value exceeds width or value has unmasked bits",
                        anchor.id
                    ),
                ));
            }
            if let Some(size) = anchor.layout_size {
                let end = u32::try_from(*offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(u32::from(*width) / 8));
                if end.is_none_or(|end| end > size) {
                    return Err(ValidationError::guard(
                        anchor,
                        index,
                        "offset",
                        format!(
                            "anchor {:?} runtime guard lies outside layout-size",
                            anchor.id
                        ),
                    ));
                }
            }
            Ok(())
        }
    }
}

fn validate_slot(
    slot: &InterfaceSlot,
    anchor: &InterfaceAnchor,
    catalogs: &SemanticCatalogs,
) -> ValidationResult<()> {
    if let Some(model) = &slot.execution_model {
        validate_dotted_id(model, "interface execution model id").map_err(|error| {
            ValidationError::slot(anchor, slot, "execution-model", error.to_string())
        })?;
        if anchor.execution_contract.is_none() {
            return Err(ValidationError::slot(
                anchor,
                slot,
                "execution-model",
                format!("slot execution model {model:?} requires anchor execution-contract"),
            ));
        }
    }
    if !matches!(slot.width, 16 | 32 | 64) {
        return Err(ValidationError::slot(
            anchor,
            slot,
            "width",
            format!("anchor {:?} has unsupported slot width", anchor.id),
        ));
    }
    if slot.origin == PackOrigin::Manual && slot.status != ReviewStatus::Reviewed {
        return Err(ValidationError::slot(
            anchor,
            slot,
            "status",
            format!(
                "manual slot at {:+#x} in anchor {:?} must be reviewed",
                slot.offset, anchor.id
            ),
        ));
    }
    match slot.status {
        ReviewStatus::Reviewed => {
            if anchor.status != ReviewStatus::Reviewed {
                return Err(ValidationError::slot(
                    anchor,
                    slot,
                    "status",
                    format!(
                        "reviewed slot at {:+#x} requires reviewed anchor {:?}",
                        slot.offset, anchor.id
                    ),
                ));
            }
            let name = slot.name.as_deref().ok_or_else(|| {
                ValidationError::slot(
                    anchor,
                    slot,
                    "name",
                    format!("reviewed slot at {:+#x} requires a name", slot.offset),
                )
            })?;
            validate_c_identifier(name, "interface slot name")
                .map_err(|message| ValidationError::slot(anchor, slot, "name", message))?;
            let arguments = slot.arguments.as_ref().ok_or_else(|| {
                ValidationError::slot(
                    anchor,
                    slot,
                    "arguments",
                    format!("reviewed slot {name:?} requires an explicit arguments array"),
                )
            })?;
            for argument in arguments {
                validate_abi_type(argument, false, &format!("slot {name:?} argument"))
                    .map_err(|message| ValidationError::slot(anchor, slot, "arguments", message))?;
            }
            let return_type = slot.return_type.as_deref().ok_or_else(|| {
                ValidationError::slot(
                    anchor,
                    slot,
                    "return",
                    format!("reviewed slot {name:?} requires an explicit return type"),
                )
            })?;
            validate_abi_type(return_type, true, &format!("slot {name:?} return"))
                .map_err(|message| ValidationError::slot(anchor, slot, "return", message))?;
            if let Some(semantic_id) = &slot.semantic {
                validate_dotted_id(semantic_id, "slot semantic operation").map_err(|error| {
                    ValidationError::slot(anchor, slot, "semantic", error.to_string())
                })?;
                let operation = catalogs.get(semantic_id).ok_or_else(|| {
                    ValidationError::slot(
                        anchor,
                        slot,
                        "semantic",
                        format!(
                            "slot {name:?} refers to unknown semantic operation {semantic_id:?}"
                        ),
                    )
                })?;
                if operation.argument_roles.len() != arguments.len() {
                    return Err(ValidationError::slot(
                        anchor,
                        slot,
                        "arguments",
                        format!(
                            "slot {name:?} has {} ABI arguments but semantic operation {semantic_id:?} has {} roles",
                            arguments.len(),
                            operation.argument_roles.len()
                        ),
                    ));
                }
                if operation.variadic != slot.variadic {
                    return Err(ValidationError::slot(
                        anchor,
                        slot,
                        "variadic",
                        format!(
                            "slot {name:?} variadic ABI does not match semantic operation {semantic_id:?}"
                        ),
                    ));
                }
                if (operation.return_role == "none") != (return_type == "void") {
                    return Err(ValidationError::slot(
                        anchor,
                        slot,
                        "return",
                        format!(
                            "slot {name:?} return type does not match semantic return role {:?}",
                            operation.return_role
                        ),
                    ));
                }
            }
            validate_slot_layout(slot, anchor)
        }
        ReviewStatus::Ignored => {
            if slot.name.is_some()
                || slot.arguments.is_some()
                || slot.return_type.is_some()
                || slot.semantic.is_some()
                || slot.execution_model.is_some()
            {
                return Err(ValidationError::slot(
                    anchor,
                    slot,
                    "status",
                    format!(
                        "ignored slot at {:+#x} in anchor {:?} cannot claim a name, ABI, or semantic operation",
                        slot.offset, anchor.id
                    ),
                ));
            }
            Ok(())
        }
        ReviewStatus::Unreviewed => {
            if slot.name.is_some()
                || slot.arguments.is_some()
                || slot.return_type.is_some()
                || slot.semantic.is_some()
                || slot.execution_model.is_some()
            {
                return Err(ValidationError::slot(
                    anchor,
                    slot,
                    "status",
                    format!(
                        "unreviewed slot at {:+#x} in anchor {:?} cannot claim reviewed metadata",
                        slot.offset, anchor.id
                    ),
                ));
            }
            Ok(())
        }
    }
}

fn validate_slot_layout(slot: &InterfaceSlot, anchor: &InterfaceAnchor) -> ValidationResult<()> {
    let pointer_width = anchor
        .pointer_width
        .expect("reviewed slots require a validated reviewed layout");
    if slot.width != pointer_width {
        return Err(ValidationError::slot(
            anchor,
            slot,
            "width",
            format!(
                "reviewed slot at {:+#x} in anchor {:?} has width {}, expected pointer-width {pointer_width}",
                slot.offset, anchor.id, slot.width
            ),
        ));
    }
    let offset = u32::try_from(slot.offset).map_err(|_| {
        ValidationError::slot(
            anchor,
            slot,
            "offset",
            format!(
                "reviewed slot at {:+#x} in anchor {:?} has a negative layout offset",
                slot.offset, anchor.id
            ),
        )
    })?;
    let stride = u32::from(
        anchor
            .slot_stride
            .expect("reviewed slots require a validated reviewed layout"),
    );
    if offset % stride != 0 {
        return Err(ValidationError::slot(
            anchor,
            slot,
            "offset",
            format!(
                "reviewed slot at {offset:#x} in anchor {:?} is not aligned to slot-stride {stride}",
                anchor.id
            ),
        ));
    }
    let end = offset
        .checked_add(u32::from(slot.width) / 8)
        .ok_or_else(|| {
            ValidationError::slot(anchor, slot, "offset", "interface slot range overflows")
        })?;
    if end
        > anchor
            .layout_size
            .expect("reviewed slots require a validated reviewed layout")
    {
        return Err(ValidationError::slot(
            anchor,
            slot,
            "offset",
            format!(
                "reviewed slot at {offset:#x} in anchor {:?} exceeds layout-size",
                anchor.id
            ),
        ));
    }
    Ok(())
}

fn validate_steps(steps: &[InterfaceFactStep], context: &str) -> std::result::Result<(), String> {
    for step in steps {
        if !matches!(step.width, 8 | 16 | 32 | 64) {
            return Err(format!("{context} has unsupported width {}", step.width));
        }
    }
    Ok(())
}

fn validate_c_identifier(value: &str, context: &str) -> std::result::Result<(), String> {
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

fn validate_source_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(format!("invalid interface source id {value:?}"));
    }
    Ok(())
}

fn validate_abi_type(
    value: &str,
    allow_void: bool,
    context: &str,
) -> std::result::Result<(), String> {
    if (allow_void && value == "void")
        || matches!(
            value,
            "bool"
                | "i8"
                | "u8"
                | "i16"
                | "u16"
                | "i32"
                | "u32"
                | "isize"
                | "usize"
                | "ptr"
                | "const-ptr"
                | "mut-ptr"
                | "out-ptr"
                | "fn-ptr"
                | "opaque-handle"
        )
    {
        Ok(())
    } else {
        Err(format!("unsupported ABI type {value:?} in {context}"))
    }
}

fn width_mask(width: u8) -> u64 {
    if width == 64 {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}
