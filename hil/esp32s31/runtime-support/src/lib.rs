#![no_std]

//! Scheduler-free Embassy support for the ESP32-S31 HIL runtime.
//!
//! This remains private test-platform code. Driver crates consume only
//! executor-neutral delay/event traits.

mod executor;
mod time_driver;

pub use executor::Executor;
pub use time_driver::{Timer, init};
