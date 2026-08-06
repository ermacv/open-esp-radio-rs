#![no_std]

//! ESP32-S31 qualification-only observers and report accumulators.
//!
//! Driver crates publish typed observation events; this crate selects
//! counters, sampling policy and report snapshots for HIL. Production
//! firmware does not depend on it.

#[cfg(test)]
extern crate std;

pub mod aggregate_tx;
pub mod mac_irq;
pub mod rx_evidence;
pub mod rx_order;
pub mod rx_pipeline;
pub mod task_poll;
