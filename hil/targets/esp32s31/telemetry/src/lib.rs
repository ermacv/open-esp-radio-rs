#![no_std]

//! ESP32-S31 qualification-only observers and report accumulators.
//!
//! Driver crates publish typed observation events; this crate selects
//! counters, sampling policy and report snapshots for HIL. Production
//! firmware does not depend on it.

#[cfg(test)]
extern crate std;

pub mod rx_pipeline;
