#![no_std]
#![deny(unsafe_code)]

//! Audited ESP32-S31 Wi-Fi MAC DMA ownership boundary.
//!
//! This crate owns the chip descriptor geometry, finite RX walker operations,
//! live ring state and permanently located RX arena. Protocol decoding and
//! executor policy deliberately remain above this leaf.

pub mod descriptor;
pub mod rx_dma;
pub mod rx_ring;
pub mod rx_storage;
