#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent access-point protocol owner.
//!
//! This crate implements a bounded Open or WPA2-Personal peer table. Frame
//! codecs remain in `open-esp-radio-ieee80211`; MMIO, DMA, IRQ, hardware key
//! slots and Embassy deadlines remain in chip/runtime crates. Replay and key
//! ownership exist only in WPA2 epochs.

#[cfg(test)]
extern crate std;

pub mod service;

pub use service::{
    AP_MAX_CLIENTS, AP_STATUS_INVALID_RSN, AP_STATUS_SUCCESS, AP_STATUS_TOO_MANY_STATIONS,
    AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT_MICROS, AP_TX_BLOCK_ACK_TID, AP_TX_BLOCK_ACK_WINDOW,
    AP_WPA2_FIRST_RETRY_INTERVAL_MICROS, AP_WPA2_RETRY_ATTEMPTS,
    AP_WPA2_SUBSEQUENT_RETRY_INTERVAL_MICROS, AccessPointClientLimit, AccessPointClientLimitError,
    AccessPointInactiveTimeout, AccessPointInactiveTimeoutError, AccessPointPeerStorage,
    AccessPointService, AccessPointServiceStatus, ApAssociationCapabilities, ApAssociationIdentity,
    ApBufferedGroupRelease, ApBufferedUnicastRelease, ApDownlinkAdmission, ApDownlinkDisposition,
    ApMlmeAction, ApPeerBinding, ApPeerClose, ApPeerCloseKind, ApPeerPhase, ApPeerPowerState,
    ApPeerStatus, ApPowerSaveAction, ApServiceError, ApWpa2Error, ApWpa2Progress,
    ApWpa2RetryProgress,
};
