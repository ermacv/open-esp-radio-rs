//! Container-relative identities independent of display names and placement.

use serde::{Deserialize, Serialize};

/// Physical object occurrence inside one immutable artifact.
///
/// Archive ordinals count payload members in container order. Equal member
/// names and equal bytes do not merge independently selectable occurrences.
/// The containing artifact's content identity qualifies this locator.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectLocation {
    Standalone,
    ArchiveMember { ordinal: u64 },
}

/// ELF symbol-table ownership is part of a symbol occurrence, not its name.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactSymbolTable {
    Static,
    Dynamic,
}

impl ArtifactSymbolTable {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Dynamic => "dynamic",
        }
    }
}

/// Exact symbol occurrence in an artifact; addresses and names are metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolLocation {
    pub object: ObjectLocation,
    pub table: ArtifactSymbolTable,
    pub index: u64,
}

/// Linkage of the symbol named by a captured relocation. A non-local
/// definition is a candidate until link selection/interposition is resolved.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolBinding {
    LocalDefinition,
    GlobalDefinition,
    WeakDefinition,
    Undefined,
}

/// Physical target entry of a relocation, independent of its display name.
/// This records source evidence, not a claim that a linker selected a target.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SymbolReference {
    Captured {
        artifact_sha256: String,
        location: SymbolLocation,
        binding: SymbolBinding,
    },
    Unknown {
        reason: String,
    },
}

impl SymbolReference {
    pub fn definition_identity(&self) -> Option<DataIdentity> {
        match self {
            Self::Captured {
                artifact_sha256,
                location,
                binding,
            } if *binding != SymbolBinding::Undefined => Some(DataIdentity::Symbol {
                artifact_sha256: artifact_sha256.clone(),
                location: *location,
            }),
            _ => None,
        }
    }

    pub fn is_local_definition(&self) -> bool {
        matches!(
            self,
            Self::Captured {
                binding: SymbolBinding::LocalDefinition,
                ..
            }
        )
    }
}

/// Source occurrence of a data definition or a zero-sized data anchor.
/// The inferred extent of an anchor is a separate fact, not its identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DataIdentity {
    Symbol {
        artifact_sha256: String,
        location: SymbolLocation,
    },
    Synthetic {
        namespace: String,
        key: String,
    },
}

impl DataIdentity {
    pub fn artifact_sha256(&self) -> Option<&str> {
        match self {
            Self::Symbol {
                artifact_sha256, ..
            } => Some(artifact_sha256),
            Self::Synthetic { .. } => None,
        }
    }
}

impl std::fmt::Display for DataIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Symbol {
                artifact_sha256,
                location,
            } => {
                write!(f, "{artifact_sha256}/")?;
                match location.object {
                    ObjectLocation::Standalone => f.write_str("elf")?,
                    ObjectLocation::ArchiveMember { ordinal } => write!(f, "member-{ordinal}")?,
                }
                write!(f, "/{}-{}", location.table.label(), location.index)
            }
            Self::Synthetic { namespace, key } => write!(f, "synthetic/{namespace:?}/{key:?}"),
        }
    }
}

/// Source identity of a code definition, independent of its display name.
///
/// A section range is distinct from a symbol-table entry even when they cover
/// identical bytes. Model boundaries and in-memory fixtures do not claim to be
/// captured definitions or proofs of code behavior.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CodeIdentity {
    Symbol {
        artifact_sha256: String,
        location: SymbolLocation,
    },
    SectionRange {
        artifact_sha256: String,
        object: ObjectLocation,
        section_index: u64,
        start_offset: u64,
        end_offset: u64,
    },
    ModelBoundary {
        artifact_sha256: String,
        symbol: String,
        address: u64,
    },
    Synthetic {
        namespace: String,
        key: String,
    },
}

impl CodeIdentity {
    pub fn artifact_sha256(&self) -> Option<&str> {
        match self {
            Self::Symbol {
                artifact_sha256, ..
            }
            | Self::SectionRange {
                artifact_sha256, ..
            }
            | Self::ModelBoundary {
                artifact_sha256, ..
            } => Some(artifact_sha256),
            Self::Synthetic { .. } => None,
        }
    }

    /// Exact captured object, available only for physical code definitions.
    pub fn object(&self) -> Option<(&str, ObjectLocation)> {
        match self {
            Self::Symbol {
                artifact_sha256,
                location,
            } => Some((artifact_sha256, location.object)),
            Self::SectionRange {
                artifact_sha256,
                object,
                ..
            } => Some((artifact_sha256, *object)),
            Self::ModelBoundary { .. } | Self::Synthetic { .. } => None,
        }
    }
}

impl std::fmt::Display for CodeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn object(f: &mut std::fmt::Formatter<'_>, location: ObjectLocation) -> std::fmt::Result {
            match location {
                ObjectLocation::Standalone => f.write_str("elf"),
                ObjectLocation::ArchiveMember { ordinal } => write!(f, "member-{ordinal}"),
            }
        }
        match self {
            Self::Symbol {
                artifact_sha256,
                location,
            } => {
                write!(f, "{artifact_sha256}/")?;
                object(f, location.object)?;
                write!(f, "/{}-{}", location.table.label(), location.index)
            }
            Self::SectionRange {
                artifact_sha256,
                object: location,
                section_index,
                start_offset,
                end_offset,
            } => {
                write!(f, "{artifact_sha256}/")?;
                object(f, *location)?;
                write!(
                    f,
                    "/section-{section_index}/{start_offset:x}-{end_offset:x}"
                )
            }
            Self::ModelBoundary {
                artifact_sha256,
                symbol,
                address,
            } => write!(f, "{artifact_sha256}/model/{symbol:?}/{address:x}"),
            Self::Synthetic { namespace, key } => write!(f, "synthetic/{namespace:?}/{key:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn repeated_member_names_and_symbol_addresses_cannot_merge_occurrences() {
        let location = |ordinal, table, index| SymbolLocation {
            object: ObjectLocation::ArchiveMember { ordinal },
            table,
            index,
        };
        let occurrences = BTreeSet::from([
            location(0, ArtifactSymbolTable::Static, 1),
            location(1, ArtifactSymbolTable::Static, 1),
            location(0, ArtifactSymbolTable::Dynamic, 1),
            location(0, ArtifactSymbolTable::Static, 2),
        ]);
        assert_eq!(occurrences.len(), 4);
        let encoded = serde_json::to_string(&occurrences).unwrap();
        assert_eq!(
            serde_json::from_str::<BTreeSet<SymbolLocation>>(&encoded).unwrap(),
            occurrences
        );
    }
}
