//! Project-level symbol facts without pretending to perform a linker pass.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use crate::{Result, artifact};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkageArtifact {
    pub(crate) path: PathBuf,
    pub(crate) roles: Vec<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) container: artifact::ArtifactContainerKind,
    pub(crate) objects: usize,
    pub(crate) skipped_members: usize,
    pub(crate) code_sections: Vec<LinkageCodeSection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkageCodeSection {
    pub(crate) member: Option<String>,
    pub(crate) object_kind: artifact::ArtifactObjectKind,
    pub(crate) coverage: artifact::ArtifactCodeSectionCoverage,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkageSymbolLocation {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) address: u64,
    pub(crate) kind: artifact::ArtifactSymbolKind,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum LinkageResolution {
    DefinedLocal,
    DefinedExported,
    Absolute,
    Common,
    SameArtifactCandidate,
    ArchiveCandidate,
    ProjectAssociated,
    AmbiguousProject,
    UndefinedImport,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum LinkUnitOriginAssociation {
    NotApplicable,
    Missing,
    UniqueNameAndKind,
    AmbiguousNameAndKind,
}

impl LinkUnitOriginAssociation {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NotApplicable => "not-applicable",
            Self::Missing => "missing",
            Self::UniqueNameAndKind => "unique-name-and-kind",
            Self::AmbiguousNameAndKind => "ambiguous-name-and-kind",
        }
    }
}

impl LinkageResolution {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::DefinedLocal => "defined-local",
            Self::DefinedExported => "defined-exported",
            Self::Absolute => "absolute",
            Self::Common => "common",
            Self::SameArtifactCandidate => "same-artifact-candidate",
            Self::ArchiveCandidate => "archive-candidate",
            Self::ProjectAssociated => "project-associated",
            Self::AmbiguousProject => "ambiguous-project",
            Self::UndefinedImport => "undefined-import",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) const fn is_unresolved(self) -> bool {
        matches!(
            self,
            Self::SameArtifactCandidate
                | Self::ArchiveCandidate
                | Self::ProjectAssociated
                | Self::AmbiguousProject
                | Self::UndefinedImport
                | Self::Unknown
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkageSymbol {
    pub(crate) artifact: usize,
    pub(crate) member: Option<String>,
    pub(crate) object_kind: artifact::ArtifactObjectKind,
    pub(crate) fact: artifact::ArtifactSymbolFact,
    pub(crate) resolution: LinkageResolution,
    pub(crate) candidates: Vec<LinkageSymbolLocation>,
    /// Archive definitions associated with an authoritative linked-ELF symbol
    /// by reviewed project source, exact name and symbol kind. This is origin
    /// navigation evidence, not a claim about linker extraction or selection.
    pub(crate) origin_association: LinkUnitOriginAssociation,
    pub(crate) origin_candidates: Vec<LinkageSymbolLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectLinkageInventory {
    pub(crate) artifacts: Vec<LinkageArtifact>,
    pub(crate) symbols: Vec<LinkageSymbol>,
}

pub(crate) fn source_id(role: &str) -> String {
    if let Some((_, source)) = role.split_once(':') {
        return source.to_owned();
    }
    role.strip_suffix("-artifact")
        .or_else(|| role.strip_suffix("-inventory"))
        .or_else(|| role.strip_suffix("-companion"))
        .unwrap_or(role)
        .to_owned()
}

fn role_source<'a>(role: &'a str, prefix: &str) -> Option<&'a str> {
    role.strip_prefix(prefix)
        .filter(|source| !source.is_empty())
}

fn origin_association(candidate_count: usize, applicable: bool) -> LinkUnitOriginAssociation {
    if !applicable {
        LinkUnitOriginAssociation::NotApplicable
    } else {
        match candidate_count {
            0 => LinkUnitOriginAssociation::Missing,
            1 => LinkUnitOriginAssociation::UniqueNameAndKind,
            _ => LinkUnitOriginAssociation::AmbiguousNameAndKind,
        }
    }
}

fn definition_resolution(fact: &artifact::ArtifactSymbolFact) -> LinkageResolution {
    match fact.definition {
        artifact::ArtifactSymbolDefinitionState::Absolute => LinkageResolution::Absolute,
        artifact::ArtifactSymbolDefinitionState::Common => LinkageResolution::Common,
        artifact::ArtifactSymbolDefinitionState::Section => {
            if fact.is_exported_definition() {
                LinkageResolution::DefinedExported
            } else {
                LinkageResolution::DefinedLocal
            }
        }
        artifact::ArtifactSymbolDefinitionState::Undefined
        | artifact::ArtifactSymbolDefinitionState::None
        | artifact::ArtifactSymbolDefinitionState::Unknown => LinkageResolution::Unknown,
    }
}

fn undefined_resolution(
    container: artifact::ArtifactContainerKind,
    artifact_index: usize,
    candidates: &[LinkageSymbolLocation],
) -> LinkageResolution {
    if candidates.is_empty() {
        LinkageResolution::UndefinedImport
    } else if candidates
        .iter()
        .all(|candidate| candidate.artifact == artifact_index)
    {
        if container == artifact::ArtifactContainerKind::Archive {
            LinkageResolution::ArchiveCandidate
        } else {
            LinkageResolution::SameArtifactCandidate
        }
    } else if candidates.len() == 1 {
        LinkageResolution::ProjectAssociated
    } else {
        LinkageResolution::AmbiguousProject
    }
}

pub(crate) fn build_project_linkage_inventory(
    inputs: &[(String, PathBuf)],
) -> Result<ProjectLinkageInventory> {
    let mut grouped = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    for (role, path) in inputs {
        grouped
            .entry(path.clone())
            .or_default()
            .insert(role.clone());
    }

    let mut artifacts = Vec::new();
    let mut inventories = Vec::new();
    for (path, roles) in grouped {
        let inventory = artifact::inspect_artifact(&path)?;
        let roles = roles.into_iter().collect::<Vec<_>>();
        let sources = roles
            .iter()
            .map(|role| source_id(role))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        artifacts.push(LinkageArtifact {
            path,
            roles,
            sources,
            container: inventory.container,
            objects: inventory.objects.len(),
            skipped_members: inventory.skipped_members,
            code_sections: inventory
                .objects
                .iter()
                .flat_map(|object| {
                    object
                        .code_sections
                        .iter()
                        .cloned()
                        .map(|coverage| LinkageCodeSection {
                            member: object.member.clone(),
                            object_kind: object.kind,
                            coverage,
                        })
                })
                .collect(),
        });
        inventories.push(inventory);
    }

    let mut definitions = BTreeMap::<String, BTreeSet<LinkageSymbolLocation>>::new();
    for (artifact_index, inventory) in inventories.iter().enumerate() {
        for (object, fact) in inventory.symbols() {
            if !fact.is_exported_definition() {
                continue;
            }
            definitions
                .entry(fact.name.clone())
                .or_default()
                .insert(LinkageSymbolLocation {
                    artifact: artifact_index,
                    member: object.member.clone(),
                    address: fact.address,
                    kind: fact.kind,
                });
        }
    }

    let authoritative_sources = artifacts
        .iter()
        .map(|artifact| {
            artifact
                .roles
                .iter()
                .filter_map(|role| role_source(role, "source-artifact:"))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let inventory_sources = artifacts
        .iter()
        .map(|artifact| {
            artifact
                .roles
                .iter()
                .filter_map(|role| role_source(role, "source-inventory:"))
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let mut origin_definitions = BTreeMap::<
        (String, String, artifact::ArtifactSymbolKind),
        BTreeSet<LinkageSymbolLocation>,
    >::new();
    for (artifact_index, inventory) in inventories.iter().enumerate() {
        if inventory.container != artifact::ArtifactContainerKind::Archive {
            continue;
        }
        for (object, fact) in inventory.symbols() {
            if fact.name.is_empty()
                || !fact.is_exported_definition()
                || fact.kind != artifact::ArtifactSymbolKind::Text
            {
                continue;
            }
            for source in &inventory_sources[artifact_index] {
                origin_definitions
                    .entry((source.clone(), fact.name.clone(), fact.kind))
                    .or_default()
                    .insert(LinkageSymbolLocation {
                        artifact: artifact_index,
                        member: object.member.clone(),
                        address: fact.address,
                        kind: fact.kind,
                    });
            }
        }
    }

    let mut symbols = Vec::new();
    for (artifact_index, inventory) in inventories.iter().enumerate() {
        for (object, fact) in inventory.symbols() {
            let candidates =
                if fact.definition == artifact::ArtifactSymbolDefinitionState::Undefined {
                    definitions
                        .get(&fact.name)
                        .map(|locations| locations.iter().cloned().collect())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
            let resolution =
                if fact.definition != artifact::ArtifactSymbolDefinitionState::Undefined {
                    definition_resolution(fact)
                } else {
                    undefined_resolution(inventory.container, artifact_index, &candidates)
                };
            let origin_applicable = inventory.container != artifact::ArtifactContainerKind::Archive
                && !authoritative_sources[artifact_index].is_empty()
                && !fact.name.is_empty()
                && fact.is_exported_definition()
                && fact.kind == artifact::ArtifactSymbolKind::Text;
            let origin_candidates = if origin_applicable {
                authoritative_sources[artifact_index]
                    .iter()
                    .flat_map(|source| {
                        origin_definitions
                            .get(&(source.clone(), fact.name.clone(), fact.kind))
                            .into_iter()
                            .flatten()
                            .cloned()
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            symbols.push(LinkageSymbol {
                artifact: artifact_index,
                member: object.member.clone(),
                object_kind: object.kind,
                fact: fact.clone(),
                resolution,
                candidates,
                origin_association: origin_association(origin_candidates.len(), origin_applicable),
                origin_candidates,
            });
        }
    }
    symbols.sort_by(|left, right| {
        (
            left.artifact,
            &left.member,
            &left.fact.name,
            left.fact.table,
            left.fact.definition,
            left.fact.address,
        )
            .cmp(&(
                right.artifact,
                &right.member,
                &right.fact.name,
                right.fact.table,
                right.fact.definition,
                right.fact.address,
            ))
    });
    Ok(ProjectLinkageInventory { artifacts, symbols })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(artifact: usize) -> LinkageSymbolLocation {
        LinkageSymbolLocation {
            artifact,
            member: Some("member.o".to_owned()),
            address: 0,
            kind: artifact::ArtifactSymbolKind::Text,
        }
    }

    #[test]
    fn unique_cross_artifact_definition_is_navigation_only() {
        assert_eq!(
            undefined_resolution(artifact::ArtifactContainerKind::Elf32, 0, &[location(1)]),
            LinkageResolution::ProjectAssociated
        );
    }

    #[test]
    fn archive_member_candidate_is_not_reported_as_linker_resolution() {
        assert_eq!(
            undefined_resolution(artifact::ArtifactContainerKind::Archive, 0, &[location(0)]),
            LinkageResolution::ArchiveCandidate
        );
    }

    #[test]
    fn multiple_project_definitions_remain_ambiguous() {
        assert_eq!(
            undefined_resolution(
                artifact::ArtifactContainerKind::Elf32,
                0,
                &[location(1), location(2)]
            ),
            LinkageResolution::AmbiguousProject
        );
    }

    #[test]
    fn link_unit_origin_association_never_claims_linker_selection() {
        assert_eq!(
            origin_association(0, false),
            LinkUnitOriginAssociation::NotApplicable
        );
        assert_eq!(
            origin_association(0, true),
            LinkUnitOriginAssociation::Missing
        );
        assert_eq!(
            origin_association(1, true),
            LinkUnitOriginAssociation::UniqueNameAndKind
        );
        assert_eq!(
            origin_association(2, true),
            LinkUnitOriginAssociation::AmbiguousNameAndKind
        );
    }
}
