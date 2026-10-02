#![no_std]
#![forbid(unsafe_code)]

//! Hardware-independent IEEE 802.11 protocol building blocks.
//!
//! This crate owns bounded frame parsing and protocol state only. It has no
//! MMIO, DMA, interrupt, executor, allocator, ESP32-S31, vendor archive, or
//! ROM ABI dependency. [`ft`] owns FT wire syntax; secret keys and FT
//! procedures belong to the RSN protocol package.

#[cfg(test)]
extern crate std;

pub mod anqp;
pub mod ap;
pub mod beacon;
pub mod block_ack;
pub mod ccmp;
pub mod channel;
pub mod classification;
mod codec;
pub mod data;
pub mod extensions;
pub mod qos;
pub mod roaming;

pub mod fragmentation;
pub mod ft;
pub mod ftm;
pub mod gas;
pub mod he;
pub mod ht;
pub mod management;
pub mod management_protection;
pub mod ndpa;
pub mod owe;
pub mod phy;
pub mod protection;
pub mod scan;
pub mod security;
pub mod sequence;
pub mod ssid;
pub mod station;
pub mod station_beacon;
pub mod station_power_save;
pub mod tbtt;
pub mod trigger;
pub mod twt;
pub mod vif;
