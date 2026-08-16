#![no_std]
#![forbid(unsafe_code)]
#![expect(
    clippy::large_enum_variant,
    reason = "executor state enums retain concrete no-alloc owners across transitions"
)]
#![expect(
    clippy::manual_async_fn,
    reason = "port traits expose explicit borrowed Future contracts shared by host and Embassy implementations"
)]
#![expect(
    clippy::result_large_err,
    reason = "failure paths return ownership-bearing state so callers can recover without allocation"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "top-level composition methods make independent hardware and service dependencies explicit"
)]
#![expect(
    clippy::type_complexity,
    reason = "composition signatures preserve the exact static owner graph and executor lifetimes"
)]

//! Embassy-specific composition for the ESP32-S31 Wi-Fi backend.
//!
//! Core radio, PHY and MAC crates remain executor-neutral. This crate owns
//! wakeups, cooperative async TX access and the bridge from pinned network
//! frames to ESP32-S31 Wi-Fi DMA.

#[cfg(test)]
extern crate std;

pub mod access_point;
pub mod aggregate_tx;
pub mod aggregate_tx_observer;
pub mod ampdu_resources;
mod connected_control;
pub mod connected_rx_protocol;
pub mod connected_sta_port;
pub mod connected_sta_teardown;
pub mod control_mailbox;
pub mod embassy_irq;
pub mod embassy_rx;
mod ethernet_rx;
#[cfg(target_arch = "riscv32")]
mod join_time;
pub mod monitor;
#[cfg(target_arch = "riscv32")]
mod monitor_builder;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod monitor_rx;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod monitor_service;
pub mod network_rx;
#[cfg(target_arch = "riscv32")]
pub mod phy_delay;
pub mod resource_profile;
pub mod rx_dma_service;
pub mod rx_frontier;
pub mod rx_pipeline_observer;
pub mod rx_reorder;
pub mod scan_port;
pub mod scan_rx;
#[cfg(target_arch = "riscv32")]
pub mod scan_target;
#[cfg(target_arch = "riscv32")]
pub mod sta_attempt_target;
#[cfg(any(target_arch = "riscv32", test))]
mod sta_join_port;
pub mod sta_tx_epoch;
pub mod station;
pub mod station_epoch;
pub mod tx_time;
pub mod wdev;
#[cfg(target_arch = "riscv32")]
mod wpa2_port;
#[cfg(target_arch = "riscv32")]
mod wpa2_time;
