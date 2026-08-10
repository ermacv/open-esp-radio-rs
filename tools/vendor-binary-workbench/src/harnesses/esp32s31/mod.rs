//! ESP32-S31 harness registration facade.

pub(crate) use open_radio_vendor_harness_esp32s31_semantic::{
    CONTRACTS, RISCV_HARNESS, driver_adapter_evidence_sources, semantic_contract_evidence_sources,
    verification, verify_driver_adapter, verify_semantic_contract,
};

pub(super) fn verify_driver_adapter_registered(
    request: &crate::harnesses::DriverAdapterRequest<'_>,
) -> crate::Result<Option<crate::harnesses::DriverAdapterVerification>> {
    Ok(verify_driver_adapter(request)?)
}

pub(super) fn verify_semantic_contract_registered(
    request: &crate::harnesses::SemanticContractRequest<'_>,
) -> crate::Result<Option<bool>> {
    Ok(verify_semantic_contract(request)?)
}

pub(super) fn verify_named_contract_registered(
    name: &str,
    svd: &crate::MmioMap,
    vendor_artifact: &std::path::Path,
    vendor_companion: &std::path::Path,
) -> crate::Result<verification::QualificationReport> {
    match name {
        "channel" => Ok(verification::verify_esp32s31_channel(
            svd,
            vendor_artifact,
            vendor_companion,
        )?),
        "rf-init" => Ok(verification::verify_esp32s31_rf_init(
            svd,
            vendor_artifact,
            vendor_companion,
        )?),
        "bluetooth-tx-power" => Ok(verification::verify_esp32s31_bluetooth_tx_power(
            svd,
            vendor_artifact,
            vendor_companion,
        )?),
        _ => Err(crate::Error::invalid(format!(
            "selected harness has no contract {name:?}"
        ))),
    }
}
#[cfg(test)]
pub(crate) use open_radio_vendor_harness_esp32s31_semantic::{
    entry_contract, external_abi, wide_signed_divide_target_address,
};
