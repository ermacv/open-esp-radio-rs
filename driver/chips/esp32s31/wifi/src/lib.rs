#![no_std]
#![forbid(unsafe_code)]

//! Role-neutral ESP32-S31 Wi-Fi device composition.
//!
//! This layer joins shared RF/PHY and Wi-Fi hardware ownership before a
//! station, access point or standalone monitor role is materialized. It does
//! not own MLME policy, an executor, a network stack or board resources.

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod channel;
#[cfg(target_arch = "riscv32")]
pub use channel::{Esp32s31PhyChannel, lower_wifi_channel, switch_esp32s31_wifi_channel};
pub mod ampdu_tx;
#[cfg(target_arch = "riscv32")]
pub mod cold_start;
pub mod cooperative_hardware;
pub mod datapath;
pub mod esp_now;
#[cfg(target_arch = "riscv32")]
pub mod mac_start;
pub mod ordinary_tx;
pub mod protected_data_rx;
#[cfg(target_arch = "riscv32")]
pub mod runtime;
pub mod tx;
