use crate::*;
use std::collections::BTreeSet;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

impl Revision {
    /// Validate schema-1 identity and capture relationships without filesystem
    /// access. Raw malformed ELF references remain inventory diagnostics; they
    /// must not be confused with broken manifest ownership relationships.
    pub fn validate(&self) -> Result<()> {
        self.validate_controlled(&mut || Ok(()))
    }
    pub fn validate_controlled(&self, control: &mut dyn RunControl) -> Result<()> {
        control.phase(RunPhase::ValidateRevision)?;
        if self.schema != 1 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported revision schema",
            ));
        }
        if self.inputs.is_empty() || self.inventory_producer.is_empty() {
            return Err(invalid(
                "revision requires inputs and an inventory producer",
            ));
        }
        for (index, input) in self.inputs.iter().enumerate() {
            control.set_position(RunPosition {
                phase: RunPhase::ValidateRevision,
                input: Some(index as u64),
                ..RunPosition::default()
            });
            control.checkpoint(1)?;
            if input.role.trim().is_empty() {
                return Err(invalid("input role is empty"));
            }
            if let (Some(expected), Some(actual)) = (&input.expected, input.capture.artifact())
                && expected != actual
            {
                return Err(invalid("captured input differs from expected identity"));
            }
            match (&input.capture, &input.inventory) {
                (Capture::Unavailable { .. }, None) if input.external_members.is_empty() => {
                    continue;
                }
                (Capture::Captured { artifact, .. }, Some(inventory)) => {
                    validate_inventory(artifact, inventory, &input.external_members, control)?
                }
                _ => return Err(invalid("input capture and inventory availability disagree")),
            }
        }
        Ok(())
    }
}

fn validate_inventory(
    artifact: &ArtifactId,
    inventory: &ArtifactInventory,
    external: &[ExternalMember],
    control: &mut dyn RunControl,
) -> Result<()> {
    let archive = matches!(
        inventory.kind,
        ContainerKind::Archive | ContainerKind::ThinArchive
    );
    if !archive && inventory.objects.len() != 1 {
        return Err(invalid("standalone input requires one object outcome"));
    }
    if inventory.kind != ContainerKind::ThinArchive && !external.is_empty() {
        return Err(invalid("only thin archives bind external payloads"));
    }
    if inventory.kind == ContainerKind::ThinArchive && external.len() != inventory.objects.len() {
        return Err(invalid("thin member bindings do not cover known objects"));
    }
    for (ordinal, object) in inventory.objects.iter().enumerate() {
        let mut position = control.position();
        position.member = Some(ordinal as u64);
        position.table = None;
        position.entry = None;
        position.artifact(artifact);
        control.set_position(position);
        control.checkpoint(1)?;
        if let Some(name) = &object.name {
            for chunk in name.chunks(WORK_BLOCK) {
                control.bytes(chunk.len())?;
            }
        }
        let expected = if archive {
            ObjectLocation::ArchiveMember {
                ordinal: ordinal as u64,
            }
        } else {
            ObjectLocation::Standalone
        };
        if object.id.artifact != *artifact || object.id.location != expected {
            return Err(invalid(
                "object identity does not belong to its ordered input container",
            ));
        }
        if !archive && object.content.as_ref() != Some(artifact) {
            return Err(invalid(
                "standalone object content differs from its input capture",
            ));
        }
        if object.content.is_none() && object.elf.is_some() {
            return Err(invalid("ELF inventory has no captured content"));
        }
        if let Some(member) = external.get(ordinal)
            && (member.ordinal != ordinal as u64
                || object.name.as_ref() != Some(&member.name)
                || member.capture.artifact() != object.content.as_ref())
        {
            return Err(invalid(
                "thin member occurrence and captured object disagree",
            ));
        }
        if let Some(elf) = &object.elf {
            let mut symbols = BTreeSet::new();
            for symbol in &elf.symbols {
                let mut position = control.position();
                position.table = Some(u64::from(symbol.id.table_section));
                position.entry = Some(symbol.id.index);
                control.set_position(position);
                control.checkpoint(1)?;
                if symbol.id.object != object.id
                    || !symbols.insert((symbol.id.table_section, symbol.id.index))
                {
                    return Err(invalid(
                        "symbol identity has wrong ownership or duplicates an entry",
                    ));
                }
            }
        }
    }
    Ok(())
}
