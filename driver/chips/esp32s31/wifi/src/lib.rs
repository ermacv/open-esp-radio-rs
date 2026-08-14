#![no_std]
#![forbid(unsafe_code)]

//! Role-neutral ESP32-S31 Wi-Fi device composition.
//!
//! This layer joins shared RF/PHY and Wi-Fi hardware ownership before a
//! station, access point or standalone monitor role is materialized. It does
//! not own MLME policy, an executor, a network stack or board resources.

#[cfg(any(target_arch = "riscv32", test))]
mod channel;
#[cfg(target_arch = "riscv32")]
pub use channel::switch_esp32s31_wifi_channel;
#[cfg(target_arch = "riscv32")]
pub mod cold_start;
#[cfg(target_arch = "riscv32")]
pub mod mac_start;
pub mod ordinary_tx;
#[cfg(target_arch = "riscv32")]
pub mod runtime;
pub mod tx;
