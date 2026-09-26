//! Typed ESP32-S31 vendor-comparison scenarios.
//!
//! Each scenario authenticates caller-supplied private inputs, drives the
//! `blobray` CLI with requests built from Blobray's own types, and checks
//! independent expectations. Chip addresses and source identities belong to
//! this verification owner; generic Blobray has no chip dependency.
pub mod artifacts;
pub mod calibration_leaves;
pub mod calibration_prefix;
pub mod channel;
pub mod contracts;
pub mod coverage;
pub mod evidence;
pub mod gain;
pub mod gain_state;
pub mod harness;
pub mod harness_edges;
pub mod i2c;
pub mod i2c_transport;
pub mod layout;
pub mod mac;
pub mod observation;
pub mod phy;
pub mod research;
pub mod rfpll;
pub mod rx_gain;
pub mod session;
pub mod setup_cache;
pub mod state;
pub mod tracking;
pub mod tracking_graph;
pub mod tx_dc;

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/vendor/projects/esp32s31/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-esp32s31-probe-radio-elf";
pub const PROBES_TARGET: &str = "riscv32imafc-unknown-none-elf";
