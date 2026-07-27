#![no_std]

//! Hardware-independent IEEE 802.11 protocol building blocks.
//!
//! This crate owns bounded frame parsing and protocol state only. It has no
//! MMIO, DMA, interrupt, executor, allocator, ESP32-S31, vendor archive, or
//! ROM ABI dependency.

pub mod management;
pub mod scan;
