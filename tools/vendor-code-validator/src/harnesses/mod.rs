//! Platform-specific validation harnesses.

pub(crate) mod esp32s31;
mod neutral;

pub(crate) use open_radio_vendor_validator_semantic::{
    DriverAdapterEvidenceSources, DriverAdapterQualification, DriverAdapterRequest,
    SemanticContractEvidenceSources, SemanticContractRequest,
};

pub(crate) fn is_available(harness: &str) -> bool {
    harness == "esp32s31-phy-v1"
}

pub(crate) fn contracts(harness: &str) -> crate::Result<&'static crate::HarnessContractSpec> {
    match harness {
        "esp32s31-phy-v1" => Ok(&esp32s31::CONTRACTS),
        _ => Err(format!("unavailable platform harness {harness:?}").into()),
    }
}

pub(crate) fn riscv(harness: &str) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    match harness {
        "esp32s31-phy-v1" => Ok(&esp32s31::RISCV_HARNESS),
        _ => Err(format!("harness {harness:?} has no RISC-V adapter").into()),
    }
}

pub(crate) fn riscv_or_neutral(
    harness: Option<&str>,
) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    harness.map_or(Ok(&neutral::RISCV_HARNESS), riscv)
}

pub(crate) fn entry_contract(harness: &str, id: &str) -> crate::Result<crate::EntryContractRef> {
    contracts(harness)?
        .entry_contract(id)
        .ok_or_else(|| format!("harness {harness:?} has no entry contract {id:?}").into())
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

pub(crate) fn qualify_driver_adapter(
    harness: &str,
    request: &DriverAdapterRequest<'_>,
) -> crate::Result<Option<DriverAdapterQualification>> {
    match harness {
        "esp32s31-phy-v1" => esp32s31::qualify_driver_adapter(request),
        _ => Err(format!("unavailable platform harness {harness:?}").into()),
    }
}

pub(crate) fn qualify_semantic_contract(
    harness: &str,
    request: &SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    match harness {
        "esp32s31-phy-v1" => esp32s31::qualify_semantic_contract(request),
        _ => Err(format!("unavailable platform harness {harness:?}").into()),
    }
}

pub(crate) fn driver_adapter_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<DriverAdapterEvidenceSources> {
    match harness {
        "esp32s31-phy-v1" => esp32s31::driver_adapter_evidence_sources(id),
        _ => None,
    }
}

pub(crate) fn semantic_contract_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<SemanticContractEvidenceSources> {
    match harness {
        "esp32s31-phy-v1" => esp32s31::semantic_contract_evidence_sources(id),
        _ => None,
    }
}

pub(crate) fn qualify_named_contract(
    harness: &str,
    name: &str,
    svd: &crate::MmioRegisterMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<bool> {
    if harness != "esp32s31-phy-v1" {
        return Err(format!("unavailable platform harness {harness:?}").into());
    }
    match name {
        "channel" => esp32s31::qualification::qualify_esp32s31_channel(
            svd,
            vendor_artifact,
            vendor_companion,
            true,
        ),
        "rf-init" => esp32s31::qualification::qualify_esp32s31_rf_init(
            svd,
            vendor_artifact,
            vendor_companion,
            true,
        ),
        _ => Err(format!("selected harness has no contract {name:?}").into()),
    }
}
