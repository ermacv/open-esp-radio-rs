use super::*;
use crate::{RequestEvent, Transmission, TxOutcome, TxPhase, deadline, exchange::Exchange};
use oer_ieee80211_mac::management::is_group_address;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmsEntryState {
    Active,
    Uncertain,
    Retired {
        last_sequence: LastSequenceControl,
        until: Instant,
    },
}
#[derive(Clone)]
struct ClientEntry<const BYTES: usize> {
    id: u8,
    attributes: Body<BYTES>,
    state: DmsEntryState,
}
struct ClientChange<const BYTES: usize> {
    index: usize,
    entry: ClientEntry<BYTES>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmsStationEvent {
    Ignored,
    Updated { request: Option<OperationId> },
    RecoveryRequired { id: OperationId },
    RequestCancelled { id: OperationId },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupReception {
    Ignored,
    Keep,
    Discard,
    SequenceUnavailable { last_sequence: LastSequenceControl },
    RecoveryRequired,
}

/// Client grant list and terminal sequence bounds. History remains bounded by
/// the caller's maximum in-flight MSDU lifetime. An uncertain removal never
/// silently continues discarding the AP's only remaining group delivery.
// CAPABILITY: wifi-tsf-beacon-monitoring-and-power-saving-directed-multicast-service-802-11v
pub struct DmsStation<const BYTES: usize, const ATTR_BYTES: usize, const STREAMS: usize> {
    exchange: Exchange<BYTES>,
    entries: [Option<ClientEntry<ATTR_BYTES>>; STREAMS],
    retirement_lifetime: Duration,
    recovery: Option<Body<BYTES>>,
    last_update: Instant,
}
impl<const B: usize, const A: usize, const S: usize> DmsStation<B, A, S> {
    pub fn new(
        link: LinkIdentity,
        timeout: Duration,
        retirement_lifetime: Duration,
    ) -> Result<Self, DmsError> {
        deadline(Instant::EPOCH, retirement_lifetime)?;
        if S == 0 {
            return Err(DmsError::Full);
        }
        Ok(Self {
            exchange: Exchange::new(link, timeout)?,
            entries: core::array::from_fn(|_| None),
            retirement_lifetime,
            recovery: None,
            last_update: Instant::EPOCH,
        })
    }
    fn time(&self, now: Instant) -> Result<(), DmsError> {
        if now < self.last_update {
            Err(Error::TimeBeforeOperation.into())
        } else {
            Ok(())
        }
    }
    fn effective<'a>(
        &'a self,
        changes: &'a [Option<ClientChange<A>>],
        index: usize,
    ) -> Option<&'a ClientEntry<A>> {
        changes
            .iter()
            .flatten()
            .rev()
            .find(|change| change.index == index)
            .map(|change| &change.entry)
            .or(self.entries[index].as_ref())
    }
    pub fn entries(&self) -> impl Iterator<Item = (u8, DmsEntryState, Elements<'_>)> {
        self.entries.iter().flatten().map(|entry| {
            (
                entry.id,
                entry.state,
                Elements::parse(entry.attributes.bytes()).expect("validated attributes"),
            )
        })
    }
    pub fn recovery_request(&self) -> Option<DmsRequest<'_>> {
        self.recovery
            .as_ref()
            .map(|body| DmsRequest::parse(body.bytes()).expect("retained request"))
    }
    pub fn request(
        &mut self,
        request: DmsRequest<'_>,
        now: Instant,
    ) -> Result<OperationId, DmsError> {
        self.time(now)?;
        request.validate()?;
        if request.descriptors()?.count() > S {
            return Err(DmsError::OperationCapacity);
        }
        let body = Body::encode(|out| request.encode(out))?;
        if self
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.bytes() != body.bytes())
        {
            return Err(DmsError::RecoveryRequired);
        }
        let mut adds = 0_usize;
        for (index, descriptor) in request.descriptors()?.enumerate() {
            if !descriptor.request_type.is_known() {
                return Err(DmsError::UnsupportedRequest(descriptor.request_type.0));
            }
            for earlier in request.descriptors()?.take(index) {
                if (descriptor.dms_id != 0 && descriptor.dms_id == earlier.dms_id)
                    || (descriptor.request_type == DmsRequestType::ADD
                        && earlier.request_type == DmsRequestType::ADD
                        && equivalent_dms_classifiers(descriptor.attributes, earlier.attributes)?)
                {
                    return Err(DmsError::ConflictingRequest);
                }
            }
            if descriptor.request_type == DmsRequestType::ADD {
                admit_classifiers(descriptor.attributes)?;
                adds += 1;
            } else if !self.entries.iter().flatten().any(|entry| {
                entry.id == descriptor.dms_id
                    && !matches!(entry.state, DmsEntryState::Retired { .. })
            }) {
                return Err(DmsError::UnknownStream(descriptor.dms_id));
            }
        }
        // Retired records retain sequence evidence; a Remove is not treated as
        // immediately freeing its slot for an Add in the same transaction.
        let free = self
            .entries
            .iter()
            .filter(|entry| {
                entry.as_ref().is_none_or(
                    |entry| matches!(entry.state,DmsEntryState::Retired {until,..} if now>=until),
                )
            })
            .count();
        let already = request
            .descriptors()?
            .filter(|descriptor| descriptor.request_type == DmsRequestType::ADD)
            .filter(|descriptor| {
                self.entries.iter().flatten().any(|entry| {
                    !matches!(entry.state, DmsEntryState::Retired { .. })
                        && equivalent_dms_classifiers(
                            Elements::parse(entry.attributes.bytes())
                                .expect("validated attributes"),
                            descriptor.attributes,
                        )
                        .expect("validated classifiers")
                })
            })
            .count();
        if adds.saturating_sub(already) > free {
            return Err(DmsError::Full);
        }
        let id = self.exchange.start(body, now)?;
        self.last_update = now;
        Ok(id)
    }
    pub fn retry(&mut self, now: Instant) -> Result<OperationId, DmsError> {
        let body = self
            .recovery
            .as_ref()
            .ok_or(DmsError::RecoveryRequired)?
            .clone();
        self.request(DmsRequest::parse(body.bytes())?, now)
    }
    pub fn transmission(&self) -> Option<Transmission<'_>> {
        self.exchange.transmission()
    }
    pub fn admitted(&mut self, id: OperationId, now: Instant) -> Result<(), DmsError> {
        self.time(now)?;
        self.exchange.admitted(id, now)?;
        self.last_update = now;
        Ok(())
    }
    fn uncertain(
        &mut self,
        body: Body<B>,
        id: OperationId,
        admitted: bool,
    ) -> Result<DmsStationEvent, DmsError> {
        if !admitted {
            return Ok(DmsStationEvent::RequestCancelled { id });
        }
        let request = DmsRequest::parse(body.bytes())?;
        for descriptor in request
            .descriptors()?
            .filter(|descriptor| descriptor.request_type == DmsRequestType::REMOVE)
        {
            for entry in self.entries.iter_mut().flatten().filter(|entry| {
                entry.id == descriptor.dms_id
                    && !matches!(entry.state, DmsEntryState::Retired { .. })
            }) {
                entry.state = DmsEntryState::Uncertain;
            }
        }
        self.recovery = Some(body);
        Ok(DmsStationEvent::RecoveryRequired { id })
    }
    pub fn tx_completed(
        &mut self,
        id: OperationId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<DmsStationEvent, DmsError> {
        let Some(pending) = self
            .exchange
            .pending
            .as_ref()
            .filter(|pending| pending.id == id)
        else {
            return Ok(DmsStationEvent::Ignored);
        };
        self.time(now)?;
        let body = pending.body.clone();
        let admitted = pending.phase != TxPhase::Ready;
        let event = self.exchange.completed(id, outcome, now)?;
        self.last_update = now;
        match event {
            RequestEvent::TimedOut { .. } | RequestEvent::TxFailed { .. } => {
                self.uncertain(body, id, admitted)
            }
            _ => Ok(DmsStationEvent::Ignored),
        }
    }
    pub fn receive(
        &mut self,
        link: LinkIdentity,
        bytes: &[u8],
        now: Instant,
    ) -> Result<DmsStationEvent, DmsError> {
        if link != self.exchange.ids.link {
            return Ok(DmsStationEvent::Ignored);
        }
        self.time(now)?;
        let response = DmsResponse::parse(bytes)?;
        let unsolicited = response.dialog_token == 0;
        if !unsolicited && !self.exchange.matches(link, response.dialog_token, now)? {
            return Ok(DmsStationEvent::Ignored);
        }
        let request = if unsolicited {
            None
        } else {
            Some(DmsRequest::parse(
                self.exchange
                    .pending
                    .as_ref()
                    .expect("matching request")
                    .body
                    .bytes(),
            )?)
        };
        let id = self
            .exchange
            .pending
            .as_ref()
            .filter(|_| !unsolicited)
            .map(|pending| pending.id);
        let mut changes: [Option<ClientChange<A>>; S] = core::array::from_fn(|_| None);
        let mut matched = [false; S];
        let mut count = 0;
        let mut denied_adds = 0;
        for status in response
            .statuses()?
            .filter(|status| status.response_type != DmsResponseType::DENIED)
        {
            if count == S {
                return Err(DmsError::OperationCapacity);
            }
            if !status.response_type.is_known() {
                return Err(DmsError::UnsupportedResponse(status.response_type.0));
            }
            if unsolicited && status.response_type != DmsResponseType::TERMINATE {
                return Err(DmsError::UnexpectedResponse);
            }
            let existing = (0..S)
                .find(|index| {
                    self.effective(&changes, *index).is_some_and(|entry| {
                        entry.id == status.dms_id
                            && !matches!(entry.state, DmsEntryState::Retired { .. })
                    })
                })
                .or_else(|| {
                    (status.response_type == DmsResponseType::TERMINATE)
                        .then(|| {
                            (0..S).find(|index| {
                                self.effective(&changes, *index)
                                    .is_some_and(|entry| entry.id == status.dms_id)
                            })
                        })
                        .flatten()
                });
            let source = if let Some(request) = request {
                let found = request
                    .descriptors()?
                    .enumerate()
                    .find(|(index, descriptor)| {
                        !matched[*index]
                            && if descriptor.request_type == DmsRequestType::ADD {
                                status.response_type == DmsResponseType::ACCEPT
                                    && equivalent_dms_classifiers(
                                        descriptor.attributes,
                                        status.attributes,
                                    )
                                    .expect("validated attributes")
                            } else {
                                descriptor.dms_id == status.dms_id
                                    && ((descriptor.request_type == DmsRequestType::REMOVE
                                        && status.response_type == DmsResponseType::TERMINATE)
                                        || (descriptor.request_type == DmsRequestType::CHANGE
                                            && status.response_type == DmsResponseType::ACCEPT))
                            }
                    });
                let (index, source) = found.ok_or(DmsError::UnexpectedResponse)?;
                matched[index] = true;
                Some(source)
            } else {
                None
            };
            let (index, entry) = if status.response_type == DmsResponseType::TERMINATE {
                let Some(index) = existing else {
                    if unsolicited {
                        continue;
                    }
                    return Err(DmsError::UnknownStream(status.dms_id));
                };
                let entry = self.effective(&changes, index).expect("existing stream");
                let incoming = LastSequenceControl::parse(status.last_sequence_control)?;
                let (last_sequence, until) = if let DmsEntryState::Retired {
                    last_sequence,
                    until,
                } = entry.state
                {
                    let sequence = match (last_sequence, incoming) {
                        (
                            LastSequenceControl::Sequence(old),
                            LastSequenceControl::Sequence(new),
                        ) if old != new => return Err(DmsError::ConflictingStream(status.dms_id)),
                        (LastSequenceControl::Sequence(_), _) => last_sequence,
                        _ => incoming,
                    };
                    (sequence, until)
                } else {
                    (incoming, deadline(now, self.retirement_lifetime)?)
                };
                (
                    index,
                    ClientEntry {
                        id: entry.id,
                        attributes: entry.attributes.clone(),
                        state: DmsEntryState::Retired {
                            last_sequence,
                            until,
                        },
                    },
                )
            } else {
                let source = source.ok_or(DmsError::UnexpectedResponse)?;
                let attributes = if source.request_type == DmsRequestType::CHANGE {
                    let index = existing.ok_or(DmsError::UnknownStream(status.dms_id))?;
                    if status.attributes != source.attributes {
                        return Err(DmsError::UnexpectedResponse);
                    }
                    merge_attributes(
                        Elements::parse(
                            self.effective(&changes, index)
                                .expect("existing stream")
                                .attributes
                                .bytes(),
                        )?,
                        status.attributes,
                    )?
                } else {
                    if status.attributes != source.attributes {
                        return Err(DmsError::UnexpectedResponse);
                    }
                    admit_classifiers(status.attributes)?;
                    attributes_body(status.attributes)?
                };
                if let Some(index) = existing
                    && source.request_type == DmsRequestType::ADD
                    && !equivalent_dms_classifiers(
                        Elements::parse(
                            self.effective(&changes, index)
                                .expect("existing stream")
                                .attributes
                                .bytes(),
                        )?,
                        status.attributes,
                    )?
                {
                    return Err(DmsError::ConflictingStream(status.dms_id));
                }
                let index=existing.or_else(||(0..S).find(|index|self.effective(&changes,*index).is_none_or(|entry|matches!(entry.state,DmsEntryState::Retired {until,..} if now>=until)))).ok_or(DmsError::Full)?;
                (
                    index,
                    ClientEntry {
                        id: status.dms_id,
                        attributes,
                        state: DmsEntryState::Active,
                    },
                )
            };
            changes[count] = Some(ClientChange { index, entry });
            count += 1;
        }
        for status in response
            .statuses()?
            .filter(|status| status.response_type == DmsResponseType::DENIED)
        {
            if unsolicited {
                return Err(DmsError::UnexpectedResponse);
            }
            let request = request.expect("solicited response");
            if status.dms_id == 0 {
                denied_adds += 1;
            } else {
                let (index, _) = request
                    .descriptors()?
                    .enumerate()
                    .find(|(index, descriptor)| {
                        !matched[*index]
                            && descriptor.request_type == DmsRequestType::CHANGE
                            && descriptor.dms_id == status.dms_id
                    })
                    .ok_or(DmsError::UnexpectedResponse)?;
                matched[index] = true;
            }
        }
        if let Some(request) = request {
            let unmatched = request
                .descriptors()?
                .enumerate()
                .filter(|(index, _)| !matched[*index]);
            let mut missing_adds = 0;
            for (_, descriptor) in unmatched {
                if descriptor.request_type != DmsRequestType::ADD {
                    return Err(DmsError::IncompleteResponse);
                }
                missing_adds += 1;
            }
            if missing_adds != denied_adds {
                return Err(DmsError::IncompleteResponse);
            }
            self.exchange.accept(bytes)?;
            self.recovery = None;
        } else {
            self.exchange.remember_response(bytes)?;
        }
        for change in changes.into_iter().flatten() {
            self.entries[change.index] = Some(change.entry);
        }
        self.last_update = now;
        Ok(DmsStationEvent::Updated { request: id })
    }
    /// Full response, including denial alternatives that cannot necessarily be
    /// mapped to one Add when several descriptors were denied in the same frame.
    pub fn response(&self) -> Option<DmsResponse<'_>> {
        self.exchange
            .response()
            .map(|bytes| DmsResponse::parse(bytes).expect("validated response"))
    }
    pub fn cancel(&mut self, now: Instant) -> Result<DmsStationEvent, DmsError> {
        self.time(now)?;
        let Some(pending) = &self.exchange.pending else {
            return Ok(DmsStationEvent::Ignored);
        };
        let id = pending.id;
        let admitted = pending.phase != TxPhase::Ready;
        let body = pending.body.clone();
        self.exchange.cancel();
        self.last_update = now;
        self.uncertain(body, id, admitted)
    }
    pub fn poll(&mut self, now: Instant) -> Result<DmsStationEvent, DmsError> {
        self.time(now)?;
        if let Some(pending) = &self.exchange.pending
            && !pending.live(now)?
        {
            let id = pending.id;
            let admitted = pending.phase != TxPhase::Ready;
            let body = pending.body.clone();
            self.exchange.poll(now)?;
            self.last_update = now;
            return self.uncertain(body, id, admitted);
        }
        for slot in &mut self.entries {
            if slot.as_ref().is_some_and(
                |entry| matches!(entry.state,DmsEntryState::Retired {until,..} if now>=until),
            ) {
                *slot = None;
            }
        }
        self.last_update = now;
        Ok(DmsStationEvent::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.exchange
            .next_deadline()
            .into_iter()
            .chain(self.entries.iter().flatten().filter_map(|entry| {
                if let DmsEntryState::Retired { until, .. } = entry.state {
                    Some(until)
                } else {
                    None
                }
            }))
            .min()
    }
    pub fn receive_group(
        &mut self,
        link: LinkIdentity,
        packet: DmsPacket,
        sequence: u16,
        now: Instant,
    ) -> Result<GroupReception, DmsError> {
        if link != self.exchange.ids.link {
            return Ok(GroupReception::Ignored);
        }
        self.time(now)?;
        let sequence = SequenceNumber::new(sequence).ok_or(DmsError::InvalidSequence)?;
        if !is_group_address(packet.destination) {
            self.last_update = now;
            return Ok(GroupReception::Keep);
        }
        if let Some(recovery) = &self.recovery {
            for descriptor in DmsRequest::parse(recovery.bytes())?
                .descriptors()?
                .filter(|descriptor| descriptor.request_type == DmsRequestType::ADD)
            {
                if group_address_matches(descriptor.attributes, packet)? {
                    self.last_update = now;
                    return Ok(GroupReception::RecoveryRequired);
                }
            }
        }
        let mut result = GroupReception::Keep;
        let mut clear = [false; S];
        for (index, entry) in self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry)))
        {
            if !group_address_matches(Elements::parse(entry.attributes.bytes())?, packet)? {
                continue;
            }
            match entry.state {
                DmsEntryState::Active => {
                    self.last_update = now;
                    return Ok(GroupReception::Discard);
                }
                DmsEntryState::Uncertain => result = GroupReception::RecoveryRequired,
                DmsEntryState::Retired { until, .. } if now >= until => clear[index] = true,
                DmsEntryState::Retired {
                    last_sequence: LastSequenceControl::Sequence(last),
                    ..
                } => {
                    let delta = SequenceNumber::new(last)
                        .ok_or(DmsError::InvalidSequence)?
                        .forward_distance(sequence);
                    if delta == SequenceNumber::HALF_SPACE {
                        return Err(DmsError::AmbiguousSequence);
                    }
                    if delta == 0 || delta > SequenceNumber::HALF_SPACE {
                        self.last_update = now;
                        return Ok(GroupReception::Discard);
                    }
                    clear[index] = true;
                }
                DmsEntryState::Retired { last_sequence, .. } => {
                    if result == GroupReception::Keep {
                        result = GroupReception::SequenceUnavailable { last_sequence };
                    }
                }
            }
        }
        for (index, clear) in clear.into_iter().enumerate() {
            if clear {
                self.entries[index] = None;
            }
        }
        self.last_update = now;
        Ok(result)
    }
}
