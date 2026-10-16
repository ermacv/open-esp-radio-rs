//! Chip-independent HIL target logic.
//!
//! The ESP32-S31 HIL runtime composes these modules with its radio, executor
//! and console; host tests exercise the same compiled code.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod base;
#[cfg(feature = "secure-gatt")]
pub mod bluetooth_gatt;
pub mod console;
pub mod liveness;
pub mod postmortem;
pub mod profile;
pub mod sample_request;

#[doc(hidden)]
pub use oer_hil_protocol as __protocol;
