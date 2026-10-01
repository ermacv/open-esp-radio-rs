#![no_std]
#![forbid(unsafe_code)]

//! Bounded IEEE 802.11k/v dialogs, measurements, neighbor data, candidate policy
//! power services and Event/Diagnostic reports, driven by supplied values.
//!
//! Each owner belongs to one peer and association generation. The integration
//! authenticates robust Action frames, copies action bodies to its own TX
//! buffers, admits transmissions and supplies their terminal results. These
//! owners never read a clock, wait, allocate, scan or change associations.
//!
//! A requester can be driven with ordinary host values. The integration must
//! validate received management frames and protect outgoing ones as negotiated.
//!
//! ```
//! use oer_ieee80211_mac::roaming::{Elements, NeighborReportRequest};
//! use oer_ieee80211_roaming::{LinkIdentity, TxOutcome};
//! use oer_ieee80211_roaming::neighbor::{NeighborEvent, NeighborReportRequester};
//! use oer_time::{Duration, Instant};
//!
//! let link = LinkIdentity { peer: [2, 0, 0, 0, 0, 1], generation: 1 };
//! let now = Instant::EPOCH;
//! let mut owner = NeighborReportRequester::<128>::new(
//!     link, Duration::from_secs(1), Duration::from_secs(30),
//! )?;
//! let id = owner.request(now, NeighborReportRequest {
//!     dialog_token: 1, elements: Elements::EMPTY,
//! })?;
//! let mut tx_storage = [0; 128];
//! let transmission = owner.transmission().expect("queued request");
//! tx_storage[..transmission.body.len()].copy_from_slice(transmission.body);
//! // After the integration's backend has accepted the owned TX buffer:
//! owner.admitted(id, now)?;
//! owner.tx_completed(id, TxOutcome::Acknowledged, now)?;
//! // An AP may return an empty report list. The validated Action body is:
//! assert_eq!(owner.receive(link, &[5, 5, 1], now)?, NeighborEvent::ReportsReceived);
//! assert!(owner.reports(now).is_some());
//! # Ok::<(), oer_ieee80211_roaming::Error>(())
//! ```

#[cfg(test)]
extern crate std;

pub mod btm;
pub mod database;
pub mod diagnostics;
pub mod dms;
pub mod events;
mod exchange;
pub mod idle;
mod management;
pub mod measurement;
pub mod neighbor;
pub mod reports;
pub mod selection;
pub mod sleep;
pub mod tclas;
pub mod tfs;

use oer_ieee80211_mac::management::is_group_address;
use oer_ieee80211_mac::roaming::{DIALOG_TOKEN_OFFSET, MacAddress, WireError};
use oer_time::{Duration, Instant};

/// Generation changes whenever the integration replaces an association.
/// An RX completion must retain the generation of the RX epoch that owns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkIdentity {
    pub peer: MacAddress,
    pub generation: u64,
}

/// Local operation identity, independent of a reusable wire dialog token.
/// Route it back to the owner that issued it; different owners have independent
/// local identity sequences, even if they belong to the same association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId {
    pub link: LinkIdentity,
    serial: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Wire(WireError),
    Busy,
    NoPendingOperation,
    WrongOperation,
    NotAdmitted,
    AlreadyAdmitted,
    ZeroTimeout,
    TimeOverflow,
    TimeBeforeOperation,
    IdentityExhausted,
    FrameTooLarge { required: usize, capacity: usize },
    UnsupportedRequestMode(u8),
    InvalidBeaconInterval,
    InvalidTarget,
    CandidateExpired,
    CandidateNotOffered,
    CandidateExcluded,
    ConflictingCandidates,
    ConflictingDialog,
    InvalidBeaconTiming,
    MissingTerminationDeadline,
}
impl From<WireError> for Error {
    fn from(value: WireError) -> Self {
        Self::Wire(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome {
    Acknowledged,
    Failed,
}

/// Copy `body` into an integration-owned buffer before admission. Admission
/// does not lend protocol storage to DMA; superseding a dialog is then safe.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Transmission<'a> {
    pub id: OperationId,
    pub destination: MacAddress,
    pub body: &'a [u8],
}
impl core::fmt::Debug for Transmission<'_> {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        out.debug_struct("Transmission")
            .field("id", &self.id)
            .field("destination", &self.destination)
            .field("body_length", &self.body.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogEvent {
    Ignored,
    Transmitted,
    TxFailed,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestEvent {
    Ignored,
    ResponseReceived { id: OperationId },
    TxFailed { id: OperationId },
    TimedOut { id: OperationId },
    Cancelled { id: OperationId },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Body<const BYTES: usize> {
    bytes: [u8; BYTES],
    len: usize,
}
impl<const BYTES: usize> Body<BYTES> {
    fn empty() -> Self {
        Self {
            bytes: [0; BYTES],
            len: 0,
        }
    }
    fn copy(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > BYTES {
            return Err(Error::FrameTooLarge {
                required: bytes.len(),
                capacity: BYTES,
            });
        }
        let mut body = Self::empty();
        body.bytes[..bytes.len()].copy_from_slice(bytes);
        body.len = bytes.len();
        Ok(body)
    }
    fn encode(encode: impl FnOnce(&mut [u8]) -> Result<usize, WireError>) -> Result<Self, Error> {
        let mut body = Self::empty();
        body.len = encode(&mut body.bytes).map_err(|error| match error {
            WireError::OutputTooSmall { required } => Error::FrameTooLarge {
                required,
                capacity: BYTES,
            },
            error => Error::Wire(error),
        })?;
        Ok(body)
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

struct Identities {
    link: LinkIdentity,
    next: u64,
}
impl Identities {
    fn new(link: LinkIdentity) -> Self {
        Self { link, next: 0 }
    }
    fn issue(&mut self) -> Result<OperationId, Error> {
        self.next = self.next.checked_add(1).ok_or(Error::IdentityExhausted)?;
        Ok(OperationId {
            link: self.link,
            serial: self.next,
        })
    }
}

fn deadline(now: Instant, timeout: Duration) -> Result<Instant, Error> {
    if timeout == Duration::ZERO {
        return Err(Error::ZeroTimeout);
    }
    now.checked_add(timeout).ok_or(Error::TimeOverflow)
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TxPhase {
    Ready,
    InFlight,
    Waiting,
}

struct Pending<const BYTES: usize> {
    id: OperationId,
    body: Body<BYTES>,
    started: Instant,
    deadline: Instant,
    phase: TxPhase,
}
impl<const BYTES: usize> Pending<BYTES> {
    fn new(id: OperationId, body: Body<BYTES>, started: Instant, deadline: Instant) -> Self {
        Self {
            id,
            body,
            started,
            deadline,
            phase: TxPhase::Ready,
        }
    }
    fn transmission(&self) -> Option<Transmission<'_>> {
        (self.phase == TxPhase::Ready).then_some(Transmission {
            id: self.id,
            destination: self.id.link.peer,
            body: self.body.bytes(),
        })
    }
    fn live(&self, now: Instant) -> Result<bool, Error> {
        if now < self.started {
            Err(Error::TimeBeforeOperation)
        } else {
            Ok(now < self.deadline)
        }
    }
    fn admit(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        if id != self.id {
            return Err(Error::WrongOperation);
        }
        if !self.live(now)? {
            return Err(Error::NoPendingOperation);
        }
        if self.phase != TxPhase::Ready {
            return Err(Error::AlreadyAdmitted);
        }
        self.phase = TxPhase::InFlight;
        Ok(())
    }
    fn complete(&mut self, id: OperationId, outcome: TxOutcome) -> Result<bool, Error> {
        if id != self.id {
            return Ok(false);
        }
        if self.phase == TxPhase::Ready {
            return Err(Error::NotAdmitted);
        }
        if self.phase == TxPhase::Waiting {
            return Ok(false);
        }
        if outcome == TxOutcome::Acknowledged {
            self.phase = TxPhase::Waiting;
        }
        Ok(true)
    }
    fn received(&self, token: u8) -> bool {
        self.phase != TxPhase::Ready && self.body.bytes()[DIALOG_TOKEN_OFFSET] == token
    }
}

/// One bounded response transmission, with no subsequent wire response.
pub(crate) struct Sender<const BYTES: usize> {
    ids: Identities,
    timeout: Duration,
    pending: Option<Pending<BYTES>>,
}
impl<const BYTES: usize> Sender<BYTES> {
    fn start_using(
        &mut self,
        now: Instant,
        body: Body<BYTES>,
        ids: &mut Identities,
    ) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if ids.link != self.ids.link {
            return Err(Error::WrongOperation);
        }
        let expires = deadline(now, self.timeout)?;
        self.start_using_until(now, body, ids, expires)
    }
    fn start_using_until(
        &mut self,
        now: Instant,
        body: Body<BYTES>,
        ids: &mut Identities,
        until: Instant,
    ) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if ids.link != self.ids.link {
            return Err(Error::WrongOperation);
        }
        if until <= now {
            return Err(Error::NoPendingOperation);
        }
        let remaining = Duration::from_micros(until.as_micros() - now.as_micros());
        let expires = deadline(now, self.timeout.min(remaining))?;
        let id = ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, expires));
        Ok(id)
    }
    fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        deadline(Instant::EPOCH, timeout)?;
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            pending: None,
        })
    }
    fn start(&mut self, now: Instant, body: Body<BYTES>) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let expires = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, expires));
        Ok(id)
    }
    fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.pending
            .as_mut()
            .ok_or(Error::NoPendingOperation)?
            .admit(id, now)
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.deadline)
    }
    fn poll(&mut self, now: Instant) -> Result<DialogEvent, Error> {
        if let Some(pending) = &self.pending
            && !pending.live(now)?
        {
            self.pending = None;
            return Ok(DialogEvent::TimedOut);
        }
        Ok(DialogEvent::Ignored)
    }
    fn completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DialogEvent, Error> {
        if self.pending.as_ref().is_none_or(|pending| pending.id != id) {
            return Ok(DialogEvent::Ignored);
        }
        if self.poll(now)? == DialogEvent::TimedOut {
            return Ok(DialogEvent::TimedOut);
        }
        let Some(pending) = &mut self.pending else {
            return Ok(DialogEvent::Ignored);
        };
        if !pending.complete(id, outcome)? {
            return Ok(DialogEvent::Ignored);
        }
        self.pending = None;
        Ok(if outcome == TxOutcome::Acknowledged {
            DialogEvent::Transmitted
        } else {
            DialogEvent::TxFailed
        })
    }
    fn cancel(&mut self) -> DialogEvent {
        if self.pending.take().is_some() {
            DialogEvent::Cancelled
        } else {
            DialogEvent::Ignored
        }
    }
}

// Octet-indexed state capacity; not an on-air protocol constant.
const OCTET_VALUE_COUNT: usize = u8::MAX as usize + 1;

fn valid_peer_address(address: MacAddress) -> bool {
    !is_group_address(address) && address.iter().any(|octet| *octet != 0)
}
