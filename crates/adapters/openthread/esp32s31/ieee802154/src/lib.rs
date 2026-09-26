#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The [`openthread`] crate's `Radio` over the ESP32-S31 IEEE 802.15.4
//! runtime.
//!
//! The radio reports the capabilities it keeps under that trait: hardware
//! acknowledgement with its timeout, address filtering, promiscuous mode,
//! source matching for frame pending, energy scan and transmission from
//! sleep. The trait passes no MAC keys, frame counters, per-frame retry or
//! CSMA-CA parameters and no enhanced-ACK content, so OpenThread's `SubMac`
//! runs CSMA-CA backoffs and retries itself and secures frames in software,
//! as ESP-IDF's OpenThread port lets it do for the backoffs and retries; each
//! of its attempts reaches the radio as one transmission with or without a
//! CCA. The radio answers 2015 frames with unsecured enhanced ACKs; secured
//! 2015 frames get none.
//!
//! [`openthread`]: https://docs.rs/openthread

#[cfg(test)]
extern crate std;

pub mod frames;

#[cfg(target_arch = "riscv32")]
mod radio;

#[cfg(target_arch = "riscv32")]
pub use radio::{OpenThreadRadio, OpenThreadRadioDefaults};
