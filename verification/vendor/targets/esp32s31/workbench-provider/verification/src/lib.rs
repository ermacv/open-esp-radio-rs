//! ESP32-S31 typed production-driver verification.
//!
//! This provider may depend on production code because its only responsibility
//! is comparison. Reusable chip lifting knowledge lives in the sibling
//! `knowledge` crate.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

pub use open_radio_vendor_backend_riscv::{
    ReferenceResolver, RiscvHarnessSpec, RiscvSummaryHooks, Rv32CallArguments,
    StructuralPointerContext, artifact, codegen, execution,
};
pub use open_radio_vendor_execution_model as execution_model;
pub use open_radio_vendor_harness_esp32s31::{CONTRACTS, entry_contract, external_abi};
pub use open_radio_vendor_knowledge_esp32s31::{RISCV_HARNESS, wide_signed_divide_target_address};
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

    #[error("verification case {case:?} failed during {phase}")]
    VerificationCase {
        case: String,
        phase: String,
        #[source]
        source: open_radio_vendor_backend_riscv::Error,
    },
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

#[path = "semantic_replay/mod.rs"]
pub mod verification;

/// Reviewed trust boundary for every compiled driver adapter.
///
/// Keep this registry presentation-neutral: the generic verifier uses the
/// same value both for its claim ceiling and for `project audit bindings`.
pub fn driver_adapter_trust(id: &str) -> Option<DriverAdapterTrust> {
    use DriverAdapterDomain::ReviewedDomain;
    use DriverAdapterRelation::{Conformance, Refinement};
    use RustBindingKind::{ExactProductionEntry, SharedProductionCore};
    use VendorOracleKind::ConcreteReplay;

    let trust = match id {
        "esp32s31-iq-est-enable-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: SharedProductionCore,
            domain: ReviewedDomain,
            relation: Refinement,
        },
        "esp32s31-wdev-process-fiq-mac-slice-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: ExactProductionEntry,
            domain: ReviewedDomain,
            relation: Refinement,
        },
        "esp32s31-hal-mac-txq-owned-publication-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: ExactProductionEntry,
            domain: ReviewedDomain,
            relation: Refinement,
        },
        "esp32s31-wdev-append-rx-blocks-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: ExactProductionEntry,
            domain: ReviewedDomain,
            relation: Refinement,
        },
        "esp32s31-sta-join-state-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: ExactProductionEntry,
            domain: ReviewedDomain,
            relation: Refinement,
        },
        "esp32s31-wifi-key-role-v1" => DriverAdapterTrust {
            vendor: ConcreteReplay,
            rust: ExactProductionEntry,
            domain: ReviewedDomain,
            relation: Conformance,
        },
        _ => return None,
    };
    Some(trust)
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
            )
            .map(Some)
        }
        "esp32s31-hal-mac-txq-owned-publication-v1" => {
            if source != "libpp" || vendor_symbol != "hal_mac_txq_enable" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            let replay = request
                .auxiliary_artifacts
                .iter()
                .find(|artifact| artifact.id == "libpp-replay")
                .ok_or("TX publication verification requires auxiliary artifact libpp-replay")?;
            verification::verify_esp32s31_hal_mac_txq_owned_publication(
                request.svd,
                request.vendor_inventory,
                request.vendor_artifact,
                request.vendor_companion,
                replay.artifact,
                request.rust_artifact,
                request.rust_companion,
                request.rust_symbol,
                request.policy,
            )
            .map(Some)
        }
        "esp32s31-wdev-append-rx-blocks-v1" => {
            if source != "libpp" || vendor_symbol != "wDev_AppendRxBlocks" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            let replay = request
                .auxiliary_artifacts
                .iter()
                .find(|artifact| artifact.id == "libpp-replay")
                .ok_or("RX append verification requires auxiliary artifact libpp-replay")?;
            verification::verify_esp32s31_wdev_append_rx_blocks(
                request.svd,
                request.vendor_inventory,
                request.vendor_artifact,
                request.vendor_companion,
                replay.artifact,
                request.rust_artifact,
                request.rust_companion,
                request.rust_symbol,
                request.policy,
            )
            .map(Some)
        }
        "esp32s31-sta-join-state-v1" => {
            if source != "libnet80211" || vendor_symbol != "ieee80211_sta_new_state" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            let replay = request
                .auxiliary_artifacts
                .iter()
                .find(|artifact| artifact.id == "wifi-key-role")
                .ok_or("STA join verification requires auxiliary artifact wifi-key-role")?;
            verification::verify_esp32s31_sta_join_state(
                request.svd,
                request.vendor_inventory,
                request.vendor_artifact,
                request.vendor_companion,
                replay.artifact,
                request.rust_artifact,
                request.rust_companion,
                request.rust_symbol,
                request.policy,
            )
            .map(Some)
        }
        "esp32s31-wifi-key-role-v1" => {
            if source != "wifi-key-role" || vendor_symbol != "wDev_Insert_KeyEntry" {
                return Err(
                    format!("driver adapter {id} cannot verify {source} {vendor_symbol}").into(),
                );
            }
            verification::verify_esp32s31_wifi_key_role(request).map(Some)
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
        "esp32s31-bluetooth-tx-gain-init" => ("archive", "phy_bt_tx_gain_init"),
        "esp32s31-baseband-init" => ("archive", "phy_bb_init"),
        "esp32s31-register-init" => ("archive", "register_chipv7_phy"),
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
        "esp32s31-channel" => {
            verification::verify_esp32s31_channel(request.svd, request.vendor_artifact, companion)?
                .matched
        }
        "esp32s31-rf-init" => {
            verification::verify_esp32s31_rf_init(request.svd, request.vendor_artifact, companion)?
                .matched
        }
        "esp32s31-bluetooth-txdc" => {
            verification::verify_esp32s31_bluetooth_txdc(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        "esp32s31-bluetooth-txdc-pwdet" => {
            verification::verify_esp32s31_bluetooth_txdc_pwdet(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        "esp32s31-bluetooth-tx-power" => {
            verification::verify_esp32s31_bluetooth_tx_power(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        "esp32s31-bluetooth-tx-gain-init" => {
            verification::verify_esp32s31_bluetooth_tx_gain_init(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        "esp32s31-baseband-init" => {
            verification::verify_esp32s31_baseband_init(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        "esp32s31-register-init" => {
            verification::verify_esp32s31_register_init(
                request.svd,
                request.vendor_artifact,
                companion,
            )?
            .matched
        }
        _ => unreachable!("registered contract was matched above"),
    };
    Ok(Some(matched))
}

const IQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/iq_estimator.rs",
        contents: include_str!("semantic_replay/iq_estimator.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const WDEV_PROCESS_FIQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/wdev_process_fiq.rs",
        contents: include_str!("semantic_replay/wdev_process_fiq.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const HAL_MAC_TXQ_ENABLE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/hal_mac_txq_enable.rs",
        contents: include_str!("semantic_replay/hal_mac_txq_enable.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const WDEV_APPEND_RX_BLOCKS_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/wdev_append_rx_blocks.rs",
        contents: include_str!("semantic_replay/wdev_append_rx_blocks.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const STA_JOIN_STATE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/sta_join_state.rs",
        contents: include_str!("semantic_replay/sta_join_state.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const WIFI_KEY_ROLE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/wifi_key_role.rs",
        contents: include_str!("semantic_replay/wifi_key_role.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
];

const SEMANTIC_COMMON_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "semantic_replay/mod.rs",
        contents: include_str!("semantic_replay/mod.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/state.rs",
        contents: include_str!("semantic_replay/state.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/runner.rs",
        contents: include_str!("semantic_replay/runner.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/runner/parents.rs",
        contents: include_str!("semantic_replay/runner/parents.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/bb_init/environment.rs",
        contents: include_str!("semantic_replay/bb_init/environment.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion.rs",
        contents: include_str!("semantic_replay/completion.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion/common.rs",
        contents: include_str!("semantic_replay/completion/common.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion/parent.rs",
        contents: include_str!("semantic_replay/completion/parent.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion/rx_gain.rs",
        contents: include_str!("semantic_replay/completion/rx_gain.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion/rx_iq.rs",
        contents: include_str!("semantic_replay/completion/rx_iq.rs"),
    },
    EvidenceSource {
        name: "semantic_replay/completion/tx.rs",
        contents: include_str!("semantic_replay/completion/tx.rs"),
    },
];

pub fn driver_adapter_evidence_sources(id: &str) -> Option<DriverAdapterEvidenceSources> {
    let adapter = match id {
        "esp32s31-iq-est-enable-v1" => IQ_DRIVER_ADAPTER_SOURCES,
        "esp32s31-wdev-process-fiq-mac-slice-v1" => WDEV_PROCESS_FIQ_DRIVER_ADAPTER_SOURCES,
        "esp32s31-hal-mac-txq-owned-publication-v1" => HAL_MAC_TXQ_ENABLE_DRIVER_ADAPTER_SOURCES,
        "esp32s31-wdev-append-rx-blocks-v1" => WDEV_APPEND_RX_BLOCKS_DRIVER_ADAPTER_SOURCES,
        "esp32s31-sta-join-state-v1" => STA_JOIN_STATE_DRIVER_ADAPTER_SOURCES,
        "esp32s31-wifi-key-role-v1" => WIFI_KEY_ROLE_DRIVER_ADAPTER_SOURCES,
        _ => return None,
    };
    Some(DriverAdapterEvidenceSources {
        adapter,
        reviewed_summary: open_radio_vendor_knowledge_esp32s31::REVIEWED_SUMMARY_EVIDENCE_SOURCE,
        trust: driver_adapter_trust(id).expect("registered adapter has a trust boundary"),
    })
}

pub fn semantic_contract_evidence_sources(id: &str) -> Option<SemanticContractEvidenceSources> {
    let contract = match id {
        "esp32s31-channel" => EvidenceSource {
            name: "semantic_replay/channel.rs",
            contents: include_str!("semantic_replay/channel.rs"),
        },
        "esp32s31-rf-init" => EvidenceSource {
            name: "semantic_replay/rf_init.rs",
            contents: include_str!("semantic_replay/rf_init.rs"),
        },
        "esp32s31-bluetooth-txdc" => EvidenceSource {
            name: "semantic_replay/bluetooth_txdc.rs",
            contents: include_str!("semantic_replay/bluetooth_txdc.rs"),
        },
        "esp32s31-bluetooth-txdc-pwdet" => EvidenceSource {
            name: "semantic_replay/bluetooth_txdc_pwdet.rs",
            contents: include_str!("semantic_replay/bluetooth_txdc_pwdet.rs"),
        },
        "esp32s31-bluetooth-tx-power" => EvidenceSource {
            name: "semantic_replay/bluetooth_tx_power.rs",
            contents: include_str!("semantic_replay/bluetooth_tx_power.rs"),
        },
        "esp32s31-bluetooth-tx-gain-init" => EvidenceSource {
            name: "semantic_replay/bluetooth_tx_gain.rs",
            contents: include_str!("semantic_replay/bluetooth_tx_gain.rs"),
        },
        "esp32s31-baseband-init" => EvidenceSource {
            name: "semantic_replay/bb_init.rs",
            contents: include_str!("semantic_replay/bb_init.rs"),
        },
        "esp32s31-register-init" => EvidenceSource {
            name: "semantic_replay/register_init.rs",
            contents: include_str!("semantic_replay/register_init.rs"),
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
    scenario.observed_memory.push(execution_model::MemoryRange {
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
