//! Chip-independent HIL target logic.
//!
//! The ESP32-S31 HIL runtime composes these modules with its radio, executor
//! and console; host tests exercise the same compiled code. Network modules
//! use the owned Xarxa stack selected by the `owned-network` feature, as the
//! runtime does.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(feature = "owned-network")]
extern crate embassy_net_owned as embassy_net;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;
#[cfg(feature = "secure-gatt")]
pub mod bluetooth_gatt;
pub mod console;
pub mod liveness;
pub mod memory_benchmark;
pub mod network;
pub mod postmortem;
pub mod trace;
pub mod traffic;
