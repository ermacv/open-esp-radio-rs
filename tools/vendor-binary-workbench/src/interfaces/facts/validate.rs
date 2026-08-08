//! Internal consistency checks for generated interface facts.

use std::collections::BTreeSet;

use crate::Result;

use super::*;

pub(super) fn validate(facts: &InterfaceFacts) -> Result<()> {
    let mut artifact_indices = BTreeSet::new();
    for artifact in &facts.artifacts {
        if !artifact_indices.insert(artifact.index) {
            return Err(format!("duplicate interface artifact index {}", artifact.index).into());
        }
        if artifact.sources.is_empty() {
            return Err(format!(
                "interface artifact {} has no logical source identity",
                artifact.index
            )
            .into());
        }
        if let Some(digest) = &artifact.sha256 {
            validate_sha256(digest, "interface artifact")?;
        }
    }
    let mut table_keys = BTreeSet::new();
    for table in &facts.tables {
        if facts.artifact(table.artifact).is_none() {
            return Err(format!(
                "interface table refers to unknown artifact {}",
                table.artifact
            )
            .into());
        }
        let key = (table.artifact, &table.root, table.container_path.as_slice());
        if !table_keys.insert(key) {
            return Err("duplicate interface table candidate".into());
        }
        validate_steps(&table.container_path, "interface container path")?;
        validate_slots(&table.slots, "interface slots")?;
        if table.slots.is_empty() {
            return Err("interface table candidate has no observed slots".into());
        }
        if table.functions.is_empty() {
            return Err("interface table candidate has no calling functions".into());
        }
    }
    let mut call_keys = BTreeSet::new();
    for call in &facts.calls {
        validate_call(facts, call, &mut call_keys)?;
    }
    Ok(())
}

fn validate_call(
    facts: &InterfaceFacts,
    call: &InterfaceCallFact,
    keys: &mut BTreeSet<InterfaceCallFact>,
) -> Result<()> {
    if facts.artifact(call.artifact).is_none() {
        return Err(format!(
            "interface call refers to unknown artifact {}",
            call.artifact
        )
        .into());
    }
    if call.function.is_empty() {
        return Err("interface call has an empty function name".into());
    }
    if !matches!(call.kind.as_str(), "call" | "tail-jump" | "linked-jump") {
        return Err(format!("interface call has unsupported kind {:?}", call.kind).into());
    }
    if !keys.insert(call.clone()) {
        return Err("duplicate interface call fact".into());
    }
    for load in &call.loads {
        if !matches!(load.width, 8 | 16 | 32 | 64) {
            return Err(format!(
                "interface call target load has unsupported width {}",
                load.width
            )
            .into());
        }
    }
    match call.loads.split_last() {
        None if call.container_depth != 0 || call.slot_offset.is_some() => {
            return Err("direct interface call has inconsistent table metadata".into());
        }
        None => {}
        Some((slot, container)) => {
            if call.container_depth != container.len() || call.slot_offset != Some(slot.offset) {
                return Err("interface call has inconsistent container/slot metadata".into());
            }
            let table = facts.tables.iter().find(|table| {
                table.artifact == call.artifact
                    && table.root == call.root
                    && table.container_path == container
            });
            let Some(table) = table else {
                return Err("interface call has no matching table candidate".into());
            };
            let table_slot = table
                .slots
                .iter()
                .find(|candidate| (candidate.offset, candidate.width) == (slot.offset, slot.width));
            let Some(table_slot) = table_slot else {
                return Err("interface call has no matching table slot".into());
            };
            if !table.functions.contains(&call.function)
                || !table_slot.functions.contains(&call.function)
            {
                return Err("interface call is missing from its table function index".into());
            }
        }
    }
    for (expected, argument) in call.arguments.iter().enumerate() {
        if argument.index != expected {
            return Err("interface call arguments must use consecutive indices".into());
        }
        if !matches!(
            argument.kind.as_str(),
            "unknown" | "constant" | "pointer-provenance"
        ) {
            return Err(format!(
                "interface call argument {} has unsupported kind {:?}",
                argument.index, argument.kind
            )
            .into());
        }
        if argument.expression.is_empty() {
            return Err(format!(
                "interface call argument {} has an empty expression",
                argument.index
            )
            .into());
        }
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str, context: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(format!("{context} has invalid lowercase SHA-256 {value:?}").into());
    }
    Ok(())
}

fn validate_steps(steps: &[InterfaceFactStep], context: &str) -> Result<()> {
    let mut keys = BTreeSet::new();
    for step in steps {
        if !matches!(step.width, 8 | 16 | 32 | 64) {
            return Err(format!("{context} has unsupported width {}", step.width).into());
        }
        if !keys.insert((step.offset, step.width)) {
            return Err(format!("{context} contains a duplicate step").into());
        }
    }
    Ok(())
}

fn validate_slots(slots: &[InterfaceFactSlot], context: &str) -> Result<()> {
    let mut keys = BTreeSet::new();
    for slot in slots {
        if !matches!(slot.width, 8 | 16 | 32 | 64) {
            return Err(format!("{context} has unsupported width {}", slot.width).into());
        }
        if !keys.insert((slot.offset, slot.width)) {
            return Err(format!("{context} contains a duplicate slot").into());
        }
        if slot.functions.is_empty() {
            return Err(format!("{context} contains a slot without calling functions").into());
        }
    }
    Ok(())
}
