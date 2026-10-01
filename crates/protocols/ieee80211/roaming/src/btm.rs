//! Per-peer BSS Transition Management owners for station and access point.
//!
//! The station asks its caller to evaluate a proposal. Candidate preferences
//! are hints, not proof of security/channel compatibility. A transition action
//! follows an explicitly accepted proposal and acknowledged response; it never
//! performs the transition. The AP reports responses and leaves steering and
//! disassociation policy to its caller.

use crate::{
    Body, Error, Identities, LinkIdentity, OperationId, Pending, Transmission, TxOutcome, deadline,
};
use oer_ieee80211_mac::management::IEEE_TIME_UNIT_MICROS;
use oer_ieee80211_mac::roaming as wire;
use oer_ieee80211_mac::roaming::{
    BtmQuery, BtmRequest, BtmRequestMode, BtmResponse, BtmStatus, Elements, MacAddress,
    NEIGHBOR_REPORT_ELEMENT_ID, NeighborReport,
};
use oer_time::{Duration, Instant};

/// Measured timing of the current BSS, supplied by the integration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssTiming {
    pub beacon_interval_tu: u16,
    /// The first TBTT at or after reception, on the image's monotonic clock.
    pub next_tbtt: Instant,
    /// Exact projection of a supplied BSS termination TSF into monotonic time.
    /// Required when the request includes BSS Termination Duration; the core
    /// never substitutes a guessed relationship between clock epochs.
    pub termination_deadline: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProposalTiming {
    pub candidates_valid_until: Instant,
    pub decision_deadline: Instant,
    pub disassociation_deadline: Option<Instant>,
    pub termination_deadline: Option<Instant>,
}
impl BssTiming {
    fn proposal(
        self,
        request: BtmRequest<'_>,
        now: Instant,
        decision_timeout: Duration,
    ) -> Result<ProposalTiming, Error> {
        if self.beacon_interval_tu == 0 {
            return Err(Error::InvalidBeaconInterval);
        }
        let interval = u64::from(self.beacon_interval_tu) * IEEE_TIME_UNIT_MICROS;
        if self.next_tbtt < now
            || self.next_tbtt
                > now
                    .checked_add(Duration::from_micros(interval))
                    .ok_or(Error::TimeOverflow)?
        {
            return Err(Error::InvalidBeaconTiming);
        }
        let candidates_valid_until = now
            .checked_add(Duration::from_micros(
                interval * u64::from(request.validity_interval),
            ))
            .ok_or(Error::TimeOverflow)?;
        let disassociation_deadline = if request.disassociation_timer != 0
            && (request
                .mode
                .contains(BtmRequestMode::DISASSOCIATION_IMMINENT)
                || request
                    .mode
                    .contains(BtmRequestMode::ESS_DISASSOCIATION_IMMINENT))
        {
            Some(
                self.next_tbtt
                    .checked_add(Duration::from_micros(
                        interval * u64::from(request.disassociation_timer - 1),
                    ))
                    .ok_or(Error::TimeOverflow)?,
            )
        } else {
            None
        };
        let termination_deadline = if request.termination.is_some() {
            Some(
                self.termination_deadline
                    .ok_or(Error::MissingTerminationDeadline)?,
            )
        } else {
            None
        };
        let decision_deadline = disassociation_deadline
            .into_iter()
            .chain(termination_deadline)
            .fold(deadline(now, decision_timeout)?, Instant::min);
        Ok(ProposalTiming {
            candidates_valid_until,
            decision_deadline,
            disassociation_deadline,
            termination_deadline,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateSource {
    /// Select one unexpired, non-excluded candidate from this request.
    Request,
    /// The caller independently scanned and validated this target. This does
    /// not reuse an expired advertised candidate as a scan observation or
    /// bypass a live exclusion or an abridged candidate list.
    Scan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BtmDecision<'a> {
    Accept {
        target: MacAddress,
        source: CandidateSource,
        elements: Elements<'a>,
    },
    Reject {
        status: BtmStatus,
        termination_delay_minutes: u8,
        elements: Elements<'a>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationEvent {
    Ignored,
    Proposal {
        id: OperationId,
        timing: ProposalTiming,
        solicited: bool,
        superseded: Option<OperationId>,
    },
    CandidatesExpired {
        id: OperationId,
    },
    DecisionTimedOut {
        id: OperationId,
    },
    TransitionReady {
        id: OperationId,
        target: MacAddress,
    },
    ResponseSent {
        id: OperationId,
        status: BtmStatus,
    },
    TxFailed {
        id: OperationId,
    },
    TimedOut {
        id: OperationId,
    },
    Cancelled {
        id: OperationId,
    },
}

struct Proposal<const BYTES: usize> {
    id: OperationId,
    request: Body<BYTES>,
    received: Instant,
    timing: ProposalTiming,
    candidates_expired: bool,
}
struct Reply<const BYTES: usize> {
    pending: Pending<BYTES>,
    request: Body<BYTES>,
    target: Option<MacAddress>,
    status: BtmStatus,
}
enum Phase<const BYTES: usize> {
    Idle,
    Query(Pending<BYTES>),
    Proposal(Proposal<BYTES>),
    Reply(Reply<BYTES>),
}
struct History<const BYTES: usize> {
    request: Body<BYTES>,
    reply: Body<BYTES>,
    until: Instant,
}

/// Station BTM query, proposal/decision and response state, for one association.
// CAPABILITY: wifi-roaming-and-service-discovery-bss-transition-management-802-11v
pub struct BtmStation<const BYTES: usize> {
    ids: Identities,
    timeout: Duration,
    decision_timeout: Duration,
    phase: Phase<BYTES>,
    history: Option<History<BYTES>>,
}
impl<const BYTES: usize> BtmStation<BYTES> {
    pub fn new(
        link: LinkIdentity,
        timeout: Duration,
        decision_timeout: Duration,
    ) -> Result<Self, Error> {
        deadline(Instant::EPOCH, timeout)?;
        deadline(Instant::EPOCH, decision_timeout)?;
        if BYTES < BtmResponse::ACCEPT_FIXED_LEN {
            return Err(Error::FrameTooLarge {
                required: BtmResponse::ACCEPT_FIXED_LEN,
                capacity: BYTES,
            });
        }
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            decision_timeout,
            phase: Phase::Idle,
            history: None,
        })
    }
    pub fn query(&mut self, now: Instant, query: BtmQuery<'_>) -> Result<OperationId, Error> {
        if !matches!(self.phase, Phase::Idle) {
            return Err(Error::Busy);
        }
        let body = Body::encode(|output| query.encode(output))?;
        let expires = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.phase = Phase::Query(Pending::new(id, body, now, expires));
        Ok(id)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        match &self.phase {
            Phase::Query(pending) => pending.transmission(),
            Phase::Reply(reply) => reply.pending.transmission(),
            _ => None,
        }
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        match &mut self.phase {
            Phase::Query(pending) => pending.admit(id, now),
            Phase::Reply(reply) => reply.pending.admit(id, now),
            _ => Err(Error::NoPendingOperation),
        }
    }
    /// Receive an authenticated request from the bound association epoch.
    /// A distinct request replaces a proposal/query and gets a fresh local ID.
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
        timing: BssTiming,
    ) -> Result<StationEvent, Error> {
        if link != self.ids.link {
            return Ok(StationEvent::Ignored);
        }
        let request = BtmRequest::parse(bytes)?;
        if request.mode.0 & !BtmRequestMode::KNOWN_BITS != 0 {
            return Err(Error::UnsupportedRequestMode(request.mode.0));
        }
        let existing = match &self.phase {
            Phase::Proposal(proposal) => Some((&proposal.request, proposal.received)),
            Phase::Reply(reply) => Some((&reply.request, reply.pending.started)),
            _ => None,
        };
        if let Some((existing, started)) = existing {
            if now < started {
                return Err(Error::TimeBeforeOperation);
            }
            if existing.bytes() == bytes {
                return Ok(StationEvent::Ignored);
            }
            if existing.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token {
                return Err(Error::ConflictingDialog);
            }
        }
        if let Some(history) = &self.history
            && now < history.until
            && history.request.bytes()[wire::DIALOG_TOKEN_OFFSET] == request.dialog_token
        {
            if history.request.bytes() != bytes {
                return Err(Error::ConflictingDialog);
            }
            if !matches!(self.phase, Phase::Idle) {
                return Ok(StationEvent::Ignored);
            }
            let body = history.reply.clone();
            let original = history.request.clone();
            let expires = history.until;
            let id = self.ids.issue()?;
            let status = BtmResponse::parse(body.bytes())?.status;
            self.phase = Phase::Reply(Reply {
                pending: Pending::new(id, body, now, expires),
                request: original,
                target: None,
                status,
            });
            return Ok(StationEvent::Ignored);
        }
        let proposal_timing = timing.proposal(request, now, self.decision_timeout)?;
        let body = Body::copy(bytes)?;
        let solicited = matches!(&self.phase, Phase::Query(pending) if pending.live(now)? && pending.received(request.dialog_token));
        let superseded = self.operation();
        let id = self.ids.issue()?;
        self.phase = Phase::Proposal(Proposal {
            id,
            request: body,
            received: now,
            timing: proposal_timing,
            candidates_expired: false,
        });
        self.history = None;
        Ok(StationEvent::Proposal {
            id,
            timing: proposal_timing,
            solicited,
            superseded,
        })
    }
    pub fn proposal(&self) -> Option<(OperationId, BtmRequest<'_>, ProposalTiming)> {
        let Phase::Proposal(proposal) = &self.phase else {
            return None;
        };
        Some((
            proposal.id,
            BtmRequest::parse(proposal.request.bytes()).expect("validated proposal"),
            proposal.timing,
        ))
    }
    pub fn decide(
        &mut self,
        id: OperationId,
        decision: BtmDecision<'_>,
        now: Instant,
    ) -> Result<OperationId, Error> {
        let Phase::Proposal(proposal) = &self.phase else {
            return Err(Error::NoPendingOperation);
        };
        if proposal.id != id {
            return Err(Error::WrongOperation);
        }
        if now < proposal.received {
            return Err(Error::TimeBeforeOperation);
        }
        if now >= proposal.timing.decision_deadline {
            return Err(Error::NoPendingOperation);
        }
        let request = BtmRequest::parse(proposal.request.bytes())?;
        let (target, status, delay, elements) = match decision {
            BtmDecision::Accept {
                target,
                source,
                elements,
            } => {
                if !crate::valid_peer_address(target) || target == self.ids.link.peer {
                    return Err(Error::InvalidTarget);
                }
                let requires_offer = source == CandidateSource::Request
                    || request.mode.contains(BtmRequestMode::ABRIDGED);
                if now >= proposal.timing.candidates_valid_until {
                    if requires_offer {
                        return Err(Error::CandidateExpired);
                    }
                } else {
                    let mut found = None;
                    for element in request
                        .elements
                        .iter()
                        .filter(|element| element.id == NEIGHBOR_REPORT_ELEMENT_ID)
                    {
                        let report = NeighborReport::parse(element.body)?;
                        if report.bssid == target && found.replace(report).is_some() {
                            return Err(Error::ConflictingCandidates);
                        }
                    }
                    match found {
                        Some(report) if report.preference()? == Some(0) => {
                            return Err(Error::CandidateExcluded);
                        }
                        None if requires_offer => return Err(Error::CandidateNotOffered),
                        _ => {}
                    }
                }
                (Some(target), BtmStatus::ACCEPT, 0, elements)
            }
            BtmDecision::Reject {
                status,
                termination_delay_minutes,
                elements,
            } => {
                if status == BtmStatus::ACCEPT {
                    return Err(Error::InvalidTarget);
                }
                (None, status, termination_delay_minutes, elements)
            }
        };
        let response = Body::encode(|output| {
            BtmResponse {
                dialog_token: request.dialog_token,
                status,
                termination_delay_minutes: delay,
                target_bssid: target,
                elements,
            }
            .encode(output)
        })?;
        let expires = proposal
            .timing
            .disassociation_deadline
            .into_iter()
            .chain(proposal.timing.termination_deadline)
            .fold(deadline(now, self.timeout)?, Instant::min);
        let original = proposal.request.clone();
        let tx = self.ids.issue()?;
        self.phase = Phase::Reply(Reply {
            pending: Pending::new(tx, response, now, expires),
            request: original,
            target,
            status,
        });
        Ok(tx)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<StationEvent, Error> {
        let pending = match &mut self.phase {
            Phase::Query(pending) => pending,
            Phase::Reply(reply) => &mut reply.pending,
            _ => return Ok(StationEvent::Ignored),
        };
        if id != pending.id {
            return Ok(StationEvent::Ignored);
        }
        if !pending.live(now)? {
            self.phase = Phase::Idle;
            return Ok(StationEvent::TimedOut { id });
        }
        if !pending.complete(id, outcome)? {
            return Ok(StationEvent::Ignored);
        }
        if outcome == TxOutcome::Failed {
            self.phase = Phase::Idle;
            return Ok(StationEvent::TxFailed { id });
        }
        if matches!(self.phase, Phase::Query(_)) {
            return Ok(StationEvent::Ignored);
        }
        let Phase::Reply(reply) = core::mem::replace(&mut self.phase, Phase::Idle) else {
            unreachable!()
        };
        self.history = Some(History {
            request: reply.request,
            reply: reply.pending.body,
            until: reply.pending.deadline,
        });
        Ok(match reply.target {
            Some(target) => StationEvent::TransitionReady { id, target },
            None => StationEvent::ResponseSent {
                id,
                status: reply.status,
            },
        })
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Idle => self.history.as_ref().map(|history| history.until),
            Phase::Query(pending) => Some(pending.deadline),
            Phase::Reply(reply) => Some(reply.pending.deadline),
            Phase::Proposal(proposal) => Some(
                if !proposal.candidates_expired
                    && BtmRequest::parse(proposal.request.bytes())
                        .expect("validated proposal")
                        .mode
                        .contains(BtmRequestMode::CANDIDATES)
                {
                    proposal
                        .timing
                        .decision_deadline
                        .min(proposal.timing.candidates_valid_until)
                } else {
                    proposal.timing.decision_deadline
                },
            ),
        }
    }
    pub fn poll(&mut self, now: Instant) -> Result<StationEvent, Error> {
        let id = self.operation();
        match &mut self.phase {
            Phase::Idle => {
                if self
                    .history
                    .as_ref()
                    .is_some_and(|history| now >= history.until)
                {
                    self.history = None;
                }
            }
            Phase::Query(pending) => {
                if !pending.live(now)? {
                    self.phase = Phase::Idle;
                    return Ok(StationEvent::TimedOut {
                        id: id.expect("query ID"),
                    });
                }
            }
            Phase::Reply(reply) => {
                if !reply.pending.live(now)? {
                    self.phase = Phase::Idle;
                    return Ok(StationEvent::TimedOut {
                        id: id.expect("reply ID"),
                    });
                }
            }
            Phase::Proposal(proposal) => {
                if now < proposal.received {
                    return Err(Error::TimeBeforeOperation);
                }
                if now >= proposal.timing.decision_deadline {
                    let id = proposal.id;
                    let token = proposal.request.bytes()[wire::DIALOG_TOKEN_OFFSET];
                    let original = proposal.request.clone();
                    let cutoff = proposal
                        .timing
                        .disassociation_deadline
                        .into_iter()
                        .chain(proposal.timing.termination_deadline)
                        .min();
                    if cutoff.is_some_and(|cutoff| now >= cutoff) {
                        self.phase = Phase::Idle;
                        return Ok(StationEvent::TimedOut { id });
                    }
                    let body = Body::encode(|output| {
                        BtmResponse {
                            dialog_token: token,
                            status: BtmStatus::REJECT_NO_SUITABLE_CANDIDATES,
                            termination_delay_minutes: 0,
                            target_bssid: None,
                            elements: Elements::EMPTY,
                        }
                        .encode(output)
                    })?;
                    let expires = cutoff
                        .into_iter()
                        .fold(deadline(now, self.timeout)?, Instant::min);
                    let tx = self.ids.issue()?;
                    self.phase = Phase::Reply(Reply {
                        pending: Pending::new(tx, body, now, expires),
                        request: original,
                        target: None,
                        status: BtmStatus::REJECT_NO_SUITABLE_CANDIDATES,
                    });
                    return Ok(StationEvent::DecisionTimedOut { id });
                }
                if !proposal.candidates_expired
                    && now >= proposal.timing.candidates_valid_until
                    && BtmRequest::parse(proposal.request.bytes())?
                        .mode
                        .contains(BtmRequestMode::CANDIDATES)
                {
                    proposal.candidates_expired = true;
                    return Ok(StationEvent::CandidatesExpired { id: proposal.id });
                }
            }
        }
        Ok(StationEvent::Ignored)
    }
    fn operation(&self) -> Option<OperationId> {
        match &self.phase {
            Phase::Idle => None,
            Phase::Query(pending) => Some(pending.id),
            Phase::Proposal(proposal) => Some(proposal.id),
            Phase::Reply(reply) => Some(reply.pending.id),
        }
    }
    pub fn cancel(&mut self) -> StationEvent {
        self.history = None;
        match self.operation() {
            Some(id) => {
                self.phase = Phase::Idle;
                StationEvent::Cancelled { id }
            }
            None => StationEvent::Ignored,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointEvent {
    Ignored,
    TxFailed,
    TimedOut,
    Cancelled,
    Response {
        status: BtmStatus,
        termination_delay_minutes: u8,
        target_bssid: Option<MacAddress>,
    },
}

/// AP-side unsolicited or query-solicited BTM request, one per bound peer.
// CAPABILITY: wifi-roaming-and-service-discovery-bss-transition-management-802-11v
pub struct BtmAccessPoint<const BYTES: usize> {
    ids: Identities,
    timeout: Duration,
    pending: Option<Pending<BYTES>>,
    response: Option<Body<BYTES>>,
}
impl<const BYTES: usize> BtmAccessPoint<BYTES> {
    pub fn new(link: LinkIdentity, timeout: Duration) -> Result<Self, Error> {
        deadline(Instant::EPOCH, timeout)?;
        Ok(Self {
            ids: Identities::new(link),
            timeout,
            pending: None,
            response: None,
        })
    }
    pub fn receive_query<'a>(
        &self,
        link: LinkIdentity,
        bytes: &'a [u8],
    ) -> Result<Option<BtmQuery<'a>>, Error> {
        if link != self.ids.link {
            return Ok(None);
        }
        Ok(Some(BtmQuery::parse(bytes)?))
    }
    pub fn request(&mut self, now: Instant, request: BtmRequest<'_>) -> Result<OperationId, Error> {
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        if request.mode.0 & !BtmRequestMode::KNOWN_BITS != 0 {
            return Err(Error::UnsupportedRequestMode(request.mode.0));
        }
        let body = Body::encode(|output| request.encode(output))?;
        let expires = deadline(now, self.timeout)?;
        let id = self.ids.issue()?;
        self.pending = Some(Pending::new(id, body, now, expires));
        self.response = None;
        Ok(id)
    }
    pub fn respond_to_query(
        &mut self,
        now: Instant,
        query: BtmQuery<'_>,
        request: BtmRequest<'_>,
    ) -> Result<OperationId, Error> {
        query.validate()?;
        if query.dialog_token != request.dialog_token {
            return Err(Error::Wire(
                oer_ieee80211_mac::roaming::WireError::InconsistentFields,
            ));
        }
        self.request(now, request)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.pending.as_ref()?.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), Error> {
        self.pending
            .as_mut()
            .ok_or(Error::NoPendingOperation)?
            .admit(id, now)
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<AccessPointEvent, Error> {
        let Some(pending) = &mut self.pending else {
            return Ok(AccessPointEvent::Ignored);
        };
        if pending.id != id {
            return Ok(AccessPointEvent::Ignored);
        }
        if !pending.live(now)? {
            self.pending = None;
            return Ok(AccessPointEvent::TimedOut);
        }
        if pending.complete(id, outcome)? && outcome == TxOutcome::Failed {
            self.pending = None;
            return Ok(AccessPointEvent::TxFailed);
        }
        Ok(AccessPointEvent::Ignored)
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<AccessPointEvent, Error> {
        if link != self.ids.link {
            return Ok(AccessPointEvent::Ignored);
        }
        let Some(pending) = &self.pending else {
            return Ok(AccessPointEvent::Ignored);
        };
        if !pending.live(now)? {
            self.pending = None;
            return Ok(AccessPointEvent::TimedOut);
        }
        let response = BtmResponse::parse(bytes)?;
        if !pending.received(response.dialog_token) {
            return Ok(AccessPointEvent::Ignored);
        }
        let body = Body::copy(bytes)?;
        let event = AccessPointEvent::Response {
            status: response.status,
            termination_delay_minutes: response.termination_delay_minutes,
            target_bssid: response.target_bssid,
        };
        self.response = Some(body);
        self.pending = None;
        Ok(event)
    }
    pub fn response(&self) -> Option<BtmResponse<'_>> {
        self.response
            .as_ref()
            .map(|body| BtmResponse::parse(body.bytes()).expect("validated response"))
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.deadline)
    }
    pub fn poll(&mut self, now: Instant) -> Result<AccessPointEvent, Error> {
        if let Some(pending) = &self.pending
            && !pending.live(now)?
        {
            self.pending = None;
            return Ok(AccessPointEvent::TimedOut);
        }
        Ok(AccessPointEvent::Ignored)
    }
    pub fn cancel(&mut self) -> AccessPointEvent {
        if self.pending.take().is_some() {
            AccessPointEvent::Cancelled
        } else {
            AccessPointEvent::Ignored
        }
    }
}

#[cfg(test)]
mod tests;
