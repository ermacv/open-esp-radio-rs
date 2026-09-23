//! Revision comparison over the common register graph, with durable evidence
//! digests instead of copying captured input payloads into revision files.

use super::*;
use crate::application::register_inventory::RegisterInventory;
use open_radio_vendor_contracts::register_inventory::RegisterSubject;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceDigest {
    id: String,
    kind: String,
    sources: BTreeSet<String>,
    /// Hash of the complete query evidence record, including its payload.
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionRegisters {
    pub(crate) inventory: RegisterInventory<EvidenceDigest>,
    fingerprints: BTreeMap<String, String>,
    context_fingerprint: Option<String>,
}

impl RevisionRegisters {
    pub(super) fn capture(inventory: RegisterInventory) -> Result<Self> {
        inventory.validate()?;
        let evidence = inventory
            .evidence
            .into_iter()
            .map(|(id, evidence)| {
                let sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&evidence)?));
                Ok((
                    id,
                    EvidenceDigest {
                        id: evidence.id,
                        kind: evidence.kind,
                        sources: evidence.sources,
                        sha256,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let inventory = RegisterInventory {
            schema_version: inventory.schema_version,
            sources: inventory.sources,
            registers: inventory.registers,
            evidence,
            gaps: inventory.gaps,
            regions: inventory.regions,
            address_domains: inventory.address_domains,
        };
        let mut captured = Self {
            inventory,
            fingerprints: BTreeMap::new(),
            context_fingerprint: None,
        };
        captured.fingerprints = captured.compute_fingerprints()?;
        captured.context_fingerprint = captured.compute_context_fingerprint()?;
        captured.validate()?;
        Ok(captured)
    }

    fn compute_fingerprints(&self) -> Result<BTreeMap<String, String>> {
        self.inventory
            .registers
            .iter()
            .map(|(id, register)| {
                let evidence = register
                    .evidence_ids()
                    .into_iter()
                    .map(|id| {
                        self.inventory.evidence.get(id).ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "revision register has dangling evidence {id:?}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok((id.clone(), fingerprint(&(register, evidence))?))
            })
            .collect()
    }

    fn compute_context_fingerprint(&self) -> Result<Option<String>> {
        let inventory = &self.inventory;
        if inventory.sources.is_empty()
            && inventory.evidence.is_empty()
            && inventory.gaps.is_empty()
            && inventory.regions.is_empty()
            && inventory.address_domains.is_empty()
        {
            return Ok(None);
        }
        fingerprint(&(
            &inventory.sources,
            &inventory.evidence,
            &inventory.gaps,
            &inventory.regions,
            &inventory.address_domains,
        ))
        .map(Some)
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.inventory.validate()?;
        for (id, evidence) in &self.inventory.evidence {
            if id != &evidence.id {
                return Err(crate::Error::invalid("revision evidence identity mismatch"));
            }
            validate_sha256("register evidence", &evidence.sha256)?;
        }
        if self.fingerprints != self.compute_fingerprints()?
            || self.context_fingerprint != self.compute_context_fingerprint()?
        {
            return Err(crate::Error::invalid(
                "revision register graph fingerprint does not match",
            ));
        }
        Ok(())
    }

    pub(super) fn entities(&self) -> impl Iterator<Item = EntityView<'_>> {
        self.fingerprints
            .iter()
            .map(|(id, fingerprint)| EntityView { id, fingerprint })
    }

    pub(super) fn context_entity(&self) -> impl Iterator<Item = EntityView<'_>> {
        self.context_fingerprint
            .as_deref()
            .map(|fingerprint| EntityView {
                id: "register-inventory-context",
                fingerprint,
            })
            .into_iter()
    }

    /// A width-free physical identity does not authorize a reviewed assertion
    /// for an incompatible, conflicted or unknown register geometry.
    pub(super) fn supports_semantic(&self, semantic: &SemanticEntityId) -> bool {
        let width = match semantic {
            SemanticEntityId::Register { width, .. } => *width,
            SemanticEntityId::RegisterField { register_width, .. } => *register_width,
            _ => return true,
        };
        self.inventory
            .registers
            .get(&rebase_subject_base(semantic))
            .is_some_and(|register| register.width() == Some(width))
    }
}

pub(super) fn semantic_location(subject: &SemanticEntityId) -> Option<String> {
    match subject {
        SemanticEntityId::Register {
            chip,
            address_space,
            address,
            ..
        }
        | SemanticEntityId::RegisterField {
            chip,
            address_space,
            address,
            ..
        } => Some(
            RegisterSubject {
                chip: chip.clone(),
                address_space: address_space.clone(),
                route: "mmio".to_owned(),
                bank: None,
                address: *address,
            }
            .id(),
        ),
        _ => None,
    }
}
