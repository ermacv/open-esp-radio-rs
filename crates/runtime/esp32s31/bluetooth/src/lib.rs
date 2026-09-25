#![no_std]
#![forbid(unsafe_code)]

//! Embassy execution and ownership adapters for the ESP32-S31 Bluetooth controller.
//!
//! The controller actor retains idle and active radio owners across borrowed
//! waits, publishes lossless response boundaries, and orchestrates finite session
//! actors. Session adapters provide DTM, advertising, scanning, and peripheral
//! waits. Durable wake notifications register before rechecking chip-owned work;
//! command interpretation and hardware transitions remain in the controller core.

#[cfg(test)]
extern crate std;

pub mod controller;
pub mod session;
pub mod time;

pub mod notification;
