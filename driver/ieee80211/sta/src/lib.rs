#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent Wi-Fi station MLME and policy.
//!
//! Protocol crates own scan records, IEEE 802.11 framing and WPA state.
//! Chip/runtime adapters own concrete hardware and timer operations. This crate
//! owns Authentication/Association state, finite candidate-scan ordering,
//! and the outer attempt, reconnect and backoff policy
//! while preserving one caller-defined resource owner across every
//! asynchronous edge.

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_support;

pub mod ftm;
pub mod join;
pub mod link_monitor;
pub mod power_save;
pub mod request;
pub mod scan;
pub mod station;
pub mod twt;
