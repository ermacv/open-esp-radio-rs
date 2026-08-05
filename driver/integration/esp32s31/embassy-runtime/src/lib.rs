#![no_std]
#![cfg(feature = "esp32s31")]

//! Scheduler-free Embassy platform runtime for ESP32-S31 applications.
//!
//! This crate owns only executor wake-up and timer-queue integration. It does
//! not own radio policy, board electrical parameters, credentials, sockets,
//! diagnostics or HIL protocol. Applications supply the ESP-HAL software
//! interrupt and timer capabilities explicitly.

mod executor;
mod time_driver;

pub use executor::Executor;
pub use time_driver::{Timer, init};
