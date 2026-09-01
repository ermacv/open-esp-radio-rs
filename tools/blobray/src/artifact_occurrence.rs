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

pub(crate) fn function_locator(member: Option<&str>, symbol: &str, address: u64) -> String {
    match member {
        Some(member) => {
            format!("archive-member:{member}/symbol:{symbol}/object-offset:{address:#x}")
        }
        None => format!("symbol:{symbol}/address:{address:#x}"),
    }
}

pub(crate) fn function_occurrence(
    artifact: &ArtifactIdentity,
    member: Option<&str>,
    symbol: &str,
    address: u64,
) -> Result<ArtifactOccurrence> {
    occurrence(
        EntityDomain::Function,
        artifact,
        function_locator(member, symbol, address),
    )
}

pub(crate) fn memory_object_locator(
    member: Option<&str>,
    section: &str,
    symbol: &str,
    object_offset: u64,
    address: Option<u32>,
    size: u64,
) -> String {
    match member {
        Some(member) => format!(
            "archive-member:{member}/section:{section}/symbol:{symbol}/object-offset:{object_offset:#x}/size:{size:#x}"
        ),
        None => format!(
            "section:{section}/symbol:{symbol}/address:{:#x}/size:{size:#x}",
            address.unwrap_or(object_offset as u32)
        ),
    }
}

pub(crate) fn memory_object_occurrence(
    artifact: &ArtifactIdentity,
    member: Option<&str>,
    section: &str,
    symbol: &str,
    object_offset: u64,
    address: Option<u32>,
    size: u64,
) -> Result<ArtifactOccurrence> {
    occurrence(
        EntityDomain::MemoryObject,
        artifact,
        memory_object_locator(member, section, symbol, object_offset, address, size),
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
    fn function_occurrences_preserve_archive_and_linked_locators() {
        let archived = function_occurrence(&artifact(), Some("15.o"), "r_sym_ble", 0x24).unwrap();
        assert_eq!(
            archived.locator,
            "archive-member:15.o/symbol:r_sym_ble/object-offset:0x24"
        );
        assert_eq!(archived.id.domain(), EntityDomain::Function);

        let linked = function_occurrence(&artifact(), None, "r_sym_ble", 0x4200_1234).unwrap();
        assert_eq!(linked.locator, "symbol:r_sym_ble/address:0x42001234");
        assert_eq!(linked.id.domain(), EntityDomain::Function);
    }

    #[test]
    fn memory_object_occurrences_preserve_archive_and_linked_locators() {
        let archived = memory_object_occurrence(
            &artifact(),
            Some("55.o"),
            ".bss",
            "r_data_ble",
            0x18,
            None,
            0x20,
        )
        .unwrap();
        assert_eq!(
            archived.locator,
            "archive-member:55.o/section:.bss/symbol:r_data_ble/object-offset:0x18/size:0x20"
        );
        assert_eq!(archived.id.domain(), EntityDomain::MemoryObject);

        let linked = memory_object_occurrence(
            &artifact(),
            None,
            ".data",
            "r_data_ble",
            0x18,
            Some(0x4080_0100),
            4,
        )
        .unwrap();
        assert_eq!(
            linked.locator,
            "section:.data/symbol:r_data_ble/address:0x40800100/size:0x4"
        );
        assert_eq!(linked.id.domain(), EntityDomain::MemoryObject);
    }
}
