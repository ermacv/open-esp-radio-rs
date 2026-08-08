//! ESP32-S31 reviewed summaries and typed production-driver verification.
//!
//! This is a platform harness implementation. Chip addresses, PHY state
//! projections and the production driver dependency belong here rather than
//! in the workbench facade or architecture backend.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact, codegen, execution,
};
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_semantics::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Semantics(#[from] open_radio_vendor_semantics::Error),

    #[error(transparent)]
    RiscvBackend(#[from] open_radio_vendor_backend_riscv::Error),
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// Keep the physical directory stable because its paths are part of existing
// evidence identities. The public module boundary describes what it does.
mod reviewed_summaries;
#[path = "qualification/mod.rs"]
pub mod verification;

const RISCV_SUMMARIES: RiscvSummaryHooks = RiscvSummaryHooks {
    secondary_return_target: |target| target == wide_signed_divide_target_address(),
    direct_semantic: reviewed_summaries::direct_semantic_function,
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

pub fn verify_driver_adapter(
    request: &DriverAdapterRequest<'_>,
) -> Result<Option<DriverAdapterVerification>> {
    let id = request.id;
    let source = request.source;
    let vendor_symbol = request.vendor_symbol;
    match id {
        "esp32s31-iq-est-enable-v1" => {
            if source != "rom" || vendor_symbol != "phy_iq_est_enable" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_iq_est_enable(
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
        "esp32s31-wdev-process-fiq-mac-slice-v1" => {
            if source != "libpp" || vendor_symbol != "wDev_ProcessFiq" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_wdev_process_fiq_mac_slice(
                request.svd,
                request.vendor_inventory,
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
        "esp32s31-hal-mac-txq-enable-register-slice-v1" => {
            if source != "libpp" || vendor_symbol != "hal_mac_txq_enable" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_hal_mac_txq_enable_register_slice(
                request.svd,
                request.vendor_inventory,
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
        "esp32s31-wdev-append-rx-blocks-v1" => {
            if source != "libpp" || vendor_symbol != "wDev_AppendRxBlocks" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_wdev_append_rx_blocks(
                request.svd,
                request.vendor_inventory,
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
        "esp32s31-sta-join-state-v1" => {
            if source != "libnet80211" || vendor_symbol != "ieee80211_sta_new_state" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_sta_join_state(
                request.svd,
                request.vendor_inventory,
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
        _ => Ok(None),
    }
}

pub fn verify_semantic_contract(request: &SemanticContractRequest<'_>) -> Result<Option<bool>> {
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
            format!("semantic contract {id} cannot verify {source} {vendor_symbol}").into(),
        );
    }
    let companion = request
        .vendor_companion
        .ok_or_else(|| format!("semantic contract {id} requires an archive companion"))?;
    let matched = match id {
        "esp32s31-channel" => verification::verify_esp32s31_channel(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-rf-init" => verification::verify_esp32s31_rf_init(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-txdc" => verification::verify_esp32s31_bluetooth_txdc(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-txdc-pwdet" => verification::verify_esp32s31_bluetooth_txdc_pwdet(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        "esp32s31-bluetooth-tx-power" => verification::verify_esp32s31_bluetooth_tx_power(
            request.svd,
            request.vendor_artifact,
            companion,
            false,
        )?,
        _ => unreachable!("registered contract was matched above"),
    };
    Ok(Some(matched))
}

const IQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/iq_estimator.rs",
        contents: include_str!("qualification/iq_estimator.rs"),
    },
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
];

const WDEV_PROCESS_FIQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/wdev_process_fiq.rs",
        contents: include_str!("qualification/wdev_process_fiq.rs"),
    },
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
];

const HAL_MAC_TXQ_ENABLE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/hal_mac_txq_enable.rs",
        contents: include_str!("qualification/hal_mac_txq_enable.rs"),
    },
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
];

const WDEV_APPEND_RX_BLOCKS_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/wdev_append_rx_blocks.rs",
        contents: include_str!("qualification/wdev_append_rx_blocks.rs"),
    },
    EvidenceSource {
        name: "qualification/mod.rs",
        contents: include_str!("qualification/mod.rs"),
    },
];

const STA_JOIN_STATE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "qualification/sta_join_state.rs",
        contents: include_str!("qualification/sta_join_state.rs"),
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
    let adapter = match id {
        "esp32s31-iq-est-enable-v1" => IQ_DRIVER_ADAPTER_SOURCES,
        "esp32s31-wdev-process-fiq-mac-slice-v1" => WDEV_PROCESS_FIQ_DRIVER_ADAPTER_SOURCES,
        "esp32s31-hal-mac-txq-enable-register-slice-v1" => {
            HAL_MAC_TXQ_ENABLE_DRIVER_ADAPTER_SOURCES
        }
        "esp32s31-wdev-append-rx-blocks-v1" => WDEV_APPEND_RX_BLOCKS_DRIVER_ADAPTER_SOURCES,
        "esp32s31-sta-join-state-v1" => STA_JOIN_STATE_DRIVER_ADAPTER_SOURCES,
        _ => return None,
    };
    Some(DriverAdapterEvidenceSources {
        adapter,
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
