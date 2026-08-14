//! Runtime registry for optional compiled knowledge and verification add-ons.

use std::sync::OnceLock;

mod neutral;

/// Architecture/ABI knowledge used while lifting vendor code.
///
/// This provider cannot run production comparisons or inspect qualification
/// state. A data-only chip pack selects it by `id`.
pub struct KnowledgeProviderDescriptor {
    pub id: &'static str,
    pub contracts: &'static crate::KnowledgeContractSpec,
    pub riscv: Option<&'static crate::RiscvHarnessSpec>,
}

/// Statically linked architecture/ABI knowledge add-ons. Executable
/// comparison plans are data owned by the project verification add-on and are
/// evaluated by the generic engine.
pub struct ProviderRegistry {
    pub knowledge: &'static [KnowledgeProviderDescriptor],
}

static BUILTIN_REGISTRY: ProviderRegistry = ProviderRegistry { knowledge: &[] };

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
    }
}
