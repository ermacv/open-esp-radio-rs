//! Typed ESP32-S31 vendor-comparison scenarios.
//!
//! Each scenario authenticates caller-supplied private inputs, drives the
//! `blobray` CLI with requests built from Blobray's own types, and checks
//! independent expectations. Chip addresses and source identities belong to
//! this verification owner; generic Blobray has no chip dependency.
pub mod bluetooth;
pub mod coexistence;
pub mod decisions;
pub mod engine;
pub mod phy;
pub mod run;
pub mod wifi;

// Domain modules keep their crate-root paths.
pub use bluetooth::ble;
pub use coexistence::{coex, coex_hw};
pub use engine::{
    artifacts, contracts, coverage, evidence, harness, harness_edges, layout, observation, session,
    state,
};
pub use phy::{
    calibration_leaves, calibration_prefix, channel, gain, gain_state, i2c, i2c_transport, rfpll,
    rx_gain, tracking, tracking_graph, tx_dc,
};
pub use wifi::{ampdu_resort, low_power_clock, mac, retry, rx_append};

/// The ESP32-S31 facts the scenario engine runs with.
pub static CHIP: oer_vendor_scenario_engine::Chip = oer_vendor_scenario_engine::Chip {
    name: "esp32s31",
    manifest: "verification/esp32s31/artifacts.toml",
    manifest_text: include_str!("../../artifacts.toml"),
    hardware_scope: "crates/hardware/esp32s31",
    shared_scopes: &["crates/hardware/ieee80211"],
    rom_input: layout::ROM_INPUT,
    rom_symbols: &layout::ROM_SYMBOLS,
    stack: (layout::STACK_ADDRESS, layout::STACK_BYTES),
    coverage: "verification/esp32s31/decisions/coverage.toml",
    observation: decisions::observation::DECISIONS,
    state: decisions::state::DECISIONS,
    phy: Some(&layout::PHY_LAYOUT),
    isa: oer_vendor_scenario_engine::Isa::Rv32imafcZbaZbbZbsZcbZcmp,
    registers: "registers/esp32s31/published/radio.bindings.toml",
};

/// Run the scenario engine for the ESP32-S31.
pub fn install() {
    oer_vendor_scenario_engine::install(&CHIP);
}

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/esp32s31/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-esp32s31-probe-radio-elf";
/// Package of the compiled Bluetooth probe image.
pub const BLUETOOTH_PROBES_PACKAGE: &str = "oer-esp32s31-probe-bluetooth-elf";
pub const PROBES_TARGET: &str = "riscv32imafc-unknown-none-elf";

#[cfg(test)]
mod tests {
    use crate::artifacts::{SourceKind, manifest, path, sha256};
    use std::path::Path;

    #[test]
    fn tracked_manifest_pins_every_scenario_input_once() {
        super::install();
        let manifest = manifest();
        for id in ["libphy", "librftest", "libpp", "rom", "sdk", "phy-sdk"] {
            assert_eq!(sha256(id).len(), 64, "{id}");
        }
        let mut ids: Vec<_> = manifest.artifact.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), manifest.artifact.len());
        for source in &manifest.source {
            if source.kind != SourceKind::Local {
                assert!(
                    source.repository.is_some() && source.revision.is_some(),
                    "{}",
                    source.id
                );
            }
            if source.kind == SourceKind::Release {
                assert!(
                    source.asset.is_some() && source.sha256.is_some(),
                    "{}",
                    source.id
                );
            }
        }
    }

    #[test]
    fn fetched_artifacts_live_under_their_source_revision() {
        super::install();
        let root = Path::new("/repo");
        assert_eq!(
            path(root, "libphy"),
            root.join("target/vendor/esp-phy-lib/20f1db053a0e6cb9f1c09d255c43bf42483041d0/esp32s31/libphy.a")
        );
        assert!(path(root, "sdk").starts_with(root.join("target/architecture-research")));
    }
}
