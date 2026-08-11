#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent access-point protocol owner.
//!
//! This crate deliberately implements one bounded WPA2-Personal peer. Frame
//! codecs remain in `open-esp-radio-ieee80211`; MMIO, DMA, IRQ, hardware key
//! slots and Embassy deadlines remain in chip/runtime crates.

#[cfg(test)]
extern crate std;

pub mod service;

pub use service::{
    AP_ASSOCIATION_ID, AP_STATUS_INVALID_RSN, AP_STATUS_SUCCESS, AP_STATUS_TOO_MANY_STATIONS,
    AccessPointService, ApMlmeAction, ApPeerPhase, ApServiceError, ApWpa2Error, ApWpa2Progress,
};
