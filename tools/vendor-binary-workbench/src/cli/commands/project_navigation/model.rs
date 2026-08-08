//! Stable navigation identity and generated document schema.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Result;
use crate::{artifact_sha256, parse_u32};

pub(super) const SCHEMA_VERSION: u32 = 1;
pub(super) const IDENTITY_SCHEME: &str = "artifact-sha256-member-symbol-object-address-v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SymbolKey {
    pub(super) artifact_sha256: String,
    pub(super) member: Option<String>,
    pub(super) name: String,
    pub(super) object_address: u32,
}

impl SymbolKey {
    pub(super) fn id(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(IDENTITY_SCHEME.as_bytes());
        digest.update([0]);
        digest.update(self.artifact_sha256.as_bytes());
        digest.update([0]);
        digest.update(self.member.as_deref().unwrap_or("").as_bytes());
        digest.update([0]);
        digest.update(self.name.as_bytes());
        digest.update([0]);
        digest.update(self.object_address.to_le_bytes());
        format!("symbol-v1:{:x}", digest.finalize())
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputDocument {
    pub(super) kind: String,
    pub(super) id: String,
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactDocument {
    pub(super) sha256: String,
    pub(super) paths: BTreeSet<String>,
    pub(super) sources: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InventoryObservation {
    pub(super) table: String,
    pub(super) definition: String,
    pub(super) kind: String,
    pub(super) resolution: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IrObservation {
    pub(super) profile: String,
    pub(super) identity: String,
    pub(super) selection: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InterfaceCallObservation {
    pub(super) site: String,
    pub(super) kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InterfaceRootObservation {
    pub(super) function: String,
    pub(super) site: String,
    pub(super) kind: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SymbolDocument {
    pub(super) id: String,
    pub(super) artifact_sha256: String,
    pub(super) member: Option<String>,
    pub(super) name: String,
    pub(super) object_address: String,
    pub(super) sources: BTreeSet<String>,
    pub(super) inventory: BTreeSet<InventoryObservation>,
    pub(super) linked_ir: BTreeSet<IrObservation>,
    pub(super) interface_calls: BTreeSet<InterfaceCallObservation>,
    pub(super) interface_roots: BTreeSet<InterfaceRootObservation>,
}

impl SymbolDocument {
    pub(super) fn from_key(key: &SymbolKey) -> Self {
        Self {
            id: key.id(),
            artifact_sha256: key.artifact_sha256.clone(),
            member: key.member.clone(),
            name: key.name.clone(),
            object_address: format!("{:#x}", key.object_address),
            ..Self::default()
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SummaryDocument {
    pub(super) artifacts: usize,
    pub(super) symbols: usize,
    pub(super) inventory_symbols: usize,
    pub(super) linked_ir_functions: usize,
    pub(super) interface_callers: usize,
    pub(super) interface_roots: usize,
    pub(super) unmatched_interface_roots: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NavigationDocument {
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) identity_scheme: String,
    pub(super) semantic_claim: bool,
    pub(super) linker_resolution_claim: bool,
    pub(super) inputs: Vec<InputDocument>,
    pub(super) artifacts: Vec<ArtifactDocument>,
    pub(super) symbols: Vec<SymbolDocument>,
    pub(super) summary: SummaryDocument,
}

pub(super) fn input(kind: &'static str, id: String, path: &Path) -> Result<InputDocument> {
    Ok(InputDocument {
        kind: kind.to_owned(),
        id,
        path: path.display().to_string(),
        sha256: artifact_sha256(path)?,
    })
}

pub(super) fn address(value: &str, context: &str) -> Result<u32> {
    parse_u32(value).ok_or_else(|| format!("invalid {context} address {value:?}").into())
}

pub(super) fn artifact<'a>(
    artifacts: &'a mut BTreeMap<String, ArtifactDocument>,
    sha256: &str,
) -> &'a mut ArtifactDocument {
    artifacts
        .entry(sha256.to_owned())
        .or_insert_with(|| ArtifactDocument {
            sha256: sha256.to_owned(),
            ..ArtifactDocument::default()
        })
}

pub(super) fn symbol<'a>(
    symbols: &'a mut BTreeMap<SymbolKey, SymbolDocument>,
    key: &SymbolKey,
) -> &'a mut SymbolDocument {
    symbols
        .entry(key.clone())
        .or_insert_with(|| SymbolDocument::from_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_id_is_stable_and_uses_every_location_dimension() {
        let base = SymbolKey {
            artifact_sha256: "11".repeat(32),
            member: Some("member.o".to_owned()),
            name: "function".to_owned(),
            object_address: 0x20,
        };
        assert_eq!(base.id(), base.clone().id());
        let mut changed = base.clone();
        changed.object_address += 4;
        assert_ne!(base.id(), changed.id());
        changed = base.clone();
        changed.member = Some("other.o".to_owned());
        assert_ne!(base.id(), changed.id());
        changed = base.clone();
        changed.name.push_str("_other");
        assert_ne!(base.id(), changed.id());
    }
}
