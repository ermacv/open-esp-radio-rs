#![no_std]
#![forbid(unsafe_code)]

//! Hardware-independent IEEE 802.11 protocol building blocks.
//!
//! This crate owns bounded frame parsing and protocol state only. It has no
//! MMIO, DMA, interrupt, executor, allocator, ESP32-S31, vendor archive, or
//! ROM ABI dependency.

pub mod alignment;
pub mod ap;
pub mod beacon;
pub mod block_ack;
pub mod ccmp;
pub mod channel;
pub mod classification;
pub mod data;
pub mod esp_now;
pub mod he;
pub mod ht;
pub mod management;
pub mod ndpa;
pub mod scan;
pub mod security;
pub mod ssid;
pub mod station;
pub mod station_beacon;
pub mod station_power_save;
pub mod tbtt;
pub mod trigger;
pub mod twt;
pub mod vif;
pub mod wmm;
