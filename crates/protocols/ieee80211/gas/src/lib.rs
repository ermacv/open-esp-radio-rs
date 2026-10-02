#![no_std]
#![forbid(unsafe_code)]

//! Bounded GAS query transport, driven by frames, completions and supplied time.
//! Dialogs exist before association; their identities belong to the caller's
//! management receive epoch, not to the roaming package's associated link.
//! The caller validates addresses/channel and any Protected Dual protection,
//! copies TX bytes into its own storage, and admits accepted transmissions.

#[cfg(test)]
extern crate std;

pub mod anqp;
pub mod requester;
pub mod responder;
mod storage;
mod transport;

use oer_ieee80211_mac::{
    gas::{self, Category},
    management::{BROADCAST_ADDRESS, IEEE_TIME_UNIT_MICROS, MacAddress, is_group_address},
};
use oer_time::{Duration, Instant};
use storage::{Bytes, OwnedAdvertisement};
use transport::{Identities, PendingTx};

/// Complete routing context supplied by the management owner. `epoch` changes
/// when its VIF, address, channel/peer context or protection binding is retired.
/// It also changes when the protocol owner is recreated. Distinct owners that
/// share routing use distinct epochs. An epoch must never be reused while old
/// queued RX/TX events can arrive; the router retains the original binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub local: MacAddress,
    pub peer: MacAddress,
    pub bssid: MacAddress,
    pub epoch: u64,
}
impl PeerIdentity {
    fn validate(self) -> Result<Self, Error> {
        if self.epoch == 0
            || self.local == self.peer
            || is_group_address(self.local)
            || is_group_address(self.peer)
            || self.local == [0; core::mem::size_of::<MacAddress>()]
            || self.peer == [0; core::mem::size_of::<MacAddress>()]
            || self.bssid == [0; core::mem::size_of::<MacAddress>()]
            || (is_group_address(self.bssid) && self.bssid != BROADCAST_ADDRESS)
        {
            return Err(Error::WrongPeer);
        }
        Ok(self)
    }
}

/// Owner-scoped dialog identity, independent of the eight-bit wire token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DialogId {
    pub peer: PeerIdentity,
    serial: u64,
}

/// Unique submission identity. Retransmissions never reuse a submission ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxId {
    pub dialog: DialogId,
    serial: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome {
    Acknowledged,
    Failed,
}

/// Management headers, sequence allocation, protection and radio storage are
/// external. Admission never lends these protocol-owned bytes to DMA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transmission<'a> {
    pub id: TxId,
    pub category: Category,
    pub body: &'a [u8],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(gas::WireError),
    InvalidConfiguration,
    WrongPeer,
    Busy,
    NoDialog,
    WrongOperation,
    NotAdmitted,
    AlreadyAdmitted,
    IdentityExhausted,
    TokensExhausted,
    TimeWentBackwards,
    TimeOverflow,
    Expired,
    Capacity { required: usize, capacity: usize },
    DialogConflict,
    AmbiguousSequence,
    ProtocolChanged,
    UnexpectedFragment { expected: u8, received: u8 },
    FragmentLimit,
    ResponseLimit,
    ResponseNotReady,
}
impl From<gas::WireError> for Error {
    fn from(value: gas::WireError) -> Self {
        Self::Wire(value)
    }
}

fn after(now: Instant, duration: Duration) -> Result<Instant, Error> {
    now.checked_add(duration).ok_or(Error::TimeOverflow)
}
fn comeback_at(now: Instant, delay_tu: u16) -> Result<Instant, Error> {
    after(
        now,
        Duration::from_micros(u64::from(delay_tu) * IEEE_TIME_UNIT_MICROS),
    )
}
fn observe(previous: &mut Instant, now: Instant) -> Result<(), Error> {
    if now < *previous {
        return Err(Error::TimeWentBackwards);
    }
    *previous = now;
    Ok(())
}

#[cfg(test)]
mod tests;
