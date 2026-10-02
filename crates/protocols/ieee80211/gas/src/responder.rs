//! Bounded concurrent GAS dialogs and asynchronous query-provider handoff.
//! Receive sequence identities distinguish MAC retries from the next Comeback
//! Request. A reply advances only after ACK or proof from the next request.
use super::*;
use gas::{
    AdvertisementProtocol, Body, Fragment, Frame, MAX_FRAGMENT_COUNT, ProtocolId, ResponseInfo,
    Status,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    /// Fixed lease for provider work, delivery and duplicate-response caching.
    pub dialog_timeout: Duration,
    /// Query payload per Comeback Response, excluding all MAC/GAS overhead.
    pub fragment_payload_limit: usize,
    pub comeback_delay_tu: u16,
    pub response_info: ResponseInfo,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Ignored,
    Query { id: DialogId },
    ResponseQueued { id: DialogId },
    ProviderReady { id: DialogId },
    ProviderRejected { id: DialogId },
    ReplayQueued { id: DialogId },
    Delivered { id: DialogId },
    DeliveryFailed { id: DialogId, tx: TxId },
    TimedOut { id: DialogId },
    CacheExpired { id: DialogId },
    Cancelled { id: DialogId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Advance {
    offset: usize,
    next_fragment: u8,
    finished: bool,
}

struct Dialog<const QUERY: usize, const RESPONSE: usize, const FRAME: usize> {
    id: DialogId,
    token: u8,
    category: Category,
    request: Bytes<QUERY>,
    advertisement: OwnedAdvertisement,
    request_info: ResponseInfo,
    until: Instant,
    response: Option<Bytes<RESPONSE>>,
    failure: Option<Status>,
    initial_cache: Bytes<FRAME>,
    initial_advance: Option<Advance>,
    last_request: Option<SequenceNumber>,
    last_response: Bytes<FRAME>,
    initial_admitted: bool,
    offset: usize,
    next_fragment: u8,
    finished: bool,
    advance: Option<Advance>,
    tx_advance: Option<Advance>,
    tx: Option<PendingTx<FRAME>>,
}

/// One advertisement service with bounded slots across peers and wire tokens.
/// Full storage is an explicit error; live work and replay leases are never
/// evicted. Additional services have their own providers and routing policy.
pub struct Responder<
    const SLOTS: usize,
    const QUERY: usize,
    const RESPONSE: usize,
    const FRAME: usize,
> {
    advertisement: OwnedAdvertisement,
    config: Config,
    ids: Identities,
    last_now: Instant,
    dialogs: [Option<Dialog<QUERY, RESPONSE, FRAME>>; SLOTS],
}
impl<const SLOTS: usize, const QUERY: usize, const RESPONSE: usize, const FRAME: usize>
    Responder<SLOTS, QUERY, RESPONSE, FRAME>
{
    pub fn new(protocol: ProtocolId<'_>, config: Config) -> Result<Self, Error> {
        if SLOTS == 0
            || config.dialog_timeout == Duration::ZERO
            || config.fragment_payload_limit == 0
            || config.fragment_payload_limit > u16::MAX as usize
            || config.comeback_delay_tu == 0
            || config.response_info.limit_units() == 0
        {
            return Err(Error::InvalidConfiguration);
        }
        let value = AdvertisementProtocol {
            info: config.response_info,
            protocol,
        };
        let overhead = Frame {
            category: Category::Public,
            dialog_token: 0,
            body: Body::ComebackResponse {
                status: Status::SUCCESS,
                fragment: Fragment::new(0, false)?,
                comeback_delay_tu: 0,
                advertisement: value,
                response: &[],
            },
        }
        .encoded_len()?;
        if FRAME < overhead || config.fragment_payload_limit > FRAME - overhead {
            return Err(Error::InvalidConfiguration);
        }
        Ok(Self {
            advertisement: OwnedAdvertisement::new(value)?,
            config,
            ids: Identities::new(),
            last_now: Instant::EPOCH,
            dialogs: core::array::from_fn(|_| None),
        })
    }
    /// `sequence` and `retry` come from the validated management header.
    /// Deliver duplicate requests here so the cached response can be replayed.
    /// Sequence equality alone is not a retry: the twelve-bit space can wrap.
    /// MAC receive admission keeps retry ordering inside the shared sequence
    /// comparison horizon and retires retries of superseded submissions.
    pub fn receive(
        &mut self,
        peer: PeerIdentity,
        sequence: SequenceNumber,
        retry: bool,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        peer.validate()?;
        let frame = Frame::parse(bytes)?;
        let found = self.dialogs.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|dialog| dialog.id.peer == peer && dialog.token == frame.dialog_token)
        });
        if let Some(index) = found {
            let dialog = self.dialogs[index].as_mut().expect("found dialog");
            if now >= dialog.until {
                return Err(Error::Expired);
            }
            if dialog.category != frame.category {
                return Err(Error::DialogConflict);
            }
            match frame.body {
                Body::InitialRequest {
                    advertisement,
                    query,
                } => {
                    let old = dialog.advertisement.get();
                    if query != dialog.request.bytes()
                        || advertisement.protocol != old.protocol
                        || advertisement.info != dialog.request_info
                    {
                        return Err(Error::DialogConflict);
                    }
                    if dialog.initial_cache.bytes().is_empty() || dialog.tx.is_some() {
                        return Ok(Event::Ignored);
                    }
                    let body = Bytes::copy(dialog.initial_cache.bytes())?;
                    let tx = PendingTx::new(&mut self.ids, dialog.id, body)?;
                    dialog.tx = Some(tx);
                    // Replaying Initial does not commit a queued fragment.
                    dialog.tx_advance = if dialog.last_request.is_none() && !dialog.finished {
                        dialog.initial_advance
                    } else {
                        None
                    };
                    Ok(Event::ReplayQueued { id: dialog.id })
                }
                Body::ComebackRequest => self.comeback(index, sequence, retry),
                _ => Ok(Event::Ignored),
            }
        } else {
            match frame.body {
                Body::InitialRequest {
                    advertisement,
                    query,
                } => {
                    let index =
                        self.dialogs
                            .iter()
                            .position(Option::is_none)
                            .ok_or(Error::Capacity {
                                required: SLOTS + 1,
                                capacity: SLOTS,
                            })?;
                    let request = Bytes::copy(query)?;
                    let response_advertisement = OwnedAdvertisement::new(AdvertisementProtocol {
                        info: self.config.response_info,
                        protocol: advertisement.protocol,
                    })?;
                    let until = after(now, self.config.dialog_timeout)?;
                    let unsupported = advertisement.protocol != self.advertisement.get().protocol;
                    // Prepare a protocol rejection before reserving the slot.
                    let rejection = if unsupported {
                        Some(Bytes::frame(Frame {
                            category: frame.category,
                            dialog_token: frame.dialog_token,
                            body: Body::InitialResponse {
                                status: Status::PROTOCOL_NOT_SUPPORTED,
                                comeback_delay_tu: 0,
                                advertisement: response_advertisement.get(),
                                response: &[],
                            },
                        })?)
                    } else {
                        None
                    };
                    let id = self.ids.dialog(peer)?;
                    let tx = if let Some(body) = &rejection {
                        Some(PendingTx::new(
                            &mut self.ids,
                            id,
                            Bytes::copy(body.bytes())?,
                        )?)
                    } else {
                        None
                    };
                    let initial_advance = unsupported.then_some(Advance {
                        offset: 0,
                        next_fragment: 0,
                        finished: true,
                    });
                    self.dialogs[index] = Some(Dialog {
                        id,
                        token: frame.dialog_token,
                        category: frame.category,
                        request,
                        advertisement: response_advertisement,
                        request_info: advertisement.info,
                        until,
                        response: None,
                        failure: unsupported.then_some(Status::PROTOCOL_NOT_SUPPORTED),
                        initial_cache: rejection.unwrap_or_else(Bytes::empty),
                        initial_advance,
                        last_request: None,
                        last_response: Bytes::empty(),
                        initial_admitted: false,
                        offset: 0,
                        next_fragment: 0,
                        finished: false,
                        advance: initial_advance,
                        tx_advance: initial_advance,
                        tx,
                    });
                    if unsupported {
                        Ok(Event::ResponseQueued { id })
                    } else {
                        Ok(Event::Query { id })
                    }
                }
                Body::ComebackRequest => self.no_outstanding(peer, sequence, frame, now),
                _ => Ok(Event::Ignored),
            }
        }
    }
    /// Provider input remains available through deferred response preparation.
    pub fn query(&self, id: DialogId) -> Option<&[u8]> {
        let dialog = self
            .dialogs
            .iter()
            .flatten()
            .find(|dialog| dialog.id == id)?;
        (!dialog.finished && dialog.response.is_none() && dialog.failure.is_none())
            .then_some(dialog.request.bytes())
    }
    /// Send an empty Initial Response while the external provider works.
    pub fn defer(&mut self, id: DialogId, now: Instant) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        let index = self.index(id, now)?;
        let dialog = self.dialogs[index].as_mut().expect("live dialog");
        if !dialog.initial_cache.bytes().is_empty() || dialog.response.is_some() {
            return Err(Error::Busy);
        }
        let frame = Frame {
            category: dialog.category,
            dialog_token: dialog.token,
            body: Body::InitialResponse {
                status: Status::SUCCESS,
                comeback_delay_tu: self.config.comeback_delay_tu,
                advertisement: dialog.advertisement.get(),
                response: &[],
            },
        };
        let body = Bytes::frame(frame)?;
        let tx = PendingTx::new(&mut self.ids, id, Bytes::copy(body.bytes())?)?;
        dialog.initial_cache = body;
        dialog.tx = Some(tx);
        Ok(Event::ResponseQueued { id })
    }
    /// Retain the complete provider response atomically. No truncation or live
    /// eviction occurs. Oversized wire responses are rejected with status 63.
    pub fn respond(&mut self, id: DialogId, response: &[u8], now: Instant) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        let index = self.index(id, now)?;
        let dialog = self.dialogs[index].as_mut().expect("live dialog");
        if dialog.finished || dialog.response.is_some() || dialog.failure.is_some() {
            return Err(Error::Busy);
        }
        let count = response
            .len()
            .div_ceil(self.config.fragment_payload_limit)
            .max(1);
        let limit = dialog.advertisement.get().info.limit_octets();
        if count > MAX_FRAGMENT_COUNT || limit.is_some_and(|limit| response.len() > limit) {
            return self.reject(id, Status::RESPONSE_TOO_LARGE, now);
        }
        let retained = Bytes::copy(response)?;
        let queues_initial = dialog.initial_cache.bytes().is_empty();
        if queues_initial {
            let fits = response.len() <= self.config.fragment_payload_limit;
            let frame = Frame {
                category: dialog.category,
                dialog_token: dialog.token,
                body: Body::InitialResponse {
                    status: Status::SUCCESS,
                    comeback_delay_tu: if fits {
                        0
                    } else {
                        self.config.comeback_delay_tu
                    },
                    advertisement: dialog.advertisement.get(),
                    response: if fits { response } else { &[] },
                },
            };
            let body = Bytes::frame(frame)?;
            let tx = PendingTx::new(&mut self.ids, id, Bytes::copy(body.bytes())?)?;
            dialog.initial_cache = body;
            dialog.tx = Some(tx);
            dialog.advance = fits.then_some(Advance {
                offset: response.len(),
                next_fragment: 0,
                finished: true,
            });
            dialog.initial_advance = dialog.advance;
            dialog.tx_advance = dialog.advance;
        }
        dialog.response = Some(retained);
        Ok(if queues_initial {
            Event::ResponseQueued { id }
        } else {
            Event::ProviderReady { id }
        })
    }
    pub fn reject(&mut self, id: DialogId, status: Status, now: Instant) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        if status == Status::SUCCESS || status == Status::RESPONSE_OUTSTANDING {
            return Err(Error::InvalidConfiguration);
        }
        let index = self.index(id, now)?;
        let dialog = self.dialogs[index].as_mut().expect("live dialog");
        if dialog.finished || dialog.response.is_some() || dialog.failure.is_some() {
            return Err(Error::Busy);
        }
        if !dialog.initial_cache.bytes().is_empty() {
            // Initial may already be admitted or received. Preserve that reply;
            // the next Comeback carries the provider's terminal failure.
            dialog.failure = Some(status);
            return Ok(Event::ProviderRejected { id });
        }
        let frame = Frame {
            category: dialog.category,
            dialog_token: dialog.token,
            body: Body::InitialResponse {
                status,
                comeback_delay_tu: 0,
                advertisement: dialog.advertisement.get(),
                response: &[],
            },
        };
        let body = Bytes::frame(frame)?;
        let tx = PendingTx::new(&mut self.ids, id, Bytes::copy(body.bytes())?)?;
        dialog.initial_cache = body;
        dialog.tx = Some(tx);
        dialog.advance = Some(Advance {
            offset: 0,
            next_fragment: 0,
            finished: true,
        });
        dialog.initial_advance = dialog.advance;
        dialog.tx_advance = dialog.advance;
        dialog.failure = Some(status);
        Ok(Event::ResponseQueued { id })
    }
    fn index(&self, id: DialogId, now: Instant) -> Result<usize, Error> {
        let index = self
            .dialogs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|dialog| dialog.id == id))
            .ok_or(Error::WrongOperation)?;
        if now >= self.dialogs[index].as_ref().expect("found dialog").until {
            return Err(Error::Expired);
        }
        Ok(index)
    }
    fn comeback(
        &mut self,
        index: usize,
        sequence: SequenceNumber,
        retry: bool,
    ) -> Result<Event, Error> {
        let dialog = self.dialogs[index].as_mut().expect("live dialog");
        if !dialog.initial_admitted {
            return Err(Error::NotAdmitted);
        }
        if retry
            && dialog
                .last_request
                .is_some_and(|last| last.forward_distance(sequence) == SequenceNumber::HALF_SPACE)
        {
            return Err(Error::AmbiguousSequence);
        }
        // A delayed retry of an older request cannot prove receipt of the
        // currently queued fragment. Modular ordering uses the shared MAC
        // sequence horizon; receive admission owns that horizon's validity.
        if retry
            && dialog
                .last_request
                .is_some_and(|last| sequence.precedes(last))
        {
            return Ok(Event::Ignored);
        }
        if retry && dialog.last_request == Some(sequence) {
            if dialog.last_response.bytes().is_empty() {
                return Err(Error::ResponseNotReady);
            }
            if dialog.tx.is_some() {
                return Ok(Event::Ignored);
            }
            let tx = PendingTx::new(
                &mut self.ids,
                dialog.id,
                Bytes::copy(dialog.last_response.bytes())?,
            )?;
            dialog.tx = Some(tx);
            dialog.tx_advance = dialog.advance;
            return Ok(Event::ReplayQueued { id: dialog.id });
        }
        // A new Comeback proves the preceding admitted reply reached the peer,
        // even if its ACK/completion is still queued or reported lost.
        if let Some(tx) = &dialog.tx
            && !tx.admitted
        {
            return Err(Error::Busy);
        }
        let offset = dialog
            .advance
            .map_or(dialog.offset, |advance| advance.offset);
        let next_fragment = dialog
            .advance
            .map_or(dialog.next_fragment, |advance| advance.next_fragment);
        let was_finished = dialog.finished;
        let finished = dialog.finished || dialog.advance.is_some_and(|advance| advance.finished);
        let (response, fragment, delay, status, advance) = if finished {
            (
                &[][..],
                Fragment::new(0, false)?,
                0,
                Status::NO_OUTSTANDING_REQUEST,
                None,
            )
        } else if let Some(status) = dialog.failure {
            (
                &[][..],
                Fragment::new(0, false)?,
                0,
                status,
                Some(Advance {
                    offset,
                    next_fragment,
                    finished: true,
                }),
            )
        } else {
            match &dialog.response {
                None => (
                    &[][..],
                    Fragment::new(0, false)?,
                    self.config.comeback_delay_tu,
                    Status::RESPONSE_OUTSTANDING,
                    None,
                ),
                Some(response) => {
                    let end = offset
                        .saturating_add(self.config.fragment_payload_limit)
                        .min(response.bytes().len());
                    let more = end < response.bytes().len();
                    let fragment = Fragment::new(next_fragment, more)?;
                    let next_fragment = if more {
                        next_fragment.checked_add(1).ok_or(Error::FragmentLimit)?
                    } else {
                        next_fragment
                    };
                    (
                        &response.bytes()[offset..end],
                        fragment,
                        0,
                        Status::SUCCESS,
                        Some(Advance {
                            offset: end,
                            next_fragment,
                            finished: !more,
                        }),
                    )
                }
            }
        };
        let frame = Frame {
            category: dialog.category,
            dialog_token: dialog.token,
            body: Body::ComebackResponse {
                status,
                fragment,
                comeback_delay_tu: delay,
                advertisement: dialog.advertisement.get(),
                response,
            },
        };
        let body = Bytes::frame(frame)?;
        let tx = PendingTx::new(&mut self.ids, dialog.id, Bytes::copy(body.bytes())?)?;
        dialog.offset = offset;
        dialog.next_fragment = next_fragment;
        dialog.finished = finished;
        dialog.tx = Some(tx);
        dialog.last_response = body;
        dialog.last_request = Some(sequence);
        dialog.advance = advance;
        dialog.tx_advance = advance;
        Ok(if finished && !was_finished {
            Event::Delivered { id: dialog.id }
        } else {
            Event::ResponseQueued { id: dialog.id }
        })
    }
    fn no_outstanding(
        &mut self,
        peer: PeerIdentity,
        sequence: SequenceNumber,
        request: Frame<'_>,
        now: Instant,
    ) -> Result<Event, Error> {
        let index = self
            .dialogs
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Capacity {
                required: SLOTS + 1,
                capacity: SLOTS,
            })?;
        let until = after(now, self.config.dialog_timeout)?;
        let id = self.ids.dialog(peer)?;
        let advertisement = OwnedAdvertisement::new(self.advertisement.get())?;
        let frame = Frame {
            category: request.category,
            dialog_token: request.dialog_token,
            body: Body::ComebackResponse {
                status: Status::NO_OUTSTANDING_REQUEST,
                fragment: Fragment::new(0, false)?,
                comeback_delay_tu: 0,
                advertisement: advertisement.get(),
                response: &[],
            },
        };
        let body = Bytes::frame(frame)?;
        let tx = PendingTx::new(&mut self.ids, id, Bytes::copy(body.bytes())?)?;
        self.dialogs[index] = Some(Dialog {
            id,
            token: request.dialog_token,
            category: request.category,
            request: Bytes::empty(),
            advertisement,
            request_info: ResponseInfo::new(0, false)?,
            until,
            response: None,
            failure: Some(Status::NO_OUTSTANDING_REQUEST),
            initial_cache: Bytes::empty(),
            initial_advance: None,
            last_request: Some(sequence),
            last_response: body,
            initial_admitted: true,
            offset: 0,
            next_fragment: 0,
            finished: false,
            advance: Some(Advance {
                offset: 0,
                next_fragment: 0,
                finished: true,
            }),
            tx_advance: Some(Advance {
                offset: 0,
                next_fragment: 0,
                finished: true,
            }),
            tx: Some(tx),
        });
        Ok(Event::ResponseQueued { id })
    }
    /// May return several queued transmissions across dialogs. The caller
    /// admits each by its own ID; no protocol storage becomes radio-owned.
    pub fn transmissions(&self) -> impl Iterator<Item = Transmission<'_>> {
        self.dialogs
            .iter()
            .flatten()
            .filter_map(|dialog| dialog.tx.as_ref()?.transmission(dialog.category))
    }
    pub fn admitted(&mut self, id: TxId, now: Instant) -> Result<(), Error> {
        observe(&mut self.last_now, now)?;
        let index = self.index(id.dialog, now)?;
        let dialog = self.dialogs[index].as_mut().expect("live dialog");
        let tx = dialog.tx.as_mut().ok_or(Error::WrongOperation)?;
        tx.admit(id)?;
        if Frame::parse(tx.body.bytes())?.body.action() == gas::Action::InitialResponse {
            dialog.initial_admitted = true;
        }
        Ok(())
    }
    pub fn tx_completed(
        &mut self,
        id: TxId,
        outcome: TxOutcome,
        now: Instant,
    ) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        let Some(index) = self
            .dialogs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|dialog| dialog.id == id.dialog))
        else {
            return Ok(Event::Ignored);
        };
        let dialog = self.dialogs[index].as_mut().expect("found dialog");
        let Some(tx) = &dialog.tx else {
            return Ok(Event::Ignored);
        };
        if !tx.matches_completion(id)? {
            return Ok(Event::Ignored);
        }
        if now >= dialog.until {
            let dialog = self.dialogs[index].take().expect("expired dialog");
            return Ok(if dialog.finished {
                Event::CacheExpired { id: dialog.id }
            } else {
                Event::TimedOut { id: dialog.id }
            });
        }
        dialog.tx = None;
        if outcome == TxOutcome::Failed {
            return Ok(Event::DeliveryFailed {
                id: dialog.id,
                tx: id,
            });
        }
        if let Some(advance) = dialog.tx_advance.take() {
            if dialog.advance == Some(advance) {
                dialog.advance = None;
            }
            dialog.offset = advance.offset;
            dialog.next_fragment = advance.next_fragment;
            if advance.finished && !dialog.finished {
                dialog.finished = true;
                return Ok(Event::Delivered { id: dialog.id });
            }
        }
        Ok(Event::Ignored)
    }
    pub fn next_deadline(&self) -> Option<Instant> {
        self.dialogs
            .iter()
            .flatten()
            .map(|dialog| dialog.until)
            .min()
    }
    /// Drain repeatedly at `now`: one expiration releases one slot and emits
    /// one event, including finished replay caches without a second delivery.
    pub fn poll(&mut self, now: Instant) -> Result<Event, Error> {
        observe(&mut self.last_now, now)?;
        let Some(index) = self
            .dialogs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|dialog| now >= dialog.until))
        else {
            return Ok(Event::Ignored);
        };
        let dialog = self.dialogs[index].take().expect("expired dialog");
        Ok(if dialog.finished {
            Event::CacheExpired { id: dialog.id }
        } else {
            Event::TimedOut { id: dialog.id }
        })
    }
    pub fn cancel(&mut self, id: DialogId) -> Result<Event, Error> {
        let index = self
            .dialogs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|dialog| dialog.id == id))
            .ok_or(Error::WrongOperation)?;
        self.dialogs[index] = None;
        Ok(Event::Cancelled { id })
    }
}
