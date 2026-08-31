//! Runtime registry for optional compiled knowledge and verification add-ons.

use std::{collections::BTreeSet, sync::OnceLock};

mod neutral;

/// Architecture/ABI knowledge used while lifting vendor code.
///
/// This provider cannot run production comparisons or inspect qualification
/// state. A data-only chip pack or an investigation project selects it by
/// `id`, according to the provider's reuse scope.
pub struct KnowledgeProviderDescriptor {
    pub id: &'static str,
    /// Optional reusable provider that this investigation overlay explicitly
    /// extends. The overlay descriptor must expose a contract superset and a
    /// precomposed architecture harness; arbitrary provider pairs never layer.
    pub extends: Option<&'static str>,
    /// Semantic revision of the compiled provider used by persistent
    /// analysis queries.
    ///
    /// Provider code is not a file-backed project input: it is linked into
    /// the target-specific Blobray host.  Incrementing this value prevents a
    /// rebuilt host from accepting linked IR produced by an older set of
    /// contracts or summary hooks while keeping unrelated structural stages
    /// cacheable.
    pub analysis_cache_revision: u32,
    pub contracts: &'static crate::KnowledgeContractSpec,
    pub riscv: Option<&'static crate::RiscvHarnessSpec>,
}

/// Statically linked architecture/ABI knowledge add-ons. Executable
/// comparison plans are data owned by the project verification add-on and are
/// evaluated by the generic engine.
pub struct ProviderRegistry {
    pub knowledge: &'static [KnowledgeProviderDescriptor],
}

impl ProviderRegistry {
    /// Validate descriptor identity, explicit extension and contract-superset
    /// invariants without installing the registry globally.
    pub fn validate(&self) -> std::result::Result<(), String> {
        validate_registry(self)
    }
}

static BUILTIN_REGISTRY: ProviderRegistry = ProviderRegistry { knowledge: &[] };

static INSTALLED_REGISTRY: OnceLock<&'static ProviderRegistry> = OnceLock::new();

pub fn install_registry(registry: &'static ProviderRegistry) -> std::result::Result<(), String> {
    registry.validate()?;
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

fn descriptor_identity(descriptor: &KnowledgeProviderDescriptor) -> String {
    descriptor_identity_in(registry(), descriptor)
}

fn descriptor_identity_in(
    registry: &ProviderRegistry,
    descriptor: &KnowledgeProviderDescriptor,
) -> String {
    let own = format!("{}@{}", descriptor.id, descriptor.analysis_cache_revision);
    descriptor.extends.map_or(own.clone(), |base| {
        let base = registry
            .knowledge
            .iter()
            .find(|candidate| candidate.id == base)
            .expect("installed provider registry was validated");
        format!("{}+{own}", descriptor_identity_in(registry, base))
    })
}

fn validate_registry(registry: &ProviderRegistry) -> std::result::Result<(), String> {
    let mut ids = BTreeSet::new();
    for descriptor in registry.knowledge {
        if descriptor.id.is_empty()
            || descriptor.id.chars().any(char::is_whitespace)
            || !ids.insert(descriptor.id)
        {
            return Err(format!(
                "knowledge provider IDs must be unique non-empty tokens, got {:?}",
                descriptor.id
            ));
        }
        if descriptor.analysis_cache_revision == 0 {
            return Err(format!(
                "knowledge provider {:?} has zero analysis cache revision",
                descriptor.id
            ));
        }
        if descriptor
            .riscv
            .is_some_and(|harness| harness.semantic_cache_domain.is_empty())
        {
            return Err(format!(
                "knowledge provider {:?} has an empty RISC-V semantic cache domain",
                descriptor.id
            ));
        }
        if let Some(harness) = descriptor.riscv {
            validate_reviewed_memory_accesses(descriptor.id, harness.reviewed_memory_accesses)?;
        }
    }
    for descriptor in registry.knowledge {
        let Some(base_id) = descriptor.extends else {
            continue;
        };
        let base = registry
            .knowledge
            .iter()
            .find(|candidate| candidate.id == base_id)
            .ok_or_else(|| {
                format!(
                    "knowledge provider {:?} extends unavailable base {base_id:?}",
                    descriptor.id
                )
            })?;
        if base.id == descriptor.id || base.extends.is_some() {
            return Err(format!(
                "knowledge provider {:?} must extend one reusable root provider, got {base_id:?}",
                descriptor.id
            ));
        }
        validate_contract_superset(base, descriptor)?;
        match (base.riscv, descriptor.riscv) {
            (Some(base), Some(overlay))
                if !overlay
                    .semantic_cache_domain
                    .strip_prefix(base.semantic_cache_domain)
                    .is_some_and(|suffix| suffix.starts_with('+') && suffix.len() > 1) =>
            {
                return Err(format!(
                    "knowledge provider {:?} semantic cache domain {:?} does not extend base domain {:?}",
                    descriptor.id, overlay.semantic_cache_domain, base.semantic_cache_domain
                ));
            }
            (Some(_), None) => {
                return Err(format!(
                    "knowledge provider {:?} drops its base RISC-V harness",
                    descriptor.id
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_contract_superset(
    base: &KnowledgeProviderDescriptor,
    overlay: &KnowledgeProviderDescriptor,
) -> std::result::Result<(), String> {
    let models_are_preserved = base
        .contracts
        .external_call_model_sets
        .iter()
        .all(|base_models| {
            overlay
                .contracts
                .external_call_model_sets
                .iter()
                .any(|overlay_models| overlay_models.spec() == base_models.spec())
        });
    let entries_are_preserved = base.contracts.entry_contracts.iter().all(|base_entry| {
        overlay
            .contracts
            .entry_contracts
            .iter()
            .any(|overlay_entry| entry_contract_specs_equal(*base_entry, *overlay_entry))
    });
    let base_diagnostics = base
        .contracts
        .diagnostic_calls
        .iter()
        .map(|call| (call.symbol, call.argument_count))
        .collect::<BTreeSet<_>>();
    let overlay_diagnostics = overlay
        .contracts
        .diagnostic_calls
        .iter()
        .map(|call| (call.symbol, call.argument_count))
        .collect::<BTreeSet<_>>();
    if !models_are_preserved
        || !entries_are_preserved
        || !base_diagnostics.is_subset(&overlay_diagnostics)
    {
        return Err(format!(
            "knowledge provider {:?} is not a contract superset of base {:?}",
            overlay.id, base.id
        ));
    }
    if let (Some(base), Some(overlay)) = (base.riscv, overlay.riscv)
        && !base
            .reviewed_memory_accesses
            .iter()
            .all(|fact| overlay.reviewed_memory_accesses.contains(fact))
    {
        return Err(format!(
            "knowledge provider {:?} drops reviewed memory-access facts from base {:?}",
            overlay.semantic_cache_domain, base.semantic_cache_domain
        ));
    }
    Ok(())
}

fn validate_reviewed_memory_accesses(
    provider: &str,
    facts: &[crate::ReviewedMemoryAccessClassification],
) -> std::result::Result<(), String> {
    let mut ids = BTreeSet::new();
    let mut occurrences = BTreeSet::new();
    for fact in facts {
        fact.validate()
            .map_err(|reason| format!("knowledge provider {provider:?}: {reason}"))?;
        if !ids.insert(fact.id) {
            return Err(format!(
                "knowledge provider {provider:?} repeats reviewed memory-access ID {:?}",
                fact.id
            ));
        }
        if !occurrences.insert((
            fact.occurrence.function,
            fact.occurrence.site,
            fact.occurrence.operation,
        )) {
            return Err(format!(
                "knowledge provider {provider:?} classifies memory access {} at {:#x} more than once",
                fact.occurrence.function, fact.occurrence.site
            ));
        }
    }
    Ok(())
}

fn entry_contract_specs_equal(
    left: crate::EntryContractRef,
    right: crate::EntryContractRef,
) -> bool {
    let left = left.spec();
    let right = right.spec();
    let tables_are_equal = match (left.function_table, right.function_table) {
        (None, None) => true,
        (Some(left), Some(right)) => left.id() == right.id() && left.targets().eq(right.targets()),
        _ => false,
    };
    left.id == right.id
        && tables_are_equal
        && left.pointer_symbols == right.pointer_symbols
        && left.data_pointer_binding == right.data_pointer_binding
}

pub(crate) fn compose_provider(base: &str, overlay: &str) -> crate::Result<&'static str> {
    compose_provider_in(registry(), base, overlay).map_err(crate::Error::invalid)
}

fn compose_provider_in(
    registry: &'static ProviderRegistry,
    base: &str,
    overlay: &str,
) -> std::result::Result<&'static str, String> {
    let overlay = registry
        .knowledge
        .iter()
        .find(|descriptor| descriptor.id == overlay)
        .ok_or_else(|| format!("unavailable investigation knowledge provider {overlay:?}"))?;
    if overlay.extends != Some(base) {
        return Err(format!(
            "investigation knowledge provider {:?} does not explicitly extend chip provider {base:?}",
            overlay.id
        ));
    }
    Ok(overlay.id)
}

pub(crate) fn is_available(provider: &str) -> bool {
    registry()
        .knowledge
        .iter()
        .any(|descriptor| descriptor.id == provider)
}

pub(crate) fn analysis_cache_identity(provider: Option<&str>) -> String {
    diagnostic_contracts_or_empty(provider).map_or_else(
        |_| format!("unavailable:{}", provider.unwrap_or("<none>")),
        |contracts| contracts.canonical(),
    )
}

pub(crate) fn contracts(provider: &str) -> crate::Result<&'static crate::KnowledgeContractSpec> {
    Ok(knowledge_descriptor(provider)?.contracts)
}

pub(crate) fn diagnostic_contracts_or_empty(
    provider: Option<&str>,
) -> crate::Result<crate::DiagnosticContractsReport> {
    let Some(provider) = provider else {
        return Ok(crate::DiagnosticContractsReport::default());
    };
    let descriptor = knowledge_descriptor(provider)?;
    diagnostic_contracts(descriptor)
}

fn diagnostic_contracts(
    descriptor: &KnowledgeProviderDescriptor,
) -> crate::Result<crate::DiagnosticContractsReport> {
    crate::DiagnosticContractsReport::from_calls(
        Some(descriptor_identity(descriptor)),
        descriptor
            .contracts
            .diagnostic_calls
            .iter()
            .map(|call| (call.symbol, call.argument_count)),
    )
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

pub(crate) fn reviewed_memory_accesses(
    provider: Option<&str>,
) -> crate::Result<&'static [crate::ReviewedMemoryAccessClassification]> {
    Ok(riscv_or_neutral(provider)?.reviewed_memory_accesses)
}

pub(crate) fn reviewed_memory_access_artifact_sources(
    provider: Option<&str>,
) -> crate::Result<BTreeSet<String>> {
    Ok(reviewed_memory_accesses(provider)?
        .iter()
        .map(|fact| fact.occurrence.artifact_source.to_owned())
        .collect())
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
            let descriptor = knowledge_descriptor(id).unwrap();
            assert_eq!(descriptor.id, id);
            assert!(descriptor.analysis_cache_revision > 0);
            diagnostic_contracts_or_empty(Some(id)).unwrap();
        }
    }

    #[test]
    fn provider_capabilities_have_independent_registries() {
        assert!(registry().knowledge.is_empty());
    }

    #[test]
    fn neutral_diagnostic_contract_provenance_is_empty_and_canonical() {
        let contracts = diagnostic_contracts_or_empty(None).unwrap();

        assert!(contracts.knowledge_provider.is_none());
        assert!(contracts.calls.is_empty());
        assert_eq!(contracts.canonical(), "provider:6:<none>");
    }

    #[test]
    fn provider_diagnostic_contract_provenance_uses_revision_and_sorted_actual_calls() {
        static CALLS: &[crate::DiagnosticCallSpec] = &[
            crate::DiagnosticCallSpec {
                symbol: "z_log",
                argument_count: 2,
            },
            crate::DiagnosticCallSpec {
                symbol: "a_assert",
                argument_count: 4,
            },
        ];
        static CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: &[],
            diagnostic_calls: CALLS,
        };
        let descriptor = KnowledgeProviderDescriptor {
            id: "fixture-provider",
            extends: None,
            analysis_cache_revision: 7,
            contracts: &CONTRACTS,
            riscv: None,
        };

        let actual = diagnostic_contracts(&descriptor).unwrap();

        assert_eq!(
            actual.knowledge_provider.as_deref(),
            Some("fixture-provider@7")
        );
        assert_eq!(actual.calls[0].symbol, "a_assert");
        assert_eq!(actual.calls[0].argument_count, 4);
        assert_eq!(actual.calls[1].symbol, "z_log");
        assert_eq!(actual.calls[1].argument_count, 2);
    }

    #[test]
    fn provider_composition_requires_an_explicit_contract_superset() {
        static BASE_CALLS: &[crate::DiagnosticCallSpec] = &[crate::DiagnosticCallSpec {
            symbol: "rom_log",
            argument_count: 1,
        }];
        static OVERLAY_CALLS: &[crate::DiagnosticCallSpec] = &[
            crate::DiagnosticCallSpec {
                symbol: "rom_log",
                argument_count: 1,
            },
            crate::DiagnosticCallSpec {
                symbol: "blob_log",
                argument_count: 2,
            },
        ];
        static BASE_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: &[],
            diagnostic_calls: BASE_CALLS,
        };
        static OVERLAY_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: &[],
            diagnostic_calls: OVERLAY_CALLS,
        };
        static PROVIDERS: &[KnowledgeProviderDescriptor] = &[
            KnowledgeProviderDescriptor {
                id: "chip-v1",
                extends: None,
                analysis_cache_revision: 1,
                contracts: &BASE_CONTRACTS,
                riscv: None,
            },
            KnowledgeProviderDescriptor {
                id: "project-v1",
                extends: Some("chip-v1"),
                analysis_cache_revision: 2,
                contracts: &OVERLAY_CONTRACTS,
                riscv: None,
            },
        ];
        static REGISTRY: ProviderRegistry = ProviderRegistry {
            knowledge: PROVIDERS,
        };

        validate_registry(&REGISTRY).unwrap();
        assert_eq!(
            compose_provider_in(&REGISTRY, "chip-v1", "project-v1").unwrap(),
            "project-v1"
        );
        assert!(compose_provider_in(&REGISTRY, "other-chip", "project-v1").is_err());
        assert_eq!(
            descriptor_identity_in(&REGISTRY, &PROVIDERS[1]),
            "chip-v1@1+project-v1@2"
        );
    }

    #[test]
    fn provider_composition_rejects_a_contract_downgrade() {
        static BASE_CALLS: &[crate::DiagnosticCallSpec] = &[crate::DiagnosticCallSpec {
            symbol: "rom_log",
            argument_count: 1,
        }];
        static BASE_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: &[],
            diagnostic_calls: BASE_CALLS,
        };
        static EMPTY_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: &[],
            diagnostic_calls: &[],
        };
        static PROVIDERS: &[KnowledgeProviderDescriptor] = &[
            KnowledgeProviderDescriptor {
                id: "chip-v1",
                extends: None,
                analysis_cache_revision: 1,
                contracts: &BASE_CONTRACTS,
                riscv: None,
            },
            KnowledgeProviderDescriptor {
                id: "project-v1",
                extends: Some("chip-v1"),
                analysis_cache_revision: 1,
                contracts: &EMPTY_CONTRACTS,
                riscv: None,
            },
        ];
        static REGISTRY: ProviderRegistry = ProviderRegistry {
            knowledge: PROVIDERS,
        };

        assert!(
            validate_registry(&REGISTRY)
                .unwrap_err()
                .contains("not a contract superset")
        );
    }

    #[test]
    fn provider_composition_rejects_a_same_id_model_semantic_change() {
        static BASE_MODEL: crate::ExternalCallModelSetSpec = crate::ExternalCallModelSetSpec {
            id: "runtime-models-v1",
            models: &[crate::ExternalCallModelSpec {
                id: "clock-frequency",
                return_model: crate::ExternalReturnModel::Constant(40),
                outputs: &[],
            }],
        };
        static CHANGED_MODEL: crate::ExternalCallModelSetSpec = crate::ExternalCallModelSetSpec {
            id: "runtime-models-v1",
            models: &[crate::ExternalCallModelSpec {
                id: "clock-frequency",
                return_model: crate::ExternalReturnModel::Constant(26),
                outputs: &[],
            }],
        };
        static BASE_MODEL_REFS: &[crate::ExternalCallModelSetRef] =
            &[crate::ExternalCallModelSetRef::new(&BASE_MODEL)];
        static CHANGED_MODEL_REFS: &[crate::ExternalCallModelSetRef] =
            &[crate::ExternalCallModelSetRef::new(&CHANGED_MODEL)];
        static BASE_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: BASE_MODEL_REFS,
            entry_contracts: &[],
            diagnostic_calls: &[],
        };
        static CHANGED_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: CHANGED_MODEL_REFS,
            entry_contracts: &[],
            diagnostic_calls: &[],
        };
        let base = KnowledgeProviderDescriptor {
            id: "chip-v1",
            extends: None,
            analysis_cache_revision: 1,
            contracts: &BASE_CONTRACTS,
            riscv: None,
        };
        let overlay = KnowledgeProviderDescriptor {
            id: "project-v1",
            extends: Some("chip-v1"),
            analysis_cache_revision: 1,
            contracts: &CHANGED_CONTRACTS,
            riscv: None,
        };

        assert!(
            validate_contract_superset(&base, &overlay)
                .unwrap_err()
                .contains("not a contract superset")
        );
    }

    #[test]
    fn provider_composition_rejects_a_same_id_entry_table_change() {
        static BASE_TABLE: crate::FunctionTableSpec = crate::FunctionTableSpec {
            id: "rom-phy-table-v1",
            targets: &[crate::FunctionTarget::Address(0x4000_1000)],
        };
        static CHANGED_TABLE: crate::FunctionTableSpec = crate::FunctionTableSpec {
            id: "rom-phy-table-v1",
            targets: &[crate::FunctionTarget::Address(0x4000_2000)],
        };
        static BASE_ENTRY_SPEC: crate::EntryContractSpec = crate::EntryContractSpec {
            id: "phy-cold",
            function_table: Some(crate::FunctionTableRef::new(&BASE_TABLE)),
            pointer_symbols: &["g_phy_table"],
            data_pointer_binding: None,
        };
        static CHANGED_ENTRY_SPEC: crate::EntryContractSpec = crate::EntryContractSpec {
            id: "phy-cold",
            function_table: Some(crate::FunctionTableRef::new(&CHANGED_TABLE)),
            pointer_symbols: &["g_phy_table"],
            data_pointer_binding: None,
        };
        static BASE_ENTRY_REFS: &[crate::EntryContractRef] =
            &[crate::EntryContractRef::new(&BASE_ENTRY_SPEC)];
        static CHANGED_ENTRY_REFS: &[crate::EntryContractRef] =
            &[crate::EntryContractRef::new(&CHANGED_ENTRY_SPEC)];
        static BASE_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: BASE_ENTRY_REFS,
            diagnostic_calls: &[],
        };
        static CHANGED_CONTRACTS: crate::KnowledgeContractSpec = crate::KnowledgeContractSpec {
            external_call_model_sets: &[],
            entry_contracts: CHANGED_ENTRY_REFS,
            diagnostic_calls: &[],
        };
        let base = KnowledgeProviderDescriptor {
            id: "chip-v1",
            extends: None,
            analysis_cache_revision: 1,
            contracts: &BASE_CONTRACTS,
            riscv: None,
        };
        let overlay = KnowledgeProviderDescriptor {
            id: "project-v1",
            extends: Some("chip-v1"),
            analysis_cache_revision: 1,
            contracts: &CHANGED_CONTRACTS,
            riscv: None,
        };

        assert!(
            validate_contract_superset(&base, &overlay)
                .unwrap_err()
                .contains("not a contract superset")
        );
    }
}
