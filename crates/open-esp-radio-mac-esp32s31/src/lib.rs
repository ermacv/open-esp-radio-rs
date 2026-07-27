#![no_std]

//! Source-owned ESP32-S31 Wi-Fi MAC building blocks.
//!
//! This crate intentionally contains no ESP-IDF ABI, vendor archive, allocator,
//! executor, or `esp-hal` dependency. Target code supplies volatile MMIO and
//! owns the interrupt binding; the state machines here stay host-testable.

pub mod crypto;
pub mod descriptor;
pub mod init;
pub mod irq;
pub mod registers;
pub mod rx;
pub mod tx;
pub mod tx_plcp;

// Preserve the qualified `mac::scan` path while the protocol owner moves to
// its hardware-independent crate.
pub use open_esp_radio_ieee80211::scan;
