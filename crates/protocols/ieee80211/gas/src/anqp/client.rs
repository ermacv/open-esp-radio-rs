//! Protocol-0 requester composition. GAS transport completion alone does not
//! publish malformed, duplicate or unrequested ANQP data as success.
use super::*;
use crate::{Category, DialogId, PeerIdentity, Transmission, TxId, TxOutcome, requester};
use oer_time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Complete {
        id: DialogId,
    },
    InvalidResponse {
        id: DialogId,
        reason: Error,
    },
    Rejected {
        id: DialogId,
        status: oer_ieee80211_mac::gas::Status,
    },
    TimedOut {
        id: DialogId,
    },
    Cancelled {
        id: DialogId,
    },
}
/// Owns only the query snapshot needed for response validation. All fragment,
/// admission, token, timeout and cancellation state has one GAS owner.
pub struct Requester<const FRAME: usize, const RESPONSE: usize> {
    gas: requester::Requester<FRAME, RESPONSE>,
    query: Bytes<FRAME>,
}
impl<const FRAME: usize, const RESPONSE: usize> Requester<FRAME, RESPONSE> {
    pub fn new(peer: PeerIdentity, config: requester::Config) -> Result<Self, Error> {
        Ok(Self {
            gas: requester::Requester::new(peer, config)?,
            query: Bytes::empty(),
        })
    }
    pub fn request(
        &mut self,
        category: Category,
        query: Query<'_>,
        now: Instant,
    ) -> Result<DialogId, Error> {
        let retained = Bytes::copy(query.as_bytes())?;
        let id = self.gas.request(
            category,
            oer_ieee80211_mac::gas::ProtocolId::Standard(oer_ieee80211_mac::gas::ANQP_PROTOCOL_ID),
            retained.bytes(),
            now,
        )?;
        self.query = retained;
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.gas.transmission()
    }
    pub fn admitted(&mut self, id: TxId, now: Instant) -> Result<(), Error> {
        Ok(self.gas.admitted(id, now)?)
    }
    pub fn tx_completed(
        &mut self,
        id: TxId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<requester::Progress, Error> {
        Ok(self.gas.tx_completed(id, outcome, now)?)
    }
    pub fn receive(
        &mut self,
        peer: PeerIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<requester::Progress, Error> {
        Ok(self.gas.receive(peer, bytes, now)?)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.gas.next_deadline()
    }
    pub fn poll(&mut self, now: Instant) -> Result<requester::Progress, Error> {
        Ok(self.gas.poll(now)?)
    }
    pub fn cancel(&mut self, id: DialogId) -> Result<(), Error> {
        Ok(self.gas.cancel(id)?)
    }
    /// Exactly one semantic outcome. Invalid ANQP has its own terminal outcome
    /// and never becomes a usable response, even after this event is drained.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        Some(match self.gas.take_outcome()? {
            requester::Outcome::Complete { id } => match self.response() {
                Ok(_) => Outcome::Complete { id },
                Err(reason) => Outcome::InvalidResponse { id, reason },
            },
            requester::Outcome::Rejected { id, status } => Outcome::Rejected { id, status },
            requester::Outcome::TimedOut { id } => Outcome::TimedOut { id },
            requester::Outcome::Cancelled { id } => Outcome::Cancelled { id },
        })
    }
    pub fn response(&self) -> Result<Option<Elements<'_>>, Error> {
        self.gas
            .response()
            .map(|bytes| {
                validate_response(
                    Query::parse(self.query.bytes()).expect("validated retained query"),
                    bytes,
                )
            })
            .transpose()
    }
}
