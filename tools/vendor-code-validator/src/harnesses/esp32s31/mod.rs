//! ESP32-S31 harness registration facade.

pub(crate) use open_radio_vendor_harness_esp32s31_semantic::{
    CONTRACTS, RISCV_HARNESS, driver_adapter_evidence_sources, qualification,
    qualify_driver_adapter, qualify_semantic_contract, semantic_contract_evidence_sources,
};
#[cfg(test)]
pub(crate) use open_radio_vendor_harness_esp32s31_semantic::{
    entry_contract, external_abi, wide_signed_divide_target_address,
};
