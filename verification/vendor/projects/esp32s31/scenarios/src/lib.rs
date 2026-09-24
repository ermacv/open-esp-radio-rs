//! Typed ESP32-S31 vendor-comparison scenarios.
//!
//! Each scenario authenticates caller-supplied private inputs, drives the
//! `blobray` CLI with requests built from Blobray's own types, and checks
//! independent expectations. Chip addresses and source identities belong to
//! this verification owner; generic Blobray has no chip dependency.
pub mod calibration_leaves;
pub mod calibration_prefix;
pub mod evidence;
pub mod gain;
pub mod gain_state;
pub mod harness;
pub mod harness_edges;
pub mod i2c;
pub mod i2c_transport;
pub mod rfpll;
pub mod session;

/// Pinned `libphy.a` archive (ESP-IDF PHY source revision `b88e4b76`).
pub const I2C_LIBRARY_SHA: &str =
    "d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580";
/// Pinned ESP32-S31 revision-0 ROM ELF.
pub const ROM_SHA: &str = "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542";
