//! Typed ESP32-S31 vendor-comparison scenarios.
//!
//! Each scenario authenticates caller-supplied private inputs, drives the
//! `blobray` CLI with requests built from Blobray's own types, and checks
//! independent expectations. Chip addresses and source identities belong to
//! this verification owner; generic Blobray has no chip dependency.
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
pub mod mutation;
pub mod mutation_campaign;
pub mod phy;
pub mod research;
pub mod rfpll;
pub mod rx_gain;
pub mod session;
pub mod setup_cache;
pub mod tracking;
pub mod tracking_graph;
pub mod tx_dc;

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/vendor/projects/esp32s31/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-verification-esp32s31-probes-elf";
pub const PROBES_TARGET: &str = "riscv32imafc-unknown-none-elf";

/// Pinned `libphy.a` archive (ESP-IDF PHY source revision `b88e4b76`).
pub const I2C_LIBRARY_SHA: &str =
    "d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580";
/// Pinned ESP32-S31 revision-0 ROM ELF.
pub const ROM_SHA: &str = "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542";
