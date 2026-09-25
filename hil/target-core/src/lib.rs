//! Chip-independent HIL target logic.
//!
//! The ESP32-S31 HIL runtime composes these modules with its radio, executor
//! and console; host tests exercise the same compiled code. Network modules
//! use the stack selected by exactly one of the `upstream-network`,
//! `embassy-network` and `owned-network` features, as the runtime does.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(any(
    all(feature = "upstream-network", feature = "embassy-network"),
    all(feature = "upstream-network", feature = "owned-network"),
    all(feature = "embassy-network", feature = "owned-network"),
))]
compile_error!("select exactly one HIL network stack feature");

#[cfg(feature = "owned-network")]
extern crate embassy_net_owned as embassy_net;
#[cfg(feature = "embassy-network")]
extern crate embassy_net_released as embassy_net;
#[cfg(feature = "upstream-network")]
extern crate embassy_net_upstream as embassy_net;

#[cfg(feature = "bluetooth")]
pub mod bluetooth;
#[cfg(feature = "secure-gatt")]
pub mod bluetooth_gatt;
pub mod console;
pub mod memory_benchmark;
pub mod network;
pub mod traffic;
