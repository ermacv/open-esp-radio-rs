#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The [`openthread`] crate's `Radio` over the ESP32-S31 IEEE 802.15.4
//! runtime.
//!
//! It builds on the repository's fork of that crate
//! (<https://github.com/ermacv/openthread>, branch `oer/radio-security`),
//! whose trait also carries OpenThread's MAC keys, frame counter and
//! per-frame transmit information. The radio reports the capabilities
//! ESP-IDF's OpenThread port reports, apart from timed transmission and
//! reception: hardware acknowledgement with its timeout, address filtering,
//! promiscuous mode, source matching for frame pending, energy scan,
//! transmission from sleep, transmit security and timed transmission and
//! reception for Coordinated Sampled Listening. As in the port,
//! OpenThread's `SubMac` runs CSMA-CA backoffs and retries itself; each of
//! its attempts reaches the radio as one transmission with or without a
//! CCA, which the radio secures with OpenThread's keys and a frame counter
//! of its own. 2015 frames are answered with enhanced ACKs, secured ones
//! with the same keys and counter.
//!
//! [`openthread`]: https://docs.rs/openthread

#[cfg(test)]
extern crate std;

pub mod frames;

#[cfg(target_arch = "riscv32")]
mod radio;

#[cfg(target_arch = "riscv32")]
pub use radio::{OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, OpenThreadRadioDefaults};
