//! Canonical artifact-local locators and revision occurrence identities.
//!
//! Raw vendor names remain part of the exact occurrence provenance even when
//! reviewed knowledge later assigns a stable semantic identity.  Producers
//! and consumers must use these helpers so a reviewed binding cannot silently
//! miss because two analysis paths formatted the same artifact location
//! differently.

use open_radio_vendor_contracts::{
    ArtifactIdentity, EntityDomain, RevisionOccurrenceId, SemanticEntityId,
};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactOccurrence {
    pub(crate) locator: String,
    pub(crate) id: RevisionOccurrenceId,
}

pub(crate) fn derive(
    domain: EntityDomain,
    source: &str,
    artifact_sha256: &str,
    locator: &str,
) -> Result<RevisionOccurrenceId> {
    let artifact = ArtifactIdentity::new(source, artifact_sha256)
        .map_err(|error| Error::invalid(error.to_string()))?;
    RevisionOccurrenceId::derive(domain, &[artifact], locator)
        .map_err(|error| Error::invalid(error.to_string()))
}

pub(crate) fn validate(
    domain: EntityDomain,
    source: &str,
    artifact_sha256: &str,
    locator: &str,
    occurrence: &str,
    semantic: Option<&str>,
) -> Result<()> {
    let persisted = occurrence
        .parse::<RevisionOccurrenceId>()
        .map_err(|error| Error::invalid(error.to_string()))?;
    let expected = derive(domain, source, artifact_sha256, locator)?;
    if persisted != expected {
        return Err(Error::invalid(format!(
            "{domain} occurrence {persisted} does not match exact artifact locator {source}@{artifact_sha256}:{locator}"
        )));
    }
    if let Some(semantic) = semantic {
        let semantic = semantic
            .parse::<SemanticEntityId>()
            .map_err(|error| Error::invalid(error.to_string()))?;
        if semantic.domain() != domain {
            return Err(Error::invalid(format!(
                "{domain} occurrence {persisted} has reviewed semantic identity from {} domain",
                semantic.domain()
            )));
        }
    }
    Ok(())
}

/// Validate physical coordinates as well as the derived revision occurrence.
pub(crate) fn validate_function(
    identity: &crate::artifact::CodeIdentity,
    source: &str,
    artifact_sha256: &str,
    locator: &str,
    occurrence: &str,
    semantic: Option<&str>,
) -> Result<()> {
    if identity
        .artifact_sha256()
        .is_some_and(|digest| digest != artifact_sha256)
        || function_locator(identity) != locator
    {
        return Err(Error::invalid(
            "function has inconsistent physical identity",
        ));
    }
    validate(
        EntityDomain::Function,
        source,
        artifact_sha256,
        locator,
        occurrence,
        semantic,
    )
}

pub(crate) fn function_locator(identity: &crate::artifact::CodeIdentity) -> String {
    identity.to_string()
}

pub(crate) fn function_occurrence(
    artifact: &ArtifactIdentity,
    identity: &crate::artifact::CodeIdentity,
) -> Result<ArtifactOccurrence> {
    if identity
        .artifact_sha256()
        .is_some_and(|digest| digest != artifact.sha256())
    {
        return Err(Error::invalid(
            "function physical identity belongs to another artifact",
        ));
    }
    occurrence(EntityDomain::Function, artifact, function_locator(identity))
}

pub(crate) fn memory_object_locator(identity: &crate::artifact::DataIdentity) -> String {
    format!("data:{identity}")
}

pub(crate) fn memory_object_occurrence(
    artifact: &ArtifactIdentity,
    identity: &crate::artifact::DataIdentity,
) -> Result<ArtifactOccurrence> {
    if identity
        .artifact_sha256()
        .is_some_and(|digest| digest != artifact.sha256())
    {
        return Err(Error::invalid(
            "data physical identity belongs to another artifact",
        ));
    }
    occurrence(
        EntityDomain::MemoryObject,
        artifact,
        memory_object_locator(identity),
    )
}

pub(crate) fn validate_data(
    identity: &crate::artifact::DataIdentity,
    source: &str,
    digest: &str,
    locator: &str,
    occurrence: &str,
    semantic: Option<&str>,
) -> Result<()> {
    if identity
        .artifact_sha256()
        .is_some_and(|actual| actual != digest)
        || memory_object_locator(identity) != locator
    {
        return Err(Error::invalid(
            "data object has inconsistent physical identity",
        ));
    }
    validate(
        EntityDomain::MemoryObject,
        source,
        digest,
        locator,
        occurrence,
        semantic,
    )
}

fn occurrence(
    domain: EntityDomain,
    artifact: &ArtifactIdentity,
    locator: String,
) -> Result<ArtifactOccurrence> {
    let id = RevisionOccurrenceId::derive(domain, std::slice::from_ref(artifact), &locator)
        .map_err(|error| Error::invalid(error.to_string()))?;
    Ok(ArtifactOccurrence { locator, id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact() -> ArtifactIdentity {
        ArtifactIdentity::new("vendor/ble", "a".repeat(64)).unwrap()
    }

    #[test]
    fn function_occurrences_preserve_physical_positions_without_names() {
        let identity = |ordinal| crate::artifact::CodeIdentity::Symbol {
            artifact_sha256: "a".repeat(64),
            location: crate::SymbolLocation {
                object: crate::ObjectLocation::ArchiveMember { ordinal },
                table: crate::ArtifactSymbolTable::Static,
                index: 1,
            },
        };
        let first = function_occurrence(&artifact(), &identity(0)).unwrap();
        let second = function_occurrence(&artifact(), &identity(1)).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.locator, second.locator);
        assert_eq!(first.id.domain(), EntityDomain::Function);
        validate(
            EntityDomain::Function,
            "vendor/ble",
            &"a".repeat(64),
            &first.locator,
            &first.id.to_string(),
            None,
        )
        .unwrap();
    }

    #[test]
    fn data_occurrences_distinguish_repeated_members_and_reject_wrong_artifact() {
        let identity = |ordinal| crate::artifact::DataIdentity::Symbol {
            artifact_sha256: "a".repeat(64),
            location: open_radio_vendor_contracts::SymbolLocation {
                object: open_radio_vendor_contracts::ObjectLocation::ArchiveMember { ordinal },
                table: open_radio_vendor_contracts::ArtifactSymbolTable::Static,
                index: 3,
            },
        };
        let first = memory_object_occurrence(&artifact(), &identity(0)).unwrap();
        let second = memory_object_occurrence(&artifact(), &identity(1)).unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.locator, second.locator);
        assert!(
            validate_data(
                &identity(1),
                "vendor/ble",
                &"a".repeat(64),
                &first.locator,
                &first.id.to_string(),
                None
            )
            .is_err()
        );
        assert!(
            validate_data(
                &identity(0),
                "vendor/ble",
                &"b".repeat(64),
                &first.locator,
                &first.id.to_string(),
                None
            )
            .is_err()
        );
    }
}
