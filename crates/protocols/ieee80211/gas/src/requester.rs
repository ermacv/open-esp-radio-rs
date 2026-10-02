//! One peer's query owner. A lost Comeback response restarts the complete
//! query with a fresh token: Comeback Requests carry no fragment number, so
//! retrying one as a new request could skip a response fragment.
use super::*;
use gas::{AdvertisementProtocol, Body, Frame, ProtocolId, ResponseInfo, Status};
mod tokens;
use tokens::Tokens;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub dialog_timeout: Duration,
    pub response_timeout: Duration,
    /// Whole-query restarts after a lost response. The total deadline is fixed.
    pub max_restarts: u8,
    /// Additional quarantine beyond the local dialog deadline, covering the
    /// peer's response/cache lifetime and retained MAC RX events. Supplied by
    /// integration; no finite on-air lifetime is invented by the core.
    pub token_reuse_guard: Duration,
}
impl Config {
    fn validate(self) -> Result<Self, Error> {
        if self.dialog_timeout == Duration::ZERO || self.response_timeout == Duration::ZERO {
            return Err(Error::InvalidConfiguration);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Complete { id: DialogId },
    Rejected { id: DialogId, status: Status },
    TimedOut { id: DialogId },
    Cancelled { id: DialogId },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Ignored,
    Waiting,
    FragmentAccepted,
    Finished,
    Restarted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Initial,
    Delay(Instant),
    Comeback,
}

struct Dialog<const FRAME: usize, const RESPONSE: usize> {
    id: DialogId,
    category: Category,
    token: u8,
    initial: Bytes<FRAME>,
    advertisement: Option<OwnedAdvertisement>,
    response: Bytes<RESPONSE>,
    last_response: Bytes<FRAME>,
    expected_fragment: u8,
    until: Instant,
    token_until: Instant,
    response_due: Option<Instant>,
    restarts: u8,
    phase: Phase,
    sent: bool,
    tx: Option<PendingTx<FRAME>>,
}

/// Retired wire tokens remain quarantined through the dialog deadline and the
/// explicitly supplied guard, including cancellation, rejection and restarts.
/// Exhaustion is temporary and exposes the next reusable-token deadline.
// CAPABILITY: wifi-roaming-and-service-discovery-gas-anqp-802-11u
pub struct Requester<const FRAME: usize, const RESPONSE: usize> {
    peer: PeerIdentity,
    config: Config,
    ids: Identities,
    tokens: Tokens,
    last_now: Instant,
    dialog: Option<Dialog<FRAME, RESPONSE>>,
    outcome: Option<Outcome>,
    result: Option<Bytes<RESPONSE>>,
}
impl<const FRAME: usize, const RESPONSE: usize> Requester<FRAME, RESPONSE> {
    pub fn new(peer: PeerIdentity, config: Config) -> Result<Self, Error> {
        Ok(Self {
            peer: peer.validate()?,
            config: config.validate()?,
            ids: Identities::new(),
            tokens: Tokens::new(),
            last_now: Instant::EPOCH,
            dialog: None,
            outcome: None,
            result: None,
        })
    }
    pub fn request(
        &mut self,
        category: Category,
        protocol: ProtocolId<'_>,
        query: &[u8],
        now: Instant,
    ) -> Result<DialogId, Error> {
        observe(&mut self.last_now, now)?;
        if self.dialog.is_some() || self.outcome.is_some() {
            return Err(Error::Busy);
        }
        let until = after(now, self.config.dialog_timeout)?;
        let token_until = after(until, self.config.token_reuse_guard)?;
        let advertisement = AdvertisementProtocol {
            info: ResponseInfo::new(0, false)?,
            protocol,
        };
        let token = self.tokens.available(now)?;
        let initial = Bytes::frame(Frame {
            category,
            dialog_token: token,
            body: Body::InitialRequest {
                advertisement,
                query,
            },
        })?;
        let outgoing = Bytes::copy(initial.bytes())?;
        let id = self.ids.dialog(self.peer)?;
        let tx = PendingTx::new(&mut self.ids, id, outgoing)?;
        self.tokens.reserve(token, token_until);
        self.result = None;
        self.dialog = Some(Dialog {
            id,
            category,
            token,
            initial,
            advertisement: None,
            response: Bytes::empty(),
            last_response: Bytes::empty(),
            expected_fragment: 0,
            until,
            token_until,
            response_due: None,
            restarts: 0,
            phase: Phase::Initial,
            sent: false,
            tx: Some(tx),
        });
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        let dialog = self.dialog.as_ref()?;
        dialog.tx.as_ref()?.transmission(dialog.category)
    }
    pub fn admitted(&mut self, id: TxId, now: Instant) -> Result<(), Error> {
        observe(&mut self.last_now, now)?;
        let dialog = self.dialog.as_mut().ok_or(Error::NoDialog)?;
        if now >= dialog.until {
            return Err(Error::Expired);
        }
        let due = after(now, self.config.response_timeout)?.min(dialog.until);
        dialog.tx.as_mut().ok_or(Error::WrongOperation)?.admit(id)?;
        dialog.sent = true;
        dialog.response_due = Some(due);
        Ok(())
    }
    /// A response may precede TX completion. Once it advances the dialog, the
    /// superseded completion is ignored and cannot affect a newer submission.
    pub fn tx_completed(
        &mut self,
        id: TxId,
        _outcome: TxOutcome,
        now: Instant,
    ) -> Result<Progress, Error> {
        observe(&mut self.last_now, now)?;
        let Some(dialog) = &mut self.dialog else {
            return Ok(Progress::Ignored);
        };
        let Some(tx) = &dialog.tx else {
            return Ok(Progress::Ignored);
        };
        if !tx.matches_completion(id)? {
            return Ok(Progress::Ignored);
        }
        if now >= dialog.until {
            let id = dialog.id;
            self.finish(Outcome::TimedOut { id });
            return Ok(Progress::Finished);
        }
        dialog.tx = None;
        // Even a failed TX can have reached the peer with a lost ACK. Preserve
        // its response window; poll owns any restart, never this completion.
        Ok(Progress::Waiting)
    }
    pub fn receive(
        &mut self,
        peer: PeerIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Progress, Error> {
        observe(&mut self.last_now, now)?;
        let Some(dialog) = &mut self.dialog else {
            return Ok(Progress::Ignored);
        };
        if peer != self.peer {
            return Ok(Progress::Ignored);
        }
        if now >= dialog.until {
            return Err(Error::Expired);
        }
        let frame = Frame::parse(bytes)?;
        if frame.category != dialog.category || frame.dialog_token != dialog.token {
            return Ok(Progress::Ignored);
        }
        // A provider may return the same delay on consecutive Comeback
        // exchanges. That is a new answer once another request is admitted.
        // Replayed data fragments remain duplicates even during that window.
        if dialog.last_response.bytes() == bytes
            && !matches!(frame.body,
            Body::ComebackResponse { comeback_delay_tu, .. } if comeback_delay_tu != 0 && dialog.sent && dialog.phase == Phase::Comeback)
        {
            return Ok(Progress::Ignored);
        }
        if !dialog.sent {
            return Ok(Progress::Ignored);
        }
        let (status, delay, advertisement, response, fragment) = match (dialog.phase, frame.body) {
            (
                Phase::Initial,
                Body::InitialResponse {
                    status,
                    comeback_delay_tu,
                    advertisement,
                    response,
                },
            ) => (status, comeback_delay_tu, advertisement, response, None),
            (
                Phase::Comeback,
                Body::ComebackResponse {
                    status,
                    fragment,
                    comeback_delay_tu,
                    advertisement,
                    response,
                },
            ) => (
                status,
                comeback_delay_tu,
                advertisement,
                response,
                Some(fragment),
            ),
            _ => return Ok(Progress::Ignored),
        };
        if status != Status::SUCCESS
            && !(fragment.is_some() && status == Status::RESPONSE_OUTSTANDING)
        {
            let id = dialog.id;
            self.finish(Outcome::Rejected { id, status });
            return Ok(Progress::Finished);
        }
        let requested = Frame::parse(dialog.initial.bytes())?
            .body
            .advertisement()
            .expect("initial advertisement");
        if advertisement.protocol != requested.protocol {
            return Err(Error::ProtocolChanged);
        }
        if let Some(previous) = &dialog.advertisement
            && previous.get() != advertisement
        {
            return Err(Error::ProtocolChanged);
        }
        if advertisement.info.limit_units() == 0 {
            return Err(Error::ResponseLimit);
        }
        let owned_advertisement = OwnedAdvertisement::new(advertisement)?;
        let retained = Bytes::copy(bytes)?;
        if delay != 0 {
            let at = comeback_at(now, delay)?;
            dialog.advertisement = Some(owned_advertisement);
            dialog.last_response = retained;
            dialog.phase = Phase::Delay(at);
            dialog.response_due = None;
            dialog.tx = None;
            dialog.sent = false;
            return Ok(Progress::Waiting);
        }
        if let Some(fragment) = fragment
            && fragment.id() != dialog.expected_fragment
        {
            return Err(Error::UnexpectedFragment {
                expected: dialog.expected_fragment,
                received: fragment.id(),
            });
        }
        let required = dialog
            .response
            .bytes()
            .len()
            .checked_add(response.len())
            .ok_or(Error::ResponseLimit)?;
        if let Some(limit) = advertisement.info.limit_octets()
            && required > limit
        {
            return Err(Error::ResponseLimit);
        }
        if required > RESPONSE {
            return Err(Error::Capacity {
                required,
                capacity: RESPONSE,
            });
        }
        let more = fragment.is_some_and(|fragment| fragment.more());
        // Prepare a follow-up and its identity before committing a fragment.
        let next_tx = if more {
            Some(PendingTx::new(
                &mut self.ids,
                dialog.id,
                Bytes::frame(Frame {
                    category: dialog.category,
                    dialog_token: dialog.token,
                    body: Body::ComebackRequest,
                })?,
            )?)
        } else {
            None
        };
        dialog.response.append(response)?;
        dialog.advertisement = Some(owned_advertisement);
        dialog.last_response = retained;
        dialog.response_due = None;
        dialog.tx = next_tx;
        dialog.sent = false;
        if more {
            dialog.expected_fragment += 1;
            dialog.phase = Phase::Comeback;
            Ok(Progress::FragmentAccepted)
        } else {
            let id = dialog.id;
            self.finish(Outcome::Complete { id });
            Ok(Progress::Finished)
        }
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        let Some(dialog) = self.dialog.as_ref() else {
            return self
                .outcome
                .is_none()
                .then(|| self.tokens.next_available_at(self.last_now))
                .flatten();
        };
        let phase = match dialog.phase {
            Phase::Delay(at) => Some(at),
            _ => dialog.response_due,
        };
        Some(phase.map_or(dialog.until, |at| at.min(dialog.until)))
    }
    pub fn poll(&mut self, now: Instant) -> Result<Progress, Error> {
        observe(&mut self.last_now, now)?;
        let Some(dialog) = &mut self.dialog else {
            return Ok(Progress::Ignored);
        };
        if now >= dialog.until {
            let id = dialog.id;
            self.finish(Outcome::TimedOut { id });
            return Ok(Progress::Finished);
        }
        if let Phase::Delay(at) = dialog.phase
            && now >= at
        {
            let tx = PendingTx::new(
                &mut self.ids,
                dialog.id,
                Bytes::frame(Frame {
                    category: dialog.category,
                    dialog_token: dialog.token,
                    body: Body::ComebackRequest,
                })?,
            )?;
            dialog.tx = Some(tx);
            dialog.phase = Phase::Comeback;
            dialog.sent = false;
            return Ok(Progress::Waiting);
        }
        if dialog.response_due.is_none_or(|due| now < due) {
            return Ok(Progress::Ignored);
        }
        if dialog.restarts == self.config.max_restarts {
            let id = dialog.id;
            self.finish(Outcome::TimedOut { id });
            return Ok(Progress::Finished);
        }
        let token = match self.tokens.available(now) {
            Ok(token) => token,
            Err(Error::TokensExhausted) => {
                dialog.response_due = Some(
                    self.tokens
                        .next_available_at(now)
                        .expect("exhausted token space")
                        .min(dialog.until),
                );
                return Ok(Progress::Waiting);
            }
            Err(error) => return Err(error),
        };
        let mut frame = Frame::parse(dialog.initial.bytes())?;
        frame.dialog_token = token;
        let initial = Bytes::frame(frame)?;
        let tx = PendingTx::new(&mut self.ids, dialog.id, Bytes::copy(initial.bytes())?)?;
        self.tokens.reserve(token, dialog.token_until);
        dialog.token = token;
        dialog.initial = initial;
        dialog.tx = Some(tx);
        dialog.restarts += 1;
        dialog.phase = Phase::Initial;
        dialog.sent = false;
        dialog.response_due = None;
        dialog.advertisement = None;
        dialog.expected_fragment = 0;
        dialog.response.clear();
        dialog.last_response.clear();
        Ok(Progress::Restarted)
    }
    pub fn cancel(&mut self, id: DialogId) -> Result<(), Error> {
        if self.dialog.as_ref().is_none_or(|dialog| dialog.id != id) {
            return Err(Error::WrongOperation);
        }
        self.finish(Outcome::Cancelled { id });
        Ok(())
    }
    fn finish(&mut self, outcome: Outcome) {
        let dialog = self.dialog.take().expect("live dialog");
        self.result = matches!(outcome, Outcome::Complete { .. }).then_some(dialog.response);
        self.outcome = Some(outcome);
    }
    /// Terminal outcome is emitted once. It must be drained before a new query.
    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }
    /// Only a complete response is exposed; partial assembly is never a result.
    pub fn response(&self) -> Option<&[u8]> {
        self.result.as_ref().map(Bytes::bytes)
    }
}
