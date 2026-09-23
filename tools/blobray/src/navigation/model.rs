//! Stable navigation identity and generated document schema.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, artifact_path_sha256, parse_u32};

pub(super) const SCHEMA_VERSION: u32 = 7;
pub(super) const IDENTITY_SCHEME: &str = "physical-occurrence-v2";

/// A symbol-table entry or a code boundary that has no symbol-table entry.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum NavigationIdentity {
    Symbol {
        artifact_sha256: String,
        location: crate::SymbolLocation,
    },
    CodeBody {
        artifact_sha256: String,
        identity: crate::artifact::CodeIdentity,
    },
}

impl NavigationIdentity {
    pub(super) fn from_code(identity: crate::artifact::CodeIdentity, digest: &str) -> Self {
        match identity {
            crate::artifact::CodeIdentity::Symbol {
                artifact_sha256,
                location,
            } => Self::Symbol {
                artifact_sha256,
                location,
            },
            identity => Self::CodeBody {
                artifact_sha256: digest.to_owned(),
                identity,
            },
        }
    }

    pub(super) fn digest(&self) -> &str {
        match self {
            Self::Symbol {
                artifact_sha256, ..
            }
            | Self::CodeBody {
                artifact_sha256, ..
            } => artifact_sha256,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SymbolKey {
    pub(super) occurrence: NavigationIdentity,
    pub(super) artifact_sha256: String,
    pub(super) member: Option<String>,
    pub(super) name: String,
    pub(super) object_address: u32,
}

impl PartialEq for SymbolKey {
    fn eq(&self, other: &Self) -> bool {
        self.occurrence == other.occurrence
    }
}
impl Eq for SymbolKey {}
impl PartialOrd for SymbolKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for SymbolKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.occurrence.cmp(&other.occurrence)
    }
}

impl SymbolKey {
    pub(super) fn id(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(IDENTITY_SCHEME.as_bytes());
        digest.update([0]);
        digest.update(
            serde_json::to_vec(&self.occurrence)
                .expect("typed navigation identity is serializable"),
        );
        format!("symbol-v2:{:x}", digest.finalize())
    }

    pub(super) fn label(&self) -> SymbolLabel {
        SymbolLabel {
            member: self.member.clone(),
            name: self.name.clone(),
            object_address: format!("{:#x}", self.object_address),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SymbolLabel {
    pub(super) member: Option<String>,
    pub(super) name: String,
    pub(super) object_address: String,
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
    pub(super) owner: crate::artifact::CodeIdentity,
    pub(super) site: String,
    pub(super) kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InterfaceRootObservation {
    pub(super) owner: crate::artifact::CodeIdentity,
    pub(super) data_address: Option<open_radio_vendor_contracts::DataAddressResolution>,
    pub(super) function: String,
    pub(super) site: String,
    pub(super) kind: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SymbolDocument {
    pub(super) occurrence: NavigationIdentity,
    pub(super) labels: BTreeSet<SymbolLabel>,
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
            occurrence: key.occurrence.clone(),
            labels: BTreeSet::from([key.label()]),
            id: key.id(),
            artifact_sha256: key.artifact_sha256.clone(),
            member: key.member.clone(),
            name: key.name.clone(),
            object_address: format!("{:#x}", key.object_address),
            sources: BTreeSet::new(),
            inventory: BTreeSet::new(),
            linked_ir: BTreeSet::new(),
            interface_calls: BTreeSet::new(),
            interface_roots: BTreeSet::new(),
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
    pub(super) project_call_links: usize,
    pub(super) unique_project_calls: usize,
    pub(super) ambiguous_project_calls: usize,
    pub(super) unresolved_project_calls: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectCallLinkDocument {
    pub(super) caller: String,
    pub(super) site: Option<u32>,
    pub(super) symbol: String,
    pub(super) status: String,
    pub(super) candidates: Vec<String>,
    /// Always false. Unique source-qualified identity is a navigation fact,
    /// not proof of linker selection or transitive behavior.
    pub(super) linker_resolution_claim: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NavigationDocument {
    pub(super) interface_observations: Option<crate::interfaces::InterfaceFacts>,
    pub(super) schema_version: u32,
    pub(super) command: String,
    pub(super) identity_scheme: String,
    pub(super) semantic_claim: bool,
    pub(super) linker_resolution_claim: bool,
    pub(super) inputs: Vec<InputDocument>,
    pub(super) artifacts: Vec<ArtifactDocument>,
    pub(super) symbols: Vec<SymbolDocument>,
    pub(super) project_calls: Vec<ProjectCallLinkDocument>,
    pub(super) summary: SummaryDocument,
}

pub(super) fn input(
    kind: &'static str,
    id: String,
    path: &Path,
    navigation_output: &Path,
) -> Result<InputDocument> {
    let base = navigation_output.parent().unwrap_or_else(|| Path::new("."));
    Ok(InputDocument {
        kind: kind.to_owned(),
        id,
        path: relative_path(base, path)?.display().to_string(),
        sha256: artifact_path_sha256(path)?,
    })
}

fn relative_path(base: &Path, target: &Path) -> Result<PathBuf> {
    let base = base.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    let base_is_absolute = matches!(
        base.first(),
        Some(Component::RootDir | Component::Prefix(_))
    );
    let target_is_absolute = matches!(
        target.first(),
        Some(Component::RootDir | Component::Prefix(_))
    );
    if base_is_absolute != target_is_absolute || (base_is_absolute && common == 0) {
        return Err(crate::Error::invalid(
            "navigation index and input do not share a filesystem root",
        ));
    }
    let mut output = PathBuf::new();
    for _ in &base[common..] {
        output.push(Component::ParentDir.as_os_str());
    }
    for component in &target[common..] {
        output.push(component.as_os_str());
    }
    if output.as_os_str().is_empty() {
        return Err(crate::Error::invalid(
            "navigation input cannot be the navigation index directory",
        ));
    }
    Ok(output)
}

pub(super) fn address(value: &str, context: &str) -> Result<u32> {
    parse_u32(value)
        .ok_or_else(|| crate::Error::invalid(format!("invalid {context} address {value:?}")))
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
    let document = symbols
        .entry(key.clone())
        .or_insert_with(|| SymbolDocument::from_key(key));
    document.labels.insert(key.label());
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_id_uses_physical_occurrence_and_retains_display_alternatives() {
        let base = SymbolKey {
            occurrence: NavigationIdentity::Symbol {
                artifact_sha256: "11".repeat(32),
                location: crate::SymbolLocation {
                    object: crate::ObjectLocation::Standalone,
                    table: crate::ArtifactSymbolTable::Static,
                    index: 1,
                },
            },
            artifact_sha256: "11".repeat(32),
            member: Some("member.o".to_owned()),
            name: "function".to_owned(),
            object_address: 0x20,
        };
        assert_eq!(base.id(), base.clone().id());
        let mut changed = base.clone();
        changed.object_address += 4;
        assert_eq!(base.id(), changed.id());
        changed = base.clone();
        changed.member = Some("other.o".to_owned());
        assert_eq!(base.id(), changed.id());
        changed = base.clone();
        changed.name.push_str("_other");
        assert_eq!(base.id(), changed.id());
        let mut documents = BTreeMap::new();
        symbol(&mut documents, &base);
        symbol(&mut documents, &changed);
        assert_eq!(documents.len(), 1);
        assert_eq!(documents.values().next().unwrap().labels.len(), 2);
        if let NavigationIdentity::Symbol { location, .. } = &mut changed.occurrence {
            location.object = crate::ObjectLocation::ArchiveMember { ordinal: 1 };
        }
        assert_ne!(base.id(), changed.id());
        symbol(&mut documents, &changed);
        assert_eq!(documents.len(), 2);
    }
}
