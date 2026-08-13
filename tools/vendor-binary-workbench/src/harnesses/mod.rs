//! Runtime registry for optional compiled platform providers.

use std::{error::Error as StdError, sync::OnceLock};

mod neutral;

pub(crate) use open_radio_vendor_semantics::{
    DriverAdapterEvidenceSources, DriverAdapterRequest, DriverAdapterVerification,
    QualificationReport, SemanticContractEvidenceSources, SemanticContractRequest,
};

pub type ProviderError = Box<dyn StdError + Send + Sync + 'static>;
pub type ProviderResult<T> = std::result::Result<T, ProviderError>;
pub type DriverAdapterVerifier =
    for<'a> fn(&DriverAdapterRequest<'a>) -> ProviderResult<Option<DriverAdapterVerification>>;
pub type SemanticContractVerifier =
    for<'a> fn(&SemanticContractRequest<'a>) -> ProviderResult<Option<bool>>;
pub type DriverEvidenceLookup = fn(&str) -> Option<DriverAdapterEvidenceSources>;
pub type SemanticEvidenceLookup = fn(&str) -> Option<SemanticContractEvidenceSources>;
pub type NamedContractVerifier = fn(
    &str,
    &crate::MmioMap,
    &std::path::Path,
    &std::path::Path,
) -> ProviderResult<QualificationReport>;

/// One statically linked executable provider. Data-only platform packs select
/// it by `id`; generic analysis remains available when the registry is empty.
pub struct HarnessDescriptor {
    pub id: &'static str,
    pub contracts: &'static crate::HarnessContractSpec,
    pub riscv: Option<&'static crate::RiscvHarnessSpec>,
    pub verify_driver_adapter: DriverAdapterVerifier,
    pub verify_semantic_contract: SemanticContractVerifier,
    pub driver_adapter_evidence_sources: DriverEvidenceLookup,
    pub semantic_contract_evidence_sources: SemanticEvidenceLookup,
    pub verify_named_contract: NamedContractVerifier,
}

static BUILTIN_REGISTRY: &[HarnessDescriptor] = &[];

static INSTALLED_REGISTRY: OnceLock<&'static [HarnessDescriptor]> = OnceLock::new();

pub fn install_registry(registry: &'static [HarnessDescriptor]) -> std::result::Result<(), String> {
    INSTALLED_REGISTRY
        .set(registry)
        .map_err(|_| "platform provider registry was already installed".to_owned())
}

pub(crate) fn registry() -> &'static [HarnessDescriptor] {
    INSTALLED_REGISTRY
        .get()
        .copied()
        .unwrap_or(BUILTIN_REGISTRY)
}

fn descriptor(harness: &str) -> crate::Result<&'static HarnessDescriptor> {
    registry()
        .iter()
        .find(|descriptor| descriptor.id == harness)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "unavailable platform harness {harness:?}; this build provides: {}",
                registry()
                    .iter()
                    .map(|descriptor| descriptor.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

pub(crate) fn is_available(harness: &str) -> bool {
    registry().iter().any(|descriptor| descriptor.id == harness)
}

pub(crate) fn contracts(harness: &str) -> crate::Result<&'static crate::HarnessContractSpec> {
    Ok(descriptor(harness)?.contracts)
}

pub(crate) fn riscv(harness: &str) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    descriptor(harness)?
        .riscv
        .ok_or_else(|| crate::Error::invalid(format!("harness {harness:?} has no RISC-V adapter")))
}

pub(crate) fn riscv_or_neutral(
    harness: Option<&str>,
) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    harness.map_or(Ok(&neutral::RISCV_HARNESS), riscv)
}

pub(crate) fn entry_contract(harness: &str, id: &str) -> crate::Result<crate::EntryContractRef> {
    contracts(harness)?.entry_contract(id).ok_or_else(|| {
        crate::Error::invalid(format!("harness {harness:?} has no entry contract {id:?}"))
    })
}

pub(crate) fn entry_contract_or_neutral(
    harness: Option<&str>,
    id: &str,
) -> crate::Result<crate::EntryContractRef> {
    match harness {
        Some(harness) => entry_contract(harness, id),
        None => neutral::entry_contract(id),
    }
}

pub(crate) fn verify_driver_adapter(
    harness: &str,
    request: &DriverAdapterRequest<'_>,
) -> crate::Result<Option<DriverAdapterVerification>> {
    (descriptor(harness)?.verify_driver_adapter)(request)
        .map_err(|error| crate::Error::platform_provider(harness, error))
}

pub(crate) fn verify_semantic_contract(
    harness: &str,
    request: &SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    (descriptor(harness)?.verify_semantic_contract)(request)
        .map_err(|error| crate::Error::platform_provider(harness, error))
}

pub(crate) fn driver_adapter_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<DriverAdapterEvidenceSources> {
    descriptor(harness)
        .ok()
        .and_then(|descriptor| (descriptor.driver_adapter_evidence_sources)(id))
}

pub(crate) fn semantic_contract_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<SemanticContractEvidenceSources> {
    descriptor(harness)
        .ok()
        .and_then(|descriptor| (descriptor.semantic_contract_evidence_sources)(id))
}

pub(crate) fn verify_named_contract(
    harness: &str,
    name: &str,
    svd: &crate::MmioMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<QualificationReport> {
    (descriptor(harness)?.verify_named_contract)(name, svd, vendor_artifact, vendor_companion)
        .map_err(|error| crate::Error::platform_provider(harness, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_resolvable() {
        let ids = registry()
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), registry().len());
        for id in ids {
            assert!(is_available(id));
            assert_eq!(descriptor(id).unwrap().id, id);
        }
    }
}
