#![no_std]
#![forbid(unsafe_code)]

//! Hardware-independent Bluetooth Low Energy Link Layer building blocks.
//!
//! This crate owns bounded over-the-air PDU codecs and protocol-role state.
//! It has no HCI, MMIO, DMA, interrupt, executor, allocator, ESP32-S31,
//! vendor-archive, or ROM-ABI dependency. Chip code lowers prepared protocol
//! work into its private descriptor and register accessors; an HCI router may
//! configure roles only after those lower ownership boundaries exist.
//!
//! [`connection::maintenance`] admits provisional budgeted event omissions on
//! the same connection owner, retaining initial acknowledgement, Instant and
//! post-maintenance recovery obligations. This protocol decision grants no
//! hardware access or timing budget; both remain with the lower radio owner.

#[cfg(test)]
extern crate std;

mod address;
pub mod advertiser;
pub mod advertising;
pub mod advertising_lifecycle;
pub mod connectable_advertising;
pub mod connection;
pub mod control;
pub mod dtm;
pub mod scanning;
pub mod security;

pub use address::{LeDeviceAddress, LeDeviceAddressKind};
