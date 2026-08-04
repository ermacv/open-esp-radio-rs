#![no_std]

//! Embassy-specific composition for the ESP32-S31 Wi-Fi backend.
//!
//! Core radio, PHY and MAC crates remain executor-neutral. This crate owns
//! wakeups, cooperative async TX access and the bridge from pinned network
//! frames to ESP32-S31 Wi-Fi DMA.

#[cfg(test)]
extern crate std;

pub mod aggregate_tx;
pub mod backend;
pub mod connected_control;
pub mod control_tx;
pub mod cooperative_tx;
pub mod embassy_irq;
pub mod embassy_rx;
pub mod embassy_tx;
pub mod link_monitor;
mod ordinary_tx;
pub mod preconnected_rx;
pub mod runner;
pub mod running_scan;
pub mod rx_backend;
pub mod rx_reorder;
pub mod rx_telemetry;
pub mod single_mpdu_tx;
pub mod sta_join;
pub mod sta_join_port;
pub mod sta_scan;
#[cfg(target_arch = "riscv32")]
pub mod sta_scan_target;
pub mod staged_rx;
pub mod station_epoch;
pub mod station_power_save;
pub mod wpa2;

/// Chip-independent outer station lifecycle contracts used by this adapter.
pub use open_esp_radio_wifi_lifecycle::station as sta_lifecycle;
