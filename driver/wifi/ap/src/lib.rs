#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent access-point protocol owner.
//!
//! This crate implements a bounded WPA2-Personal peer table. Frame
//! codecs remain in `open-esp-radio-ieee80211`; MMIO, DMA, IRQ, hardware key
//! slots and Embassy deadlines remain in chip/runtime crates.

#[cfg(test)]
extern crate std;

pub mod service;

pub use service::{
    AP_MAX_CLIENTS, AP_STATUS_INVALID_RSN, AP_STATUS_SUCCESS, AP_STATUS_TOO_MANY_STATIONS,
    AP_WPA2_FIRST_RETRY_INTERVAL_MICROS, AP_WPA2_RETRY_ATTEMPTS,
    AP_WPA2_SUBSEQUENT_RETRY_INTERVAL_MICROS, AccessPointClientLimit, AccessPointClientLimitError,
    AccessPointInactiveTimeout, AccessPointInactiveTimeoutError, AccessPointPeerStorage,
    AccessPointService, AccessPointServiceStatus, ApMlmeAction, ApPeerClose, ApPeerCloseKind,
    ApPeerPhase, ApPeerStatus, ApServiceError, ApWpa2Error, ApWpa2Progress, ApWpa2RetryProgress,
};
