#![no_std]
#![forbid(unsafe_code)]

//! Source-owned ESP32-S31 Wi-Fi MAC building blocks.
//!
//! This crate intentionally contains no ESP-IDF ABI, vendor archive, allocator,
//! executor, or `esp-hal` dependency. Target code supplies volatile MMIO and
//! owns the interrupt binding; the state machines here stay host-testable.

#[cfg(test)]
extern crate std;

pub use open_esp_radio_esp32s31_hal::types::{
    MacInterface, MacPti, MacRoleReceivePolicy, MacStaApReceivePlan, MacStaPolicyMode,
};

pub mod ap_policy;
pub mod ap_tsf;
pub mod capabilities;
pub mod channel_state;
pub mod coex_runtime;
mod cold;
pub mod crypto;
pub mod edca;
pub mod he;
pub mod init;
mod interface_address;
pub mod irq;
pub mod rx;
mod sniffer;
pub mod sta_ap_registers;
mod sta_link_policy;
pub mod tx;

/// Rate selection, receive observations and scheduling policy.
pub mod rate;

// Preserve the original public module paths for downstream users.
pub use rate::control as rate_control;
pub use rate::low as low_rate;
pub use rate::rx as rate_rx;
pub use rate::schedule as rate_schedule;
pub use rx::ampdu as rx_ampdu;
pub use rx::hardware as rx_ampdu_hw;
pub use rx::pool as rx_pool;
pub use tx::ampdu as tx_ampdu;
pub use tx::metadata as tx_metadata;
pub use tx::policy as tx_policy;
pub use tx::protection as tx_protection;
pub use tx::runtime as tx_runtime;
