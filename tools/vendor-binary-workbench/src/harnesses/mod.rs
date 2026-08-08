//! Static registry for optional compiled platform harnesses.

#[cfg(feature = "esp32s31-harness")]
pub(crate) mod esp32s31;
mod neutral;

#[cfg(feature = "esp32s31-harness")]
pub(crate) use open_radio_vendor_harness_esp32s31_semantic::verification::{
    QualificationCase, QualificationDifference, QualificationReport,
};
pub(crate) use open_radio_vendor_semantics::{
    DriverAdapterEvidenceSources, DriverAdapterRequest, DriverAdapterVerification,
    SemanticContractEvidenceSources, SemanticContractRequest,
};

type DriverAdapterVerifier =
    for<'a> fn(&DriverAdapterRequest<'a>) -> crate::Result<Option<DriverAdapterVerification>>;
type SemanticContractVerifier =
    for<'a> fn(&SemanticContractRequest<'a>) -> crate::Result<Option<bool>>;
type DriverEvidenceLookup = fn(&str) -> Option<DriverAdapterEvidenceSources>;
type SemanticEvidenceLookup = fn(&str) -> Option<SemanticContractEvidenceSources>;

/// One statically linked executable addon. Data-only platform packs select it
/// by `id`; generic analysis remains available when the registry is empty.
pub(crate) struct HarnessDescriptor {
    pub(crate) id: &'static str,
    contracts: &'static crate::HarnessContractSpec,
    riscv: Option<&'static crate::RiscvHarnessSpec>,
    verify_driver_adapter: DriverAdapterVerifier,
    verify_semantic_contract: SemanticContractVerifier,
    driver_adapter_evidence_sources: DriverEvidenceLookup,
    semantic_contract_evidence_sources: SemanticEvidenceLookup,
    #[cfg(feature = "esp32s31-harness")]
    verify_named_contract: fn(
        &str,
        &crate::MmioMap,
        &std::path::Path,
        &std::path::Path,
    ) -> crate::Result<QualificationReport>,
}

#[cfg(feature = "esp32s31-harness")]
static REGISTRY: &[HarnessDescriptor] = &[HarnessDescriptor {
    id: "esp32s31-radio-v1",
    contracts: &esp32s31::CONTRACTS,
    riscv: Some(&esp32s31::RISCV_HARNESS),
    verify_driver_adapter: esp32s31::verify_driver_adapter_registered,
    verify_semantic_contract: esp32s31::verify_semantic_contract_registered,
    driver_adapter_evidence_sources: esp32s31::driver_adapter_evidence_sources,
    semantic_contract_evidence_sources: esp32s31::semantic_contract_evidence_sources,
    verify_named_contract: esp32s31::verify_named_contract_registered,
}];

#[cfg(not(feature = "esp32s31-harness"))]
static REGISTRY: &[HarnessDescriptor] = &[];

pub(crate) fn registry() -> &'static [HarnessDescriptor] {
    REGISTRY
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
}

pub(crate) fn verify_semantic_contract(
    harness: &str,
    request: &SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    (descriptor(harness)?.verify_semantic_contract)(request)
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

#[cfg(feature = "esp32s31-harness")]
pub(crate) fn verify_named_contract(
    harness: &str,
    name: &str,
    svd: &crate::MmioMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<QualificationReport> {
    (descriptor(harness)?.verify_named_contract)(name, svd, vendor_artifact, vendor_companion)
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
