//! Standalone Traffic Filtering Service. Frames replace the entire requested
//! filter set. The AP and the station own independent negotiated state.
mod ap;
mod station;
mod traffic;
pub use ap::*;
pub use station::*;
pub use traffic::*;

use crate::{Error, RequestEvent};
use oer_ieee80211_mac::roaming::WireError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TfsError {
    Protocol(Error),
    Traffic(TrafficError),
    UnknownNotification(u8),
    UnsupportedElements,
}
impl From<Error> for TfsError {
    fn from(value: Error) -> Self {
        Self::Protocol(value)
    }
}
impl From<WireError> for TfsError {
    fn from(value: WireError) -> Self {
        Self::Protocol(value.into())
    }
}
impl From<TrafficError> for TfsError {
    fn from(value: TrafficError) -> Self {
        Self::Traffic(value)
    }
}

/// A lost admitted replacement might already have taken effect at the AP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TfsState {
    Unconfigured,
    Configured,
    Uncertain,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TfsStationEvent {
    Ignored,
    ResponseReceived { id: crate::OperationId },
    Request(RequestEvent),
    RecoveryRequired { id: crate::OperationId },
}

#[cfg(test)]
mod tests;
