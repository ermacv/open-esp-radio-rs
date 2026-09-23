//! Internal consistency checks for generated interface facts.

use std::collections::BTreeSet;

use crate::Result;

use super::*;

pub(super) fn validate(facts: &InterfaceFacts) -> Result<()> {
    for blocker in &facts.decode_blockers {
        validate_owner(facts, blocker.artifact, &blocker.owner)?;
        if facts.artifact(blocker.artifact).is_none() || blocker.class.is_empty() {
            return Err(crate::Error::invalid(
                "interface decode blocker has invalid source or class",
            ));
        }
    }
    for failure in &facts.analysis_failures {
        validate_owner(facts, failure.artifact, &failure.owner)?;
        if facts.artifact(failure.artifact).is_none() || failure.error.is_empty() {
            return Err(crate::Error::invalid(
                "interface analysis failure has invalid source or diagnostic",
            ));
        }
    }
    for gap in &facts.gaps {
        if facts.artifact(gap.artifact).is_none() || gap.evidence.registers.len() != 32 {
            return Err(crate::Error::invalid(
                "interface gap has invalid artifact or register snapshot",
            ));
        }
        validate_owner(facts, gap.artifact, &gap.evidence.owner)?;
        for (index, register) in gap.evidence.registers.iter().enumerate() {
            if usize::from(register.register) != index {
                return Err(crate::Error::invalid(
                    "interface gap register indices are not consecutive",
                ));
            }
            validate_argument(facts, gap.artifact, &gap.evidence.owner, &register.value)?;
        }
    }
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
        validate_root_origin(facts, table.artifact, None, &table.root)?;
        validate_data_root(&table.root, "interface table root")?;
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
        validate_owner(facts, assignment.artifact, &assignment.owner)?;
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
        validate_root_origin(
            facts,
            assignment.artifact,
            Some(&assignment.owner),
            &assignment.root,
        )?;
        validate_root_origin(
            facts,
            assignment.artifact,
            Some(&assignment.owner),
            &assignment.target,
        )?;
        validate_data_root(&assignment.root, "interface assignment root")?;
        validate_data_root(&assignment.target, "interface assignment target")?;
        if assignment.container_path.is_empty() {
            validate_access(
                &assignment.root,
                assignment.offset,
                Some(assignment.width),
                "interface assignment root",
            )?;
        }
        validate_steps(
            &assignment.target_loads,
            "interface assignment target loads",
        )?;
        let (offset, width) = assignment
            .target_loads
            .first()
            .map_or((assignment.target_offset, None), |load| {
                (load.offset, Some(load.width))
            });
        validate_access(
            &assignment.target,
            offset,
            width,
            "interface assignment target",
        )?;
        if !assignment_keys.insert(assignment.clone()) {
            return Err(crate::Error::invalid("duplicate interface assignment fact"));
        }
    }
    Ok(())
}

fn validate_owner(
    facts: &InterfaceFacts,
    artifact: usize,
    owner: &crate::artifact::CodeIdentity,
) -> Result<()> {
    let artifact = facts
        .artifact(artifact)
        .ok_or_else(|| crate::Error::invalid("interface owner references an absent artifact"))?;
    if let Some(digest) = owner.artifact_sha256() {
        validate_sha256(digest, "interface code owner")?;
        if artifact.sha256.as_deref() != Some(digest) {
            return Err(crate::Error::invalid(
                "interface code owner belongs to another artifact",
            ));
        }
    }
    Ok(())
}

fn validate_reference(
    facts: &InterfaceFacts,
    artifact: usize,
    reference: &open_radio_vendor_contracts::SymbolReference,
) -> Result<()> {
    match reference {
        open_radio_vendor_contracts::SymbolReference::Captured {
            artifact_sha256, ..
        } => {
            validate_sha256(artifact_sha256, "interface relocation reference")?;
            if facts
                .artifact(artifact)
                .and_then(|artifact| artifact.sha256.as_ref())
                != Some(artifact_sha256)
            {
                return Err(crate::Error::invalid(
                    "interface relocation reference belongs to another artifact",
                ));
            }
        }
        open_radio_vendor_contracts::SymbolReference::Unknown { reason }
            if reason.trim().is_empty() =>
        {
            return Err(crate::Error::invalid(
                "unknown interface relocation reference requires a reason",
            ));
        }
        open_radio_vendor_contracts::SymbolReference::Unknown { .. } => {}
    }
    Ok(())
}

fn validate_root_origin(
    facts: &InterfaceFacts,
    artifact: usize,
    expected_owner: Option<&crate::artifact::CodeIdentity>,
    root: &InterfaceFactRoot,
) -> Result<()> {
    match root {
        InterfaceFactRoot::RelocatedSymbol { reference, .. } => {
            validate_reference(facts, artifact, reference)?
        }
        InterfaceFactRoot::FunctionArgument { owner, argument } => {
            validate_owner(facts, artifact, owner)?;
            if *argument >= 8 || expected_owner.is_some_and(|expected| expected != owner) {
                return Err(crate::Error::invalid(
                    "interface argument root has invalid index or belongs to another function",
                ));
            }
        }
        InterfaceFactRoot::AbsoluteAddress { .. } => {}
    }
    Ok(())
}

fn validate_data_root(root: &InterfaceFactRoot, context: &str) -> Result<()> {
    if let InterfaceFactRoot::AbsoluteAddress { data_address, .. } = root {
        data_address
            .validate()
            .map_err(|error| crate::Error::invalid(format!("{context}: {error}")))?;
    }
    Ok(())
}

fn validate_access(
    root: &InterfaceFactRoot,
    offset: i32,
    width: Option<u8>,
    context: &str,
) -> Result<()> {
    if let InterfaceFactRoot::AbsoluteAddress {
        address,
        data_address,
    } = root
        && let Some((observed, observed_width)) = data_address.access()
        && (address.checked_add_signed(offset) != Some(observed) || observed_width != width)
    {
        return Err(crate::Error::invalid(format!(
            "{context}: data-address evidence does not match the observed access"
        )));
    }
    Ok(())
}

fn validate_call(
    facts: &InterfaceFacts,
    call: &InterfaceCallFact,
    keys: &mut BTreeSet<InterfaceCallFact>,
) -> Result<()> {
    validate_owner(facts, call.artifact, &call.owner)?;
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
    if call.link_register >= 32
        || match call.kind.as_str() {
            "call" => call.link_register != 1,
            "tail-jump" => call.link_register != 0,
            "linked-jump" => call.link_register <= 1,
            _ => true,
        }
    {
        return Err(crate::Error::invalid(
            "interface call kind disagrees with link register",
        ));
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
    validate_root_origin(facts, call.artifact, Some(&call.owner), &call.root)?;
    validate_data_root(&call.root, "interface call root")?;
    if let Some(load) = call.loads.first() {
        validate_access(
            &call.root,
            load.offset,
            Some(load.width),
            "interface call root",
        )?;
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
                    && crate::interfaces::facts::same_step_shape(&table.container_path, container)
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
        validate_argument(facts, call.artifact, &call.owner, &argument.value)?;
    }
    Ok(())
}

fn validate_argument(
    facts: &InterfaceFacts,
    artifact: usize,
    owner: &crate::artifact::CodeIdentity,
    value: &crate::interface_discovery::InterfaceArgumentValue,
) -> Result<()> {
    use crate::interface_discovery::{InterfaceArgumentValue as V, InterfaceRoot};
    match value {
        V::Alternatives(values) => {
            if values.len() < 2
                || values.iter().collect::<BTreeSet<_>>().len() != values.len()
                || values.iter().any(|v| matches!(v, V::Alternatives(_)))
            {
                return Err(crate::Error::invalid(
                    "interface alternatives must contain distinct atomic values",
                ));
            }
            for value in values {
                validate_argument(facts, artifact, owner, value)?;
            }
        }
        V::Selector(selector) => validate_selector(
            Some(InterfaceFactSelector {
                argument: selector.argument,
                scale: selector.scale,
                addend: selector.addend,
            }),
            "interface argument selector",
        )?,
        V::Pointer(pointer) | V::GotAddress(pointer) | V::IndexedPointer { pointer, .. } => {
            match &pointer.root {
                InterfaceRoot::RelocatedSymbol { reference, .. } => {
                    validate_reference(facts, artifact, reference)?
                }
                InterfaceRoot::FunctionArgument {
                    owner: root_owner, ..
                } => {
                    validate_owner(facts, artifact, root_owner)?;
                    if root_owner != owner {
                        return Err(crate::Error::invalid(
                            "interface argument root belongs to another function",
                        ));
                    }
                }
                InterfaceRoot::AbsoluteAddress { .. } => {}
            }
            if let InterfaceRoot::AbsoluteAddress { data_address, .. } = &pointer.root {
                data_address.validate().map_err(crate::Error::invalid)?;
            }
            if let InterfaceRoot::FunctionArgument { index, .. } = &pointer.root
                && *index >= 8
            {
                return Err(crate::Error::invalid("invalid interface argument root"));
            }
            for load in &pointer.loads {
                validate_steps(
                    &[InterfaceFactStep {
                        site: Some(load.site),
                        offset: load.offset,
                        width: load.width,
                        selector: load.selector.as_ref().map(|s| InterfaceFactSelector {
                            argument: s.argument,
                            scale: s.scale,
                            addend: s.addend,
                        }),
                    }],
                    "interface argument load",
                )?;
            }
            if let V::IndexedPointer { selector, .. } = value {
                validate_argument(facts, artifact, owner, &V::Selector(selector.clone()))?;
            }
        }
        V::Unknown | V::Constant(_) => {}
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
            site: None,
            offset: 0,
            width: 32,
            selector: None,
        };
        validate_steps(&[step, step], "nested pointer chain").unwrap();
    }
}
