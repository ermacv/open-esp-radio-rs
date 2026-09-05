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

/// Microseconds in a backend-defined monotonic radio epoch.
///
/// The epoch is deliberately not wall-clock time. An adapter must use one
/// stable epoch for every timestamp it publishes in a controller instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioTimestamp(u64);

impl RadioTimestamp {
    /// Construct a timestamp from monotonic microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Return monotonic microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0
    }
}
