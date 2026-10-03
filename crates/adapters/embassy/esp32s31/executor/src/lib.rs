#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Scheduler-free Embassy platform runtime for ESP32-S31 applications.
//!
//! This crate owns only executor wake-up and timer-queue integration. It does
//! not own radio policy, board electrical parameters, credentials, sockets,
//! HIL protocol. Optional timer observations describe only its own execution. Applications supply the ESP-HAL software
//! interrupt and timer capabilities explicitly, with the tokens of their
//! sources in the image's interrupt table, whose entries name
//! `wake_handler` and `timer_interrupt`.

#[cfg(feature = "esp32s31")]
mod executor;
#[cfg(feature = "esp32s31")]
mod time_driver;
#[cfg(any(feature = "esp32s31", test))]
mod timer_queue;

#[cfg(feature = "esp32s31")]
pub use executor::{Executor, wake_handler};
#[cfg(feature = "esp32s31")]
pub use time_driver::{Timer, init, timer_interrupt};

#[cfg(any(feature = "timer-observation", test))]
pub mod timer_observation;
