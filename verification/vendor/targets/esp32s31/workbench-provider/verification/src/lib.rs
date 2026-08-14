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

#[path = "production_adapter/mod.rs"]
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
    let _ = request;
    Ok(None)
}

const IQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/iq_estimator.rs",
        contents: include_str!("production_adapter/iq_estimator.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
    },
];

const WDEV_PROCESS_FIQ_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/wdev_process_fiq.rs",
        contents: include_str!("production_adapter/wdev_process_fiq.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
    },
];

const HAL_MAC_TXQ_ENABLE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/hal_mac_txq_enable.rs",
        contents: include_str!("production_adapter/hal_mac_txq_enable.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
    },
];

const WDEV_APPEND_RX_BLOCKS_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/wdev_append_rx_blocks.rs",
        contents: include_str!("production_adapter/wdev_append_rx_blocks.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
    },
];

const STA_JOIN_STATE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/sta_join_state.rs",
        contents: include_str!("production_adapter/sta_join_state.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
    },
];

const WIFI_KEY_ROLE_DRIVER_ADAPTER_SOURCES: &[EvidenceSource] = &[
    EvidenceSource {
        name: "production_adapter/wifi_key_role.rs",
        contents: include_str!("production_adapter/wifi_key_role.rs"),
    },
    EvidenceSource {
        name: "production_adapter/mod.rs",
        contents: include_str!("production_adapter/mod.rs"),
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
    let _ = id;
    None
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
