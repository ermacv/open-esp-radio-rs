#![no_std]

//! Source-owned ESP32-S31 Wi-Fi MAC building blocks.
//!
//! This crate intentionally contains no ESP-IDF ABI, vendor archive, allocator,
//! executor, or `esp-hal` dependency. Target code supplies volatile MMIO and
//! owns the interrupt binding; the state machines here stay host-testable.

#[cfg(test)]
extern crate std;

mod cold_antenna;
mod cold_coex;
mod cold_crypto;
mod cold_enable;
mod cold_hal_tail;
mod cold_handshake;
mod cold_he;
mod cold_last_rx_buffer;
mod cold_rx_buffer;
mod cold_rx_policy;
mod cold_txrx;
pub mod crypto;
pub mod descriptor;
pub mod he;
pub mod init;
mod interface_address;
pub mod irq;
mod low_rate;
pub mod rate_control;
pub mod rate_rx;
pub mod rate_schedule;
pub mod registers;
pub mod rx;
pub mod rx_ampdu;
pub mod rx_ampdu_hw;
mod sniffer;
mod sta_link_policy;
pub mod tx;
pub mod tx_ampdu;
pub mod tx_plcp;
pub mod tx_policy;
pub mod tx_queue;

// Preserve the qualified `mac::scan` path while the protocol owner moves to
// its hardware-independent crate.
pub use open_esp_radio_ieee80211::scan;
