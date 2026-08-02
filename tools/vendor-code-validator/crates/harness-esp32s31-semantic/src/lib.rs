//! ESP32-S31 reviewed summaries and typed production-driver qualification.
//!
//! This is a platform harness implementation. Chip addresses, PHY state
//! projections and the production driver dependency belong here rather than
//! in the validator facade or architecture backend.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact, codegen, execution,
};
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_validator_semantic::*;

pub mod qualification;
mod reviewed_summaries;

const RISCV_SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |target| target == wide_signed_divide_target_address(),
    reference_intrinsic: reviewed_summaries::reference_intrinsic_trace,
    standard_memory_intrinsic: reviewed_summaries::standard_memory_intrinsic_trace,
    wide_signed_divide: reviewed_summaries::wide_signed_divide_intrinsic,
};

pub static RISCV_HARNESS: RiscvHarnessSpec = RiscvHarnessSpec {
    contracts: &CONTRACTS,
    summaries: &RISCV_SUMMARIES,
};

pub const fn wide_signed_divide_target_address() -> u32 {
    0x2f81_ce6e
}

pub fn qualify_driver_adapter(
    request: &DriverAdapterRequest<'_>,
) -> Result<Option<DriverAdapterQualification>> {
    let id = request.id;
    let source = request.source;
    let vendor_symbol = request.vendor_symbol;
    if id != "esp32s31-iq-est-enable-v1" {
        return Ok(None);
    }
    if source != "rom" || vendor_symbol != "phy_iq_est_enable" {
        return Err(format!("driver adapter {id} cannot qualify {source} {vendor_symbol}").into());
    }
    qualification::qualify_esp32s31_iq_est_enable(
        request.svd,
        request.vendor_artifact,
        request.vendor_companion,
        request.rust_artifact,
        request.rust_companion,
        request.rust_symbol,
        request.policy,
        false,
    )
    .map(Some)
}

pub fn qualify_semantic_contract(request: &SemanticContractRequest<'_>) -> Result<Option<bool>> {
    let id = request.id;
    let source = request.source;
    let vendor_symbol = request.vendor_symbol;
    let expected = match id {
        "esp32s31-channel" => ("archive", "phy_chip_set_chan"),
        "esp32s31-rf-init" => ("archive", "phy_rf_init"),
        "esp32s31-bluetooth-txdc" => ("archive", "phy_bt_txdc_cal_new"),
        "esp32s31-bluetooth-txdc-pwdet" => ("archive", "phy_txdc_cal_pwdet_init"),
        "esp32s31-bluetooth-tx-power" => ("archive", "phy_bt_tx_pwctrl_init"),
        _ => return Ok(None),
    };
    if (source, vendor_symbol) != expected {
        return Err(
            format!("semantic contract {id} cannot qualify {source} {vendor_symbol}").into(),
        );
    }
    let companion = request
        .vendor_companion
        .ok_or_else(|| format!("semantic contract {id} requires an archive companion"))?;
    let matched = match id {
        "esp32s31-channel" => qualification::qualify_esp32s31_channel(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-rf-init" => qualification::qualify_esp32s31_rf_init(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-txdc" => qualification::qualify_esp32s31_bluetooth_txdc(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-txdc-pwdet" => qualification::qualify_esp32s31_bluetooth_txdc_pwdet(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-tx-power" => qualification::qualify_esp32s31_bluetooth_tx_power(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        _ => unreachable!("registered contract was matched above"),
    };
    Ok(Some(matched))
}

const DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/iq_estimator.rs",
        contents: include_str!("qualification/iq_estimator.rs"),
    },
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
];

const SEMANTIC_COMMON_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
    EvidenceSource {
        name: "qualification/state.rs",
        contents: include_str!("qualification/state.rs"),
    },
    EvidenceSource {
        name: "qualification/runner.rs",
        contents: include_str!("qualification/runner.rs"),
    },
];

pub fn driver_adapter_evidence_sources(id: &str) -> Option<DriverAdapterEvidenceSources> {
    (id == "esp32s31-iq-est-enable-v1").then_some(DriverAdapterEvidenceSources {
        adapter: DRIVER_ADAPTER_SOURCES,
        reviewed_summary: EvidenceSource {
            name: "reviewed_summaries.rs",
            contents: include_str!("reviewed_summaries.rs"),
        },
    })
}

pub fn semantic_contract_evidence_sources(id: &str) -> Option<SemanticContractEvidenceSources> {
    let contract = match id {
        "esp32s31-channel" => EvidenceSource {
            name: "qualification/channel.rs",
            contents: include_str!("qualification/channel.rs"),
        },
        "esp32s31-rf-init" => EvidenceSource {
            name: "qualification/rf_init.rs",
            contents: include_str!("qualification/rf_init.rs"),
        },
        "esp32s31-bluetooth-txdc" => EvidenceSource {
            name: "qualification/bluetooth_txdc.rs",
            contents: include_str!("qualification/bluetooth_txdc.rs"),
        },
        "esp32s31-bluetooth-txdc-pwdet" => EvidenceSource {
            name: "qualification/bluetooth_txdc_pwdet.rs",
            contents: include_str!("qualification/bluetooth_txdc_pwdet.rs"),
        },
        "esp32s31-bluetooth-tx-power" => EvidenceSource {
            name: "qualification/bluetooth_tx_power.rs",
            contents: include_str!("qualification/bluetooth_tx_power.rs"),
        },
        _ => return None,
    };
    Some(SemanticContractEvidenceSources {
        common: SEMANTIC_COMMON_SOURCES,
        contract,
    })
}

pub fn artifact_sha256(path: &Path) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

pub fn seed_ram_word(scenario: &mut execution::Scenario, address: u32, value: u32) {
    write_ram_word(scenario, address, value);
    scenario.observed_memory.push(execution::MemoryRange {
        start: address,
        length: 4,
    });
}

pub fn write_ram_word(scenario: &mut execution::Scenario, address: u32, value: u32) {
    for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
        scenario
            .memory_initial
            .insert(address.wrapping_add(offset as u32), byte);
    }
}

pub fn unmapped_execution_address(event: &execution::ExecutionEvent) -> Option<u32> {
    match event {
        execution::ExecutionEvent::Read {
            address, register, ..
        }
        | execution::ExecutionEvent::Write {
            address, register, ..
        } if register == "UNMAPPED" => Some(*address),
        _ => None,
    }
}
