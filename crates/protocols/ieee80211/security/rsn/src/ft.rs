//! Sans-IO Fast BSS Transition: key hierarchy, peer procedures and key holders.
//!
//! FT keys have different contexts and lifetimes from ordinary RSN PMKs/PTKs.
//! The caller supplies authenticated initial credentials, nonces, time, frame
//! delivery and completed key/association transactions. No method performs IO.

mod crypto;
pub use crypto::*;
mod initial;
pub use initial::*;
mod protocol;
pub use protocol::{SecurityProfile, TransitionGroupKeys};
mod session;
pub use session::*;
mod station;
pub use station::*;
mod ap;
pub use ap::*;
mod key_holders;
pub use key_holders::*;
mod ds;
pub use ds::*;

use oer_ieee80211_mac::ft as wire;
pub use oer_ieee80211_mac::ft::{FtAkm, MobilityDomain, MobilityDomainId, R0khId, R1khId};
use oer_ieee80211_mac::management::MacAddress;
use oer_ieee80211_mac::security::RSN_NONCE_LEN;
use oer_ieee80211_mac::ssid::WifiSsid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(wire::WireError),
    Frame(crate::frames::RsnFrameError),
    WrongKeyContext,
    WrongPeer,
    WrongPhase,
    StaleOperation,
    KeyExpired,
    ZeroNonce,
    InvalidMic,
    InvalidGroupKeys,
    UnsupportedSecurity,
    SecurityMismatch,
    PeerRejected(u16),
    ExpiredOperation,
    InvalidConfiguration,
    TransportFailure,
    CapacityExceeded,
    DeadlineOverflow,
    TimeWentBackwards,
    Busy,
    OperationExhausted,
}
impl From<wire::WireError> for Error {
    fn from(error: wire::WireError) -> Self {
        Self::Wire(error)
    }
}

/// Immutable scope of a root key. The station address is part of its KDF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootContext {
    pub ssid: WifiSsid,
    pub mobility_domain: MobilityDomainId,
    pub r0kh: R0khId,
    pub station: MacAddress,
}

#[cfg(test)]
mod tests;

impl From<crate::frames::RsnFrameError> for Error {
    fn from(error: crate::frames::RsnFrameError) -> Self {
        Self::Frame(error)
    }
}
