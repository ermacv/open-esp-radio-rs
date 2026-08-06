#![no_std]
#![forbid(unsafe_code)]

//! Embassy-specific composition for the ESP32-S31 Wi-Fi backend.
//!
//! Core radio, PHY and MAC crates remain executor-neutral. This crate owns
//! wakeups, cooperative async TX access and the bridge from pinned network
//! frames to ESP32-S31 Wi-Fi DMA.

#[cfg(test)]
extern crate std;

pub mod aggregate_tx;
pub mod aggregate_tx_observer;
pub mod connected_control;
pub mod connected_runner;
pub mod connected_rx_protocol;
pub mod connected_services;
pub mod connected_sta_port;
pub mod connected_sta_teardown;
pub mod control_mailbox;
pub mod control_tx;
pub mod cooperative_hardware;
#[doc(hidden)]
pub mod cooperative_tx;
pub mod embassy_irq;
pub mod embassy_rx;
#[cfg(target_arch = "riscv32")]
mod join_time;
pub mod network_rx;
mod ordinary_tx;
#[cfg(target_arch = "riscv32")]
pub mod phy_delay;
pub mod preconnected_rx;
pub mod rx_dma_service;
pub mod rx_pipeline_observer;
pub mod rx_reorder;
pub mod scan_port;
pub mod scan_rx;
#[cfg(target_arch = "riscv32")]
pub mod scan_target;
pub mod scan_tx;
pub mod single_mpdu_tx;
#[cfg(target_arch = "riscv32")]
pub mod sta_attempt_target;
pub mod sta_join_port;
pub mod sta_tx_epoch;
pub mod station;
pub mod station_epoch;
mod station_tasks;
pub mod tx_time;
pub mod wpa2_port;
#[cfg(target_arch = "riscv32")]
mod wpa2_time;
