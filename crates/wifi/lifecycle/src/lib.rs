#![no_std]

//! Executor- and chip-independent Wi-Fi lifecycle ownership.
//!
//! Protocol crates own scan, IEEE 802.11 and WPA state. Chip/runtime adapters
//! own concrete hardware and timer operations. This crate owns finite
//! candidate-scan ordering plus the outer attempt, reconnect and backoff policy
//! while preserving one caller-defined resource owner across every
//! asynchronous edge.

#[cfg(test)]
extern crate std;

pub mod scan;
pub mod station;
