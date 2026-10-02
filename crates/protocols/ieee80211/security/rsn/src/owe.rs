//! OWE key hierarchy and association procedures.
//!
//! OWE uses the existing RSN four-way automata with [`Group`] as their suite.
//! Its PMK and MIC lengths cannot be represented by the legacy fixed PSK key
//! types, so OWE keys retain their negotiated group and full key lengths.
pub use oer_ieee80211_mac::owe::Group;
use oer_ieee80211_mac::{management::MacAddress, security::rsn::RSN_PMKID_LEN};

mod crypto;
pub use crypto::{OwePmk, OwePtk};
mod curve;
pub use curve::KeyPair;
mod security;
pub use security::SecurityProfile;
mod cache;
pub use cache::{CacheScope, CachedPmk, PmkCache};
mod initial;
pub use initial::InitialAssociation;
mod transition;
pub use transition::{BssIdentity, TransitionPair, TransitionTarget};
mod exchange;
pub use exchange::{ExchangeId, RetryPolicy, Transmission, TxTicket};
mod association;
pub use association::{
    AccessPoint, AccessPointPhase, AssociationContext, GroupPolicy, Station, StationPhase,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(oer_ieee80211_mac::owe::WireError),
    InvalidKey,
    WrongContext,
    InvalidMic,
    Eapol(crate::EapolParseError),
    Unwrap(crate::aes::SoftwareAesKeyUnwrapError),
    Wrap(crate::aes::SoftwareAesKeyWrapError),
    Elements(oer_ieee80211_mac::management::elements::ElementError),
    UnsupportedSecurity,
    SecurityMismatch,
    CapacityExceeded,
    KeyExpired,
    TimeWentBackwards,
    Frame(crate::frames::RsnFrameError),
    WrongPhase,
    InvalidConfiguration,
    DeadlineOverflow,
    StaleCompletion,
    RetryExhausted,
    TimedOut,
    AssociationRejected(u16),
}
impl From<crate::frames::RsnFrameError> for Error {
    fn from(error: crate::frames::RsnFrameError) -> Self {
        Self::Frame(error)
    }
}
impl From<oer_ieee80211_mac::management::elements::ElementError> for Error {
    fn from(error: oer_ieee80211_mac::management::elements::ElementError) -> Self {
        Self::Elements(error)
    }
}
impl From<crate::EapolParseError> for Error {
    fn from(error: crate::EapolParseError) -> Self {
        Self::Eapol(error)
    }
}

/// PMK cache/association scope. It is never inferred from an EAPOL frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Addresses {
    pub station: MacAddress,
    pub access_point: MacAddress,
}
impl Addresses {
    pub(super) fn validate(self) -> Result<(), Error> {
        use oer_ieee80211_mac::management::{MAC_ADDRESS_LEN, is_group_address};
        if self.station == self.access_point
            || self.station == [0; MAC_ADDRESS_LEN]
            || self.access_point == [0; MAC_ADDRESS_LEN]
            || is_group_address(self.station)
            || is_group_address(self.access_point)
        {
            return Err(Error::WrongContext);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
