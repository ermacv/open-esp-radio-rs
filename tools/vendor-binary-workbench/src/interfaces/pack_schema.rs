//! Syntactic, layout, ABI, and semantic-link validation for interface packs.

use std::collections::BTreeSet;

use super::{
    InterfaceAnchor, InterfaceFactStep, InterfaceGuard, InterfaceRootSelector, InterfaceSlot,
    PackOrigin, ReviewStatus, SemanticCatalogs, validate_dotted_id, validate_sha256,
};
use crate::Result;

pub(super) fn validate_anchor_shape(
    anchor: &InterfaceAnchor,
    catalogs: &SemanticCatalogs,
) -> Result<()> {
    validate_dotted_id(&anchor.id, "interface anchor id")?;
    validate_source_id(&anchor.source)?;
    if anchor.origin == PackOrigin::Manual && anchor.status != ReviewStatus::Reviewed {
        return Err(crate::Error::invalid(format!(
            "manual interface anchor {:?} must have status = \"reviewed\"",
            anchor.id
        )));
    }
    validate_selector(&anchor.root, &anchor.id)?;
    validate_steps(
        &anchor.container_path,
        &format!("anchor {:?} container path", anchor.id),
    )?;
    let artifact_guards = anchor
        .guards
        .iter()
        .filter(|guard| matches!(guard, InterfaceGuard::ArtifactSha256 { .. }))
        .count();
    if artifact_guards > 1 {
        return Err(crate::Error::invalid(format!(
            "anchor {:?} has multiple artifact digest guards",
            anchor.id
        )));
    }
    for guard in &anchor.guards {
        validate_guard(guard, anchor)?;
    }
    match anchor.status {
        ReviewStatus::Reviewed => validate_reviewed_layout(anchor)?,
        ReviewStatus::Ignored if !anchor.slots.is_empty() => {
            return Err(crate::Error::invalid(format!(
                "ignored anchor {:?} cannot define slots",
                anchor.id
            )));
        }
        ReviewStatus::Unreviewed => {
            if anchor.slots.iter().any(|slot| {
                slot.status != ReviewStatus::Unreviewed
                    || slot.name.is_some()
                    || slot.semantic.is_some()
            }) {
                return Err(crate::Error::invalid(format!(
                    "unreviewed anchor {:?} cannot contain reviewed slot claims",
                    anchor.id
                )));
            }
        }
        ReviewStatus::Ignored => {}
    }
    let mut slot_keys = BTreeSet::new();
    let mut slot_names = BTreeSet::new();
    for slot in &anchor.slots {
        if !slot_keys.insert((slot.offset, slot.width)) {
            return Err(crate::Error::invalid(format!(
                "anchor {:?} has a duplicate slot",
                anchor.id
            )));
        }
        validate_slot(slot, anchor, catalogs)?;
        if let Some(name) = &slot.name
            && !slot_names.insert(name.as_str())
        {
            return Err(crate::Error::invalid(format!(
                "anchor {:?} has duplicate reviewed slot name {name:?}",
                anchor.id
            )));
        }
    }
    Ok(())
}

fn validate_selector(selector: &InterfaceRootSelector, anchor: &str) -> Result<()> {
    match selector {
        InterfaceRootSelector::RelocatedSymbol {
            member,
            symbol,
            addressing,
            ..
        } => {
            if symbol.is_empty() {
                return Err(crate::Error::invalid(format!(
                    "anchor {anchor:?} has an empty symbol selector"
                )));
            }
            if member.as_deref() == Some("") {
                return Err(crate::Error::invalid(format!(
                    "anchor {anchor:?} has an empty member selector"
                )));
            }
            if !matches!(addressing.as_str(), "absolute" | "pc-relative" | "got") {
                return Err(crate::Error::invalid(format!(
                    "anchor {anchor:?} has unsupported symbol addressing {addressing:?}"
                )));
            }
        }
        InterfaceRootSelector::FunctionArgument { argument } if *argument >= 8 => {
            return Err(crate::Error::invalid(format!(
                "anchor {anchor:?} argument root exceeds RV32 ILP32 a0..a7"
            )));
        }
        InterfaceRootSelector::FunctionArgument { .. }
        | InterfaceRootSelector::AbsoluteAddress { .. } => {}
    }
    Ok(())
}

fn validate_reviewed_layout(anchor: &InterfaceAnchor) -> Result<()> {
    let version = anchor
        .layout_version
        .as_deref()
        .ok_or_else(|| format!("reviewed anchor {:?} requires layout-version", anchor.id))
        .map_err(crate::Error::invalid)?;
    if version.trim().is_empty() || version == "unreviewed" {
        return Err(crate::Error::invalid(format!(
            "reviewed anchor {:?} requires a non-placeholder layout-version",
            anchor.id
        )));
    }
    let pointer_width = anchor
        .pointer_width
        .ok_or_else(|| format!("reviewed anchor {:?} requires pointer-width", anchor.id))
        .map_err(crate::Error::invalid)?;
    if !matches!(pointer_width, 16 | 32 | 64) {
        return Err(crate::Error::invalid(format!(
            "reviewed anchor {:?} has unsupported pointer width {pointer_width}",
            anchor.id
        )));
    }
    let size = anchor
        .layout_size
        .ok_or_else(|| format!("reviewed anchor {:?} requires layout-size", anchor.id))
        .map_err(crate::Error::invalid)?;
    if size == 0 {
        return Err(crate::Error::invalid(format!(
            "reviewed anchor {:?} has an empty layout",
            anchor.id
        )));
    }
    let stride = anchor
        .slot_stride
        .ok_or_else(|| format!("reviewed anchor {:?} requires slot-stride", anchor.id))
        .map_err(crate::Error::invalid)?;
    if stride == 0 {
        return Err(crate::Error::invalid(format!(
            "reviewed anchor {:?} has zero slot stride",
            anchor.id
        )));
    }
    if anchor.guards.is_empty() {
        return Err(crate::Error::invalid(format!(
            "reviewed anchor {:?} requires an artifact-sha256 or runtime-value guard",
            anchor.id
        )));
    }
    Ok(())
}

fn validate_guard(guard: &InterfaceGuard, anchor: &InterfaceAnchor) -> Result<()> {
    match guard {
        InterfaceGuard::ArtifactSha256 { sha256 } => {
            validate_sha256(sha256, &format!("anchor {:?} guard", anchor.id))
        }
        InterfaceGuard::RuntimeValue {
            purpose,
            offset,
            width,
            mask,
            value,
        } => {
            validate_dotted_id(purpose, "runtime guard purpose")?;
            if *offset < 0 || !matches!(width, 8 | 16 | 32 | 64) {
                return Err(crate::Error::invalid(format!(
                    "anchor {:?} has an invalid runtime guard",
                    anchor.id
                )));
            }
            let width_mask = width_mask(*width);
            if *mask == 0 || mask & !width_mask != 0 || value & !mask != 0 {
                return Err(crate::Error::invalid(format!(
                    "anchor {:?} runtime guard mask/value exceeds width or value has unmasked bits",
                    anchor.id
                )));
            }
            if let Some(size) = anchor.layout_size {
                let end = u32::try_from(*offset)
                    .ok()
                    .and_then(|offset| offset.checked_add(u32::from(*width) / 8));
                if end.is_none_or(|end| end > size) {
                    return Err(crate::Error::invalid(format!(
                        "anchor {:?} runtime guard lies outside layout-size",
                        anchor.id
                    )));
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
) -> Result<()> {
    if !matches!(slot.width, 16 | 32 | 64) {
        return Err(crate::Error::invalid(format!(
            "anchor {:?} has unsupported slot width",
            anchor.id
        )));
    }
    if slot.origin == PackOrigin::Manual && slot.status != ReviewStatus::Reviewed {
        return Err(crate::Error::invalid(format!(
            "manual slot at {:+#x} in anchor {:?} must be reviewed",
            slot.offset, anchor.id
        )));
    }
    match slot.status {
        ReviewStatus::Reviewed => {
            if anchor.status != ReviewStatus::Reviewed {
                return Err(crate::Error::invalid(format!(
                    "reviewed slot at {:+#x} requires reviewed anchor {:?}",
                    slot.offset, anchor.id
                )));
            }
            let name = slot
                .name
                .as_deref()
                .ok_or_else(|| format!("reviewed slot at {:+#x} requires a name", slot.offset))
                .map_err(crate::Error::invalid)?;
            validate_c_identifier(name, "interface slot name")?;
            let arguments = slot
                .arguments
                .as_ref()
                .ok_or_else(|| {
                    format!("reviewed slot {name:?} requires an explicit arguments array")
                })
                .map_err(crate::Error::invalid)?;
            for argument in arguments {
                validate_abi_type(argument, false, &format!("slot {name:?} argument"))?;
            }
            let return_type = slot
                .return_type
                .as_deref()
                .ok_or_else(|| format!("reviewed slot {name:?} requires an explicit return type"))
                .map_err(crate::Error::invalid)?;
            validate_abi_type(return_type, true, &format!("slot {name:?} return"))?;
            if let Some(semantic_id) = &slot.semantic {
                validate_dotted_id(semantic_id, "slot semantic operation")?;
                let operation = catalogs
                    .get(semantic_id)
                    .ok_or_else(|| {
                        format!(
                            "slot {name:?} refers to unknown semantic operation {semantic_id:?}"
                        )
                    })
                    .map_err(crate::Error::invalid)?;
                if operation.argument_roles.len() != arguments.len() {
                    return Err(crate::Error::invalid(format!(
                        "slot {name:?} has {} ABI arguments but semantic operation {semantic_id:?} has {} roles",
                        arguments.len(),
                        operation.argument_roles.len()
                    )));
                }
                if operation.variadic != slot.variadic {
                    return Err(crate::Error::invalid(format!(
                        "slot {name:?} variadic ABI does not match semantic operation {semantic_id:?}"
                    )));
                }
                if (operation.return_role == "none") != (return_type == "void") {
                    return Err(crate::Error::invalid(format!(
                        "slot {name:?} return type does not match semantic return role {:?}",
                        operation.return_role
                    )));
                }
            }
            validate_slot_layout(slot, anchor)
        }
        ReviewStatus::Ignored => {
            if slot.name.is_some()
                || slot.arguments.is_some()
                || slot.return_type.is_some()
                || slot.semantic.is_some()
            {
                return Err(crate::Error::invalid(format!(
                    "ignored slot at {:+#x} in anchor {:?} cannot claim a name, ABI, or semantic operation",
                    slot.offset, anchor.id
                )));
            }
            Ok(())
        }
        ReviewStatus::Unreviewed => {
            if slot.name.is_some()
                || slot.arguments.is_some()
                || slot.return_type.is_some()
                || slot.semantic.is_some()
            {
                return Err(crate::Error::invalid(format!(
                    "unreviewed slot at {:+#x} in anchor {:?} cannot claim reviewed metadata",
                    slot.offset, anchor.id
                )));
            }
            Ok(())
        }
    }
}

fn validate_slot_layout(slot: &InterfaceSlot, anchor: &InterfaceAnchor) -> Result<()> {
    let pointer_width = anchor
        .pointer_width
        .expect("reviewed slots require a validated reviewed layout");
    if slot.width != pointer_width {
        return Err(crate::Error::invalid(format!(
            "reviewed slot at {:+#x} in anchor {:?} has width {}, expected pointer-width {pointer_width}",
            slot.offset, anchor.id, slot.width
        )));
    }
    let offset = u32::try_from(slot.offset)
        .map_err(|_| {
            format!(
                "reviewed slot at {:+#x} in anchor {:?} has a negative layout offset",
                slot.offset, anchor.id
            )
        })
        .map_err(crate::Error::invalid)?;
    let stride = u32::from(
        anchor
            .slot_stride
            .expect("reviewed slots require a validated reviewed layout"),
    );
    if offset % stride != 0 {
        return Err(crate::Error::invalid(format!(
            "reviewed slot at {offset:#x} in anchor {:?} is not aligned to slot-stride {stride}",
            anchor.id
        )));
    }
    let end = offset
        .checked_add(u32::from(slot.width) / 8)
        .ok_or("interface slot range overflows")
        .map_err(crate::Error::invalid)?;
    if end
        > anchor
            .layout_size
            .expect("reviewed slots require a validated reviewed layout")
    {
        return Err(crate::Error::invalid(format!(
            "reviewed slot at {offset:#x} in anchor {:?} exceeds layout-size",
            anchor.id
        )));
    }
    Ok(())
}

fn validate_steps(steps: &[InterfaceFactStep], context: &str) -> Result<()> {
    for step in steps {
        if !matches!(step.width, 8 | 16 | 32 | 64) {
            return Err(crate::Error::invalid(format!(
                "{context} has unsupported width {}",
                step.width
            )));
        }
    }
    Ok(())
}

fn validate_c_identifier(value: &str, context: &str) -> Result<()> {
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

fn validate_source_id(value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || !value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(crate::Error::invalid(format!(
            "invalid interface source id {value:?}"
        )));
    }
    Ok(())
}

fn validate_abi_type(value: &str, allow_void: bool, context: &str) -> Result<()> {
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
        Err(crate::Error::invalid(format!(
            "unsupported ABI type {value:?} in {context}"
        )))
    }
}

fn width_mask(width: u8) -> u64 {
    if width == 64 {
        u64::MAX
    } else {
        (1_u64 << width) - 1
    }
}
