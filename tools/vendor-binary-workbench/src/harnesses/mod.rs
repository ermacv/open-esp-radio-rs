//! Runtime registry for optional compiled knowledge and verification add-ons.

use std::{error::Error as StdError, sync::OnceLock};

mod neutral;

pub(crate) use open_radio_vendor_semantics::{
    DriverAdapterEvidenceSources, DriverAdapterRequest, DriverAdapterVerification,
    SemanticContractEvidenceSources, SemanticContractRequest, SemanticVerificationReport,
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
) -> ProviderResult<SemanticVerificationReport>;

/// Architecture/ABI knowledge used while lifting vendor code.
///
/// This provider cannot run production comparisons or inspect qualification
/// state. A data-only chip pack selects it by `id`.
pub struct KnowledgeProviderDescriptor {
    pub id: &'static str,
    pub contracts: &'static crate::KnowledgeContractSpec,
    pub riscv: Option<&'static crate::RiscvHarnessSpec>,
}

/// Optional compiled comparison provider. Keeping this separate prevents
/// generic analysis from acquiring a production-driver dependency.
pub struct VerificationProviderDescriptor {
    pub id: &'static str,
    pub verify_driver_adapter: DriverAdapterVerifier,
    pub verify_semantic_contract: SemanticContractVerifier,
    pub driver_adapter_evidence_sources: DriverEvidenceLookup,
    pub semantic_contract_evidence_sources: SemanticEvidenceLookup,
    pub verify_named_contract: NamedContractVerifier,
}

/// Statically linked add-ons. Knowledge and verification have independent
/// registries so selecting target knowledge does not implicitly enable a
/// production-driver comparison provider. Product readiness is deliberately
/// outside the Workbench API.
pub struct ProviderRegistry {
    pub knowledge: &'static [KnowledgeProviderDescriptor],
    pub verification: &'static [VerificationProviderDescriptor],
}

static BUILTIN_REGISTRY: ProviderRegistry = ProviderRegistry {
    knowledge: &[],
    verification: &[],
};

static INSTALLED_REGISTRY: OnceLock<&'static ProviderRegistry> = OnceLock::new();

pub fn install_registry(registry: &'static ProviderRegistry) -> std::result::Result<(), String> {
    INSTALLED_REGISTRY
        .set(registry)
        .map_err(|_| "add-on provider registry was already installed".to_owned())
}

fn registry() -> &'static ProviderRegistry {
    INSTALLED_REGISTRY
        .get()
        .copied()
        .unwrap_or(&BUILTIN_REGISTRY)
}

fn knowledge_descriptor(provider: &str) -> crate::Result<&'static KnowledgeProviderDescriptor> {
    registry()
        .knowledge
        .iter()
        .find(|descriptor| descriptor.id == provider)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "unavailable knowledge provider {provider:?}; this build provides: {}",
                registry()
                    .knowledge
                    .iter()
                    .map(|descriptor| descriptor.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

fn verification_descriptor(
    provider: &str,
) -> crate::Result<&'static VerificationProviderDescriptor> {
    registry()
        .verification
        .iter()
        .find(|descriptor| descriptor.id == provider)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "unavailable verification provider {provider:?}; this build provides: {}",
                registry()
                    .verification
                    .iter()
                    .map(|descriptor| descriptor.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

pub(crate) fn is_available(provider: &str) -> bool {
    registry()
        .knowledge
        .iter()
        .any(|descriptor| descriptor.id == provider)
}

pub(crate) fn contracts(provider: &str) -> crate::Result<&'static crate::KnowledgeContractSpec> {
    Ok(knowledge_descriptor(provider)?.contracts)
}

pub(crate) fn riscv(provider: &str) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    knowledge_descriptor(provider)?.riscv.ok_or_else(|| {
        crate::Error::invalid(format!(
            "knowledge provider {provider:?} has no RISC-V adapter"
        ))
    })
}

pub(crate) fn riscv_or_neutral(
    provider: Option<&str>,
) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    provider.map_or(Ok(&neutral::RISCV_HARNESS), riscv)
}

pub(crate) fn entry_contract(provider: &str, id: &str) -> crate::Result<crate::EntryContractRef> {
    contracts(provider)?.entry_contract(id).ok_or_else(|| {
        crate::Error::invalid(format!(
            "knowledge provider {provider:?} has no entry contract {id:?}"
        ))
    })
}

pub(crate) fn entry_contract_or_neutral(
    provider: Option<&str>,
    id: &str,
) -> crate::Result<crate::EntryContractRef> {
    match provider {
        Some(provider) => entry_contract(provider, id),
        None => neutral::entry_contract(id),
    }
}

pub(crate) fn verify_driver_adapter(
    provider: &str,
    request: &DriverAdapterRequest<'_>,
) -> crate::Result<Option<DriverAdapterVerification>> {
    (verification_descriptor(provider)?.verify_driver_adapter)(request)
        .map_err(|error| crate::Error::addon_provider(provider, error))
}

pub(crate) fn verify_semantic_contract(
    provider: &str,
    request: &SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    (verification_descriptor(provider)?.verify_semantic_contract)(request)
        .map_err(|error| crate::Error::addon_provider(provider, error))
}

pub(crate) fn driver_adapter_evidence_sources(
    provider: &str,
    id: &str,
) -> Option<DriverAdapterEvidenceSources> {
    verification_descriptor(provider)
        .ok()
        .and_then(|descriptor| (descriptor.driver_adapter_evidence_sources)(id))
}

pub(crate) fn semantic_contract_evidence_sources(
    provider: &str,
    id: &str,
) -> Option<SemanticContractEvidenceSources> {
    verification_descriptor(provider)
        .ok()
        .and_then(|descriptor| (descriptor.semantic_contract_evidence_sources)(id))
}

pub(crate) fn verify_named_contract(
    provider: &str,
    name: &str,
    svd: &crate::MmioMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<SemanticVerificationReport> {
    (verification_descriptor(provider)?.verify_named_contract)(
        name,
        svd,
        vendor_artifact,
        vendor_companion,
    )
    .map_err(|error| crate::Error::addon_provider(provider, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_ids_are_unique_and_resolvable() {
        let ids = registry()
            .knowledge
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), registry().knowledge.len());
        for id in ids {
            assert!(is_available(id));
            assert_eq!(knowledge_descriptor(id).unwrap().id, id);
        }
    }

    #[test]
    fn provider_capabilities_have_independent_registries() {
        assert!(registry().knowledge.is_empty());
        assert!(registry().verification.is_empty());
    }
}
