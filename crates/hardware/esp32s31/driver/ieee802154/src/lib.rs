//! Interrupt-driven ESP32-S31 IEEE 802.15.4 MAC driver.
//!
//! [`engine`] ports the public ESP-IDF driver's state machine
//! (`esp_ieee802154_dev.c`) and public API layer over the HAL LL backend.
//! The engine names no executor and no interrupt route: a runtime holds it
//! with the MAC owners in one critical section and calls its operation
//! entries and interrupt handler.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

#[cfg(test)]
extern crate std;

pub mod engine;
