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
pub mod wifi;

// Domain modules keep their crate-root paths.
pub use bluetooth::ble;
pub use coexistence::{coex, coex_hw};
pub use engine::{
    artifacts, contracts, coverage, evidence, harness, harness_edges, layout, observation, session,
    setup_cache, state,
};
pub use phy::{
    calibration_leaves, calibration_prefix, channel, gain, gain_state, i2c, i2c_transport,
    research, rfpll, rx_gain, tracking, tracking_graph, tx_dc,
};
pub use wifi::{mac, retry};

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/esp32s31/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-esp32s31-probe-radio-elf";
/// Package of the compiled Bluetooth probe image.
pub const BLUETOOTH_PROBES_PACKAGE: &str = "oer-esp32s31-probe-bluetooth-elf";
pub const PROBES_TARGET: &str = "riscv32imafc-unknown-none-elf";
