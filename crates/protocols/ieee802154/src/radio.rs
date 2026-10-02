//! Portable radio requests, backend observations and their finite admission state.
//! Correlation identifiers and monotonic timestamps are shared across that boundary.

/// Published backend operation capabilities.
pub mod capabilities;
/// Validated channels in the implemented 2.4 GHz profile.
pub mod channel;
/// Caller requests and configuration values.
pub mod command;
/// Backend observations and normalized receive metadata.
pub mod event;
/// Addressing interfaces of a multi-PAN radio.
pub mod interface;
/// Command admission and event validation under one finite owner.
pub mod state;

/// Caller-assigned identifier used to correlate an operation and completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RequestId(u32);

impl RequestId {
    /// Preserve one caller-owned identifier.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the caller-owned identifier image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl oer_radio_port::Correlation for RequestId {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn raw(self) -> u32 {
        self.0
    }
}

/// Microseconds in the backend's monotonic radio epoch.
///
/// The epoch is deliberately not wall-clock time. An adapter must use one
/// stable epoch for every timestamp it publishes in a controller instance.
pub use oer_time::RadioInstant;
