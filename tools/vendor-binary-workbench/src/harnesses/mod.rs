//! Platform-specific verification harnesses.

pub(crate) mod esp32s31;
mod neutral;

pub(crate) use open_radio_vendor_harness_esp32s31_semantic::verification::{
    QualificationCase, QualificationDifference, QualificationReport,
};
pub(crate) use open_radio_vendor_semantics::{
    DriverAdapterEvidenceSources, DriverAdapterRequest, DriverAdapterVerification,
    SemanticContractEvidenceSources, SemanticContractRequest,
};

pub(crate) fn is_available(harness: &str) -> bool {
    harness == "esp32s31-radio-v1"
}

pub(crate) fn contracts(harness: &str) -> crate::Result<&'static crate::HarnessContractSpec> {
    match harness {
        "esp32s31-radio-v1" => Ok(&esp32s31::CONTRACTS),
        _ => Err(crate::Error::invalid(format!(
            "unavailable platform harness {harness:?}"
        ))),
    }
}

pub(crate) fn riscv(harness: &str) -> crate::Result<&'static crate::RiscvHarnessSpec> {
    match harness {
        "esp32s31-radio-v1" => Ok(&esp32s31::RISCV_HARNESS),
        _ => Err(crate::Error::invalid(format!(
            "harness {harness:?} has no RISC-V adapter"
        ))),
    }
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
    match harness {
        "esp32s31-radio-v1" => Ok(esp32s31::verify_driver_adapter(request)?),
        _ => Err(crate::Error::invalid(format!(
            "unavailable platform harness {harness:?}"
        ))),
    }
}

pub(crate) fn verify_semantic_contract(
    harness: &str,
    request: &SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    match harness {
        "esp32s31-radio-v1" => Ok(esp32s31::verify_semantic_contract(request)?),
        _ => Err(crate::Error::invalid(format!(
            "unavailable platform harness {harness:?}"
        ))),
    }
}

pub(crate) fn driver_adapter_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<DriverAdapterEvidenceSources> {
    match harness {
        "esp32s31-radio-v1" => esp32s31::driver_adapter_evidence_sources(id),
        _ => None,
    }
}

pub(crate) fn semantic_contract_evidence_sources(
    harness: &str,
    id: &str,
) -> Option<SemanticContractEvidenceSources> {
    match harness {
        "esp32s31-radio-v1" => esp32s31::semantic_contract_evidence_sources(id),
        _ => None,
    }
}

pub(crate) fn verify_named_contract(
    harness: &str,
    name: &str,
    svd: &crate::MmioRegisterMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<QualificationReport> {
    if harness != "esp32s31-radio-v1" {
        return Err(crate::Error::invalid(format!(
            "unavailable platform harness {harness:?}"
        )));
    }
    match name {
        "channel" => Ok(esp32s31::verification::verify_esp32s31_channel(
            svd,
            vendor_artifact,
            vendor_companion,
        )?),
        "rf-init" => Ok(esp32s31::verification::verify_esp32s31_rf_init(
            svd,
            vendor_artifact,
            vendor_companion,
        )?),
        _ => Err(crate::Error::invalid(format!(
            "selected harness has no contract {name:?}"
        ))),
    }
}
