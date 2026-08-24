//! Internal consistency checks for generated interface facts.

use std::collections::BTreeSet;

use crate::Result;

use super::*;

pub(super) fn validate(facts: &InterfaceFacts) -> Result<()> {
    let mut artifact_indices = BTreeSet::new();
    for artifact in &facts.artifacts {
        if !artifact_indices.insert(artifact.index) {
            return Err(crate::Error::invalid(format!(
                "duplicate interface artifact index {}",
                artifact.index
            )));
        }
        if artifact.sources.is_empty() {
            return Err(crate::Error::invalid(format!(
                "interface artifact {} has no logical source identity",
                artifact.index
            )));
        }
        if let Some(digest) = &artifact.sha256 {
            validate_sha256(digest, "interface artifact")?;
        }
    }
    let mut table_keys = BTreeSet::new();
    for table in &facts.tables {
        if facts.artifact(table.artifact).is_none() {
            return Err(crate::Error::invalid(format!(
                "interface table refers to unknown artifact {}",
                table.artifact
            )));
        }
        let key = (table.artifact, &table.root, table.container_path.as_slice());
        if !table_keys.insert(key) {
            return Err(crate::Error::invalid("duplicate interface table candidate"));
        }
        validate_steps(&table.container_path, "interface container path")?;
        validate_slots(&table.slots, "interface slots")?;
        if table.slots.is_empty() {
            return Err(crate::Error::invalid(
                "interface table candidate has no observed slots",
            ));
        }
        if table.functions.is_empty() {
            return Err(crate::Error::invalid(
                "interface table candidate has no calling functions",
            ));
        }
    }
    let mut call_keys = BTreeSet::new();
    for call in &facts.calls {
        validate_call(facts, call, &mut call_keys)?;
    }
    let mut assignment_keys = BTreeSet::new();
    for assignment in &facts.assignments {
        if facts.artifact(assignment.artifact).is_none() {
            return Err(crate::Error::invalid(format!(
                "interface assignment refers to unknown artifact {}",
                assignment.artifact
            )));
        }
        if assignment.function.is_empty() {
            return Err(crate::Error::invalid(
                "interface assignment has an empty producer function",
            ));
        }
        if assignment.width != 32 {
            return Err(crate::Error::invalid(format!(
                "interface assignment has unsupported pointer width {}",
                assignment.width
            )));
        }
        validate_steps(
            &assignment.container_path,
            "interface assignment container path",
        )?;
        validate_bounded_data_root(&assignment.root, "interface assignment root")?;
        validate_bounded_data_root(&assignment.target, "interface assignment target")?;
        if let InterfaceFactRoot::BoundedDataAddress {
            address,
            symbol_address,
            symbol_size,
            ..
        } = &assignment.root
            && assignment.container_path.is_empty()
        {
            if assignment.offset != 0 {
                return Err(crate::Error::invalid(
                    "bounded interface assignment root is not normalized to offset zero",
                ));
            }
            let end = symbol_address
                .checked_add(*symbol_size)
                .ok_or_else(|| crate::Error::invalid("bounded data-symbol range overflows"))?;
            let access_end = address
                .checked_add(u32::from(assignment.width) / 8)
                .ok_or_else(|| crate::Error::invalid("bounded assignment access overflows"))?;
            if access_end > end {
                return Err(crate::Error::invalid(
                    "bounded interface assignment store exceeds its data symbol",
                ));
            }
        }
        if !matches!(
            assignment.target,
            InterfaceFactRoot::RelocatedSymbol { .. }
                | InterfaceFactRoot::FunctionArgument { .. }
                | InterfaceFactRoot::BoundedDataAddress { .. }
        ) {
            return Err(crate::Error::invalid(
                "interface assignment target lacks function-pointer provenance",
            ));
        }
        if !assignment_keys.insert(assignment.clone()) {
            return Err(crate::Error::invalid("duplicate interface assignment fact"));
        }
    }
    Ok(())
}

fn validate_bounded_data_root(root: &InterfaceFactRoot, context: &str) -> Result<()> {
    let InterfaceFactRoot::BoundedDataAddress {
        canonical,
        member,
        symbol,
        address,
        symbol_address,
        symbol_size,
        ..
    } = root
    else {
        return Ok(());
    };
    if symbol.is_empty() || *symbol_size == 0 {
        return Err(crate::Error::invalid(format!(
            "{context} has an empty data-symbol identity or range"
        )));
    }
    let end = symbol_address
        .checked_add(*symbol_size)
        .ok_or_else(|| crate::Error::invalid(format!("{context} data-symbol range overflows")))?;
    if *address < *symbol_address || *address >= end {
        return Err(crate::Error::invalid(format!(
            "{context} address lies outside its data-symbol range"
        )));
    }
    let expected = format!(
        "{}::{symbol}{:+#x}",
        member.as_deref().unwrap_or("<elf>"),
        address.wrapping_sub(*symbol_address)
    );
    if canonical != &expected {
        return Err(crate::Error::invalid(format!(
            "{context} canonical identity does not match its bounded address"
        )));
    }
    Ok(())
}

fn validate_call(
    facts: &InterfaceFacts,
    call: &InterfaceCallFact,
    keys: &mut BTreeSet<InterfaceCallFact>,
) -> Result<()> {
    if facts.artifact(call.artifact).is_none() {
        return Err(crate::Error::invalid(format!(
            "interface call refers to unknown artifact {}",
            call.artifact
        )));
    }
    if call.function.is_empty() {
        return Err(crate::Error::invalid(
            "interface call has an empty function name",
        ));
    }
    if !matches!(call.kind.as_str(), "call" | "tail-jump" | "linked-jump") {
        return Err(crate::Error::invalid(format!(
            "interface call has unsupported kind {:?}",
            call.kind
        )));
    }
    if !keys.insert(call.clone()) {
        return Err(crate::Error::invalid("duplicate interface call fact"));
    }
    if call
        .root_linkage
        .candidates
        .iter()
        .any(|candidate| facts.artifact(candidate.artifact).is_none())
    {
        return Err(crate::Error::invalid(
            "interface call root linkage refers to an unknown artifact",
        ));
    }
    if call.root_linkage.resolutions.iter().any(String::is_empty) {
        return Err(crate::Error::invalid(
            "interface call root linkage has an empty resolution",
        ));
    }
    for load in &call.loads {
        if !matches!(load.width, 8 | 16 | 32 | 64) {
            return Err(crate::Error::invalid(format!(
                "interface call target load has unsupported width {}",
                load.width
            )));
        }
    }
    match call.loads.split_last() {
        None if call.container_depth != 0 || call.slot_offset.is_some() => {
            return Err(crate::Error::invalid(
                "direct interface call has inconsistent table metadata",
            ));
        }
        None => {}
        Some((slot, container)) => {
            let expected_fixed_offset = slot.selector.is_none().then_some(slot.offset);
            if call.container_depth != container.len() || call.slot_offset != expected_fixed_offset
            {
                return Err(crate::Error::invalid(
                    "interface call has inconsistent container/slot metadata",
                ));
            }
            let table = facts.tables.iter().find(|table| {
                table.artifact == call.artifact
                    && table.root == call.root
                    && table.container_path == container
            });
            let Some(table) = table else {
                return Err(crate::Error::invalid(
                    "interface call has no matching table candidate",
                ));
            };
            let table_slot = table.slots.iter().find(|candidate| {
                (candidate.offset, candidate.width, candidate.selector)
                    == (slot.offset, slot.width, slot.selector)
            });
            let Some(table_slot) = table_slot else {
                return Err(crate::Error::invalid(
                    "interface call has no matching table slot",
                ));
            };
            if !table.functions.contains(&call.function)
                || !table_slot.functions.contains(&call.function)
            {
                return Err(crate::Error::invalid(
                    "interface call is missing from its table function index",
                ));
            }
        }
    }
    for (expected, argument) in call.arguments.iter().enumerate() {
        if argument.index != expected {
            return Err(crate::Error::invalid(
                "interface call arguments must use consecutive indices",
            ));
        }
        if !matches!(
            argument.kind.as_str(),
            "unknown" | "constant" | "pointer-provenance"
        ) {
            return Err(crate::Error::invalid(format!(
                "interface call argument {} has unsupported kind {:?}",
                argument.index, argument.kind
            )));
        }
        if argument.expression.is_empty() {
            return Err(crate::Error::invalid(format!(
                "interface call argument {} has an empty expression",
                argument.index
            )));
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
        return Err(crate::Error::invalid(format!(
            "{context} has invalid lowercase SHA-256 {value:?}"
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
        validate_selector(step.selector, context)?;
    }
    Ok(())
}

fn validate_slots(slots: &[InterfaceFactSlot], context: &str) -> Result<()> {
    let mut keys = BTreeSet::new();
    for slot in slots {
        if !matches!(slot.width, 8 | 16 | 32 | 64) {
            return Err(crate::Error::invalid(format!(
                "{context} has unsupported width {}",
                slot.width
            )));
        }
        validate_selector(slot.selector, context)?;
        if !keys.insert((slot.offset, slot.width, slot.selector)) {
            return Err(crate::Error::invalid(format!(
                "{context} contains a duplicate slot"
            )));
        }
        if slot.functions.is_empty() {
            return Err(crate::Error::invalid(format!(
                "{context} contains a slot without calling functions"
            )));
        }
    }
    Ok(())
}

fn validate_selector(selector: Option<InterfaceFactSelector>, context: &str) -> Result<()> {
    if let Some(selector) = selector
        && (selector.argument >= 8 || selector.scale == 0)
    {
        return Err(crate::Error::invalid(format!(
            "{context} has invalid indexed selector {selector:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_offsets_at_different_pointer_depths_are_valid() {
        let step = InterfaceFactStep {
            offset: 0,
            width: 32,
            selector: None,
        };
        validate_steps(&[step, step], "nested pointer chain").unwrap();
    }
}
