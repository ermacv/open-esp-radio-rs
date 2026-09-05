#![no_std]
#![deny(unsafe_code)]

//! Embassy-specific composition for the ESP32-S31 Wi-Fi backend.
//!
//! Core radio, PHY and MAC crates remain executor-neutral. This crate owns
//! wakeups, cooperative async TX access and the bridge from pinned network
//! frames to ESP32-S31 Wi-Fi DMA.

#[cfg(test)]
extern crate std;

pub mod composition;
pub mod datapath;
pub mod diagnostics;
pub mod roles;
