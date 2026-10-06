#![no_std]
#![forbid(unsafe_code)]

//! The driver of the IEEE 802.11 transmit planner over a lower-MAC port.
//!
//! [`UpperMacTx`] runs one frame exchange of `oer-ieee80211-upper-mac`'s
//! [`TxPlanner`] over any [`Ieee80211LowerMacPort`]: it encodes each planned
//! attempt into a port buffer, submits it, awaits its completion from the
//! port's [`EventRouter`], feeds the completion to the planner and repeats
//! until the planner reports the exchange's end. One attempt is one
//! submission.
//!
//! The [`EventRouter`] is the port's one event consumer: it hands each
//! completion to the exchange whose identity it carries, so several
//! [`UpperMacTx`] (one per access category) run their exchanges
//! concurrently over one port, and received frames, lifecycle terminals and
//! extension events go to queues of their own. When the port lost events
//! before an attempt's completion arrived, the exchange cancels the attempt
//! by its identity and either gets its completion or learns that it ended
//! in the gap.
//! Aggregates need the [`LowerMacAmpdu`] extension, which
//! [`UpperMacTx::send_ampdu`] requires as a bound.
//!
//! The caller keeps its encoded MPDUs, header to end of body without the
//! FCS, and the driver copies them into a fresh buffer for every attempt,
//! setting the Retry bit where the plan says so; a retransmission therefore
//! repeats the first encoding's sequence number and CCMP packet number.
//! The driver waits only on the router; it never reads another clock
//! (the planner ages aggregates on the port's radio clock) and runs under any
//! executor.

#[cfg(test)]
extern crate std;

pub mod aggregate;
pub mod client;
pub mod frame;
pub mod reorder;
pub mod router;
pub mod rx_hold;

pub use router::{
    AttachError, Attachment, Awaited, EventRouter, PORT_BACKLOG, PORT_EXCHANGES, PortRouter,
    ROUTER_VIFS, Registration, RouterFull,
};

use oer_ieee80211_lower_mac::{
    AirReservation, AmpduBuffer, AmpduPayload, Backoff, CancelError, CoexPriority,
    Ieee80211LowerMacPort, KeySelector, LowerMacAirReservation, LowerMacAmpdu, PhyRate, Protection,
    ReclaimError, Refused, SubmitError, TxAttempt, TxBody as PortTxBody, TxBuffer, TxCompletion,
    TxId, TxPayload, TxPower, TxResponse, VifId,
};
use oer_ieee80211_mac::block_ack::encode_block_ack_request;
use oer_ieee80211_mac::qos::WmmAccessCategory;
use oer_ieee80211_softmac::BackoffEntropy;
use oer_ieee80211_upper_mac::{
    AttemptContent, HeTxopRtsBudget, RateLadder, TxAttemptPlan, TxBody, TxExchange, TxPlanError,
    TxPlanner, TxReport, TxRequest, TxStep, ampdu::set_retry_bit,
};

/// Why an exchange ended without a report.
#[derive(Debug, Eq, PartialEq)]
pub enum UpperMacTxError<E> {
    /// The planner refused the request.
    Plan(TxPlanError),
    /// The frames do not match the request: a subframe is missing, or a
    /// frame is too short to carry the Frame Control and addresses the
    /// driver reads.
    InvalidFrames,
    /// The port lent no buffer.
    NoBuffer,
    /// The port refused an attempt; nothing was sent.
    Refused(SubmitError),
    /// Every completion slot of the router is registered.
    RouterFull,
    /// The port lost the attempt's completion: the attempt ended, its
    /// outcome is unknown, and the exchange ends without a report.
    CompletionLost { attempt: TxId },
    /// The port reported its terminal poisoned event.
    Poisoned,
    /// The port kept the bodies of an attempt that ended.
    BodiesHeld { attempt: TxId },
    /// The port cannot serve.
    Port(E),
}

/// One MPDU to send: the `header` a service encoded and, after it, the
/// `body` it hands the port by ownership, such as a network frame's payload
/// (a management frame is all header). Each attempt writes the header into
/// the buffer the port lends and hands the port the body, which comes back
/// after the attempt for the next one.
#[derive(Debug, Eq, PartialEq)]
pub struct TxMpdu<'h, O> {
    pub header: &'h [u8],
    pub body: Option<O>,
}

impl<'h, O> TxMpdu<'h, O> {
    /// A frame encoded whole, as a management frame is.
    pub const fn whole(frame: &'h [u8]) -> Self {
        Self {
            header: frame,
            body: None,
        }
    }
}

/// The subframes of one A-MPDU and how the port sends them: subframe `i` is
/// `headers[i]`, then `bodies[i]`'s octets. An attempt takes the bodies of
/// the subframes it carries and puts them back after its completion; the
/// bodies stay the caller's when the exchange ends.
#[derive(Debug)]
pub struct AmpduFrames<'f, O> {
    pub headers: &'f [&'f [u8]],
    pub bodies: &'f mut [Option<O>],
    pub key: KeySelector,
    /// The recipient's Minimum MPDU Start Spacing, IEEE encoding 0-7.
    pub min_mpdu_start_spacing: u8,
}

/// The octets of an MPDU of `header` and `body`.
fn mpdu_len<O: PortTxBody>(header: &[u8], body: Option<&O>) -> usize {
    header.len() + body.map_or(0, |body| body.bytes().len())
}

/// The caller's subframe the `nth` subframe of an aggregate of `selected`
/// carries: the index of the `nth` set bit.
fn nth_selected(selected: u64, nth: usize) -> Option<usize> {
    let mut remaining = selected;
    for _ in 0..nth {
        remaining &= remaining.checked_sub(1)?;
    }
    (remaining != 0).then(|| remaining.trailing_zeros() as usize)
}

/// A submitted single attempt and the caller's subframe whose body it took.
type Submitted<'r, 'p, P, const WAITERS: usize, const RX: usize> =
    (Registration<'r, 'p, P, WAITERS, RX>, Option<usize>);

/// The transmit driver of one interface over a lower-MAC port, sharing the
/// port's [`EventRouter`] with other drivers.
pub struct UpperMacTx<'r, 'p, P: Ieee80211LowerMacPort, B, const WAITERS: usize, const RX: usize> {
    router: &'r EventRouter<'p, P, WAITERS, RX>,
    port: &'p P,
    vif: VifId,
    planner: TxPlanner<B>,
}

impl<'r, 'p, P, B, const WAITERS: usize, const RX: usize> UpperMacTx<'r, 'p, P, B, WAITERS, RX>
where
    P: Ieee80211LowerMacPort,
    B: HeTxopRtsBudget,
{
    /// A driver of `vif`'s transmissions through the port of `router`,
    /// whose attempt identities the router allocates.
    pub const fn new(
        router: &'r EventRouter<'p, P, WAITERS, RX>,
        vif: VifId,
        planner: TxPlanner<B>,
    ) -> Self {
        Self {
            router,
            port: router.port(),
            vif,
            planner,
        }
    }

    pub fn planner(&self) -> &TxPlanner<B> {
        &self.planner
    }

    pub fn planner_mut(&mut self) -> &mut TxPlanner<B> {
        &mut self.planner
    }

    /// Send one MPDU: `frame`, the request's [`TxBody::Mpdu`] describing
    /// it, protected with `key`. Its body goes back to its owner when the
    /// exchange ends.
    pub async fn send_mpdu(
        &mut self,
        frame: TxMpdu<'_, P::TxBody>,
        key: KeySelector,
        request: TxRequest,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<TxReport, UpperMacTxError<P::Error>> {
        if !matches!(request.body, TxBody::Mpdu(_)) {
            return Err(UpperMacTxError::InvalidFrames);
        }
        let queue = self
            .router
            .queue(self.port.capabilities().tx_queue(request.access_category))
            .map_err(|_| UpperMacTxError::RouterFull)?;
        queue.ready().await.map_err(|_| UpperMacTxError::Poisoned)?;
        let headers = [frame.header];
        let mut bodies = [frame.body];
        let (mut exchange, mut plan) = self
            .planner
            .begin(request, ladder, entropy)
            .map_err(UpperMacTxError::Plan)?;
        loop {
            let (registration, origin) =
                self.submit_single(&exchange, &plan, &headers, &mut bodies, key)?;
            let completion = self
                .completion(&registration, &mut bodies, |_| origin)
                .await?;
            match self.step(&mut exchange, &completion, ladder, entropy)? {
                TxStep::Attempt(next) => plan = next,
                TxStep::Done(report) => return Ok(report),
            }
        }
    }

    /// Plan the next step from a completion, on the port's radio clock.
    fn step(
        &mut self,
        exchange: &mut TxExchange,
        completion: &TxCompletion,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<TxStep, UpperMacTxError<P::Error>> {
        let now = self.port.now().map_err(UpperMacTxError::Port)?;
        Ok(self
            .planner
            .complete(exchange, completion, now, ladder, entropy))
    }

    /// Submit an attempt that carries one MPDU: the request's MPDU, one
    /// subframe of it, or a BlockAckReq for it.
    /// Submit an attempt that carries one MPDU: the request's MPDU, one
    /// subframe of it, or a BlockAckReq for it; the caller's subframe whose
    /// body it took, if any.
    fn submit_single(
        &mut self,
        exchange: &TxExchange,
        plan: &TxAttemptPlan,
        headers: &[&[u8]],
        bodies: &mut [Option<P::TxBody>],
        key: KeySelector,
    ) -> Result<Submitted<'r, 'p, P, WAITERS, RX>, UpperMacTxError<P::Error>> {
        let request = exchange.request();
        let request_frame: [u8; oer_ieee80211_mac::block_ack::BLOCK_ACK_REQUEST_LEN];
        let (header, origin, set_retry, response, key): (
            &[u8],
            Option<usize>,
            bool,
            TxResponse,
            KeySelector,
        ) = match plan.content {
            AttemptContent::Mpdu { set_retry_bit } => {
                let TxBody::Mpdu(mpdu) = request.body else {
                    return Err(UpperMacTxError::InvalidFrames);
                };
                let header = headers.first().ok_or(UpperMacTxError::InvalidFrames)?;
                (header, Some(0), set_retry_bit, mpdu.response, key)
            }
            AttemptContent::Subframe { index } => (
                headers
                    .get(usize::from(index))
                    .ok_or(UpperMacTxError::InvalidFrames)?,
                Some(usize::from(index)),
                true,
                TxResponse::Ack,
                key,
            ),
            AttemptContent::BlockAckRequest {
                tid,
                starting_sequence,
            } => {
                let header = headers
                    .first()
                    .and_then(|header| header.get(4..16))
                    .ok_or(UpperMacTxError::InvalidFrames)?;
                let mut receiver = [0; 6];
                let mut transmitter = [0; 6];
                receiver.copy_from_slice(&header[..6]);
                transmitter.copy_from_slice(&header[6..]);
                request_frame =
                    encode_block_ack_request(receiver, transmitter, tid, starting_sequence);
                (
                    &request_frame[..],
                    None,
                    false,
                    TxResponse::BlockAck,
                    KeySelector::Plaintext,
                )
            }
            AttemptContent::Ampdu { .. } => return Err(UpperMacTxError::InvalidFrames),
        };
        let body_slot = match origin {
            Some(origin) => Some(
                bodies
                    .get_mut(origin)
                    .ok_or(UpperMacTxError::InvalidFrames)?,
            ),
            None => None,
        };
        let len = mpdu_len(header, body_slot.as_ref().and_then(|body| body.as_ref()));
        let mut buffer = self
            .port
            .tx_buffer(len)
            .map_err(UpperMacTxError::Port)?
            .ok_or(UpperMacTxError::NoBuffer)?;
        buffer.frame_mut()[..header.len()].copy_from_slice(header);
        if set_retry && !set_retry_bit(buffer.frame_mut()) {
            self.port.release_tx_buffer(buffer);
            return Err(UpperMacTxError::InvalidFrames);
        }
        let Ok(registration) = self.router.register(self.router.next_id()) else {
            self.port.release_tx_buffer(buffer);
            return Err(UpperMacTxError::RouterFull);
        };
        let id = registration.id();
        let body_slot = body_slot.filter(|body| body.is_some());
        let lent = body_slot.is_some().then_some(origin).flatten();
        let attempt = self.attempt(
            id,
            plan,
            TxPayload {
                frame: buffer,
                body: body_slot.and_then(Option::take),
                response,
            },
            key,
        );
        match self.port.submit(attempt).map_err(UpperMacTxError::Port)? {
            Ok(()) => Ok((registration, lent)),
            Err(Refused { error, attempt }) => {
                // The body comes back with the refused attempt.
                if let (Some(origin), Some(body)) = (lent, attempt.payload.body) {
                    bodies[origin] = Some(body);
                }
                self.port.release_tx_buffer(attempt.payload.frame);
                Err(UpperMacTxError::Refused(error))
            }
        }
    }

    fn attempt<T>(
        &self,
        id: TxId,
        plan: &TxAttemptPlan,
        payload: T,
        key: KeySelector,
    ) -> TxAttempt<T> {
        TxAttempt {
            id,
            vif: self.vif,
            access_category: plan.access_category,
            payload,
            rate: plan.rate,
            protection: plan.port_protection(),
            key,
            power: plan.power,
            backoff: plan.backoff,
            coex: plan.coex,
        }
    }

    /// Await the completion of a registered attempt from the router and take
    /// back the bodies it carried, the attempt's subframe `n` into the
    /// caller's `origin(n)`. After a loss, cancel the attempt: an admitted
    /// cancel produces its completion; a refusal proves it ended, and the
    /// router resolves whether its completion is still queued or was lost.
    async fn completion(
        &self,
        registration: &Registration<'r, 'p, P, WAITERS, RX>,
        bodies: &mut [Option<P::TxBody>],
        origin: impl Fn(usize) -> Option<usize>,
    ) -> Result<TxCompletion, UpperMacTxError<P::Error>> {
        let id = registration.id();
        let completion = loop {
            match self.router.completion(id).await {
                Awaited::Completed(completion) => break Ok(completion),
                Awaited::Poisoned => return Err(UpperMacTxError::Poisoned),
                Awaited::Lost => match self.port.cancel(id).map_err(UpperMacTxError::Port)? {
                    Ok(()) => {}
                    Err(CancelError::NotRunning) => {
                        break self
                            .router
                            .resolve(id)
                            .await
                            .ok_or(UpperMacTxError::CompletionLost { attempt: id });
                    }
                },
            }
        };
        // The attempt ended either way: its bodies come back.
        let reclaimed = self
            .port
            .reclaim_tx_bodies(id, |subframe, body| {
                if let Some(slot) = origin(subframe).and_then(|origin| bodies.get_mut(origin)) {
                    *slot = Some(body);
                }
            })
            .map_err(UpperMacTxError::Port)?;
        match reclaimed {
            Ok(()) | Err(ReclaimError::Unknown) => completion,
            Err(ReclaimError::Running) => Err(UpperMacTxError::BodiesHeld { attempt: id }),
        }
    }
}

impl<'r, 'p, P, B, const WAITERS: usize, const RX: usize> UpperMacTx<'r, 'p, P, B, WAITERS, RX>
where
    P: LowerMacAirReservation,
    B: HeTxopRtsBudget,
{
    /// Reserve the interface's air for `duration` with a CTS-to-self, sent
    /// once after the access category's AIFS with no further backoff, and
    /// wait until it went out: its completion.
    pub async fn reserve_air(
        &self,
        duration: oer_time::Duration,
        access_category: WmmAccessCategory,
        rate: PhyRate,
        power: TxPower,
        coex: CoexPriority,
    ) -> Result<TxCompletion, UpperMacTxError<P::Error>> {
        let queue = self
            .router
            .queue(self.port.capabilities().tx_queue(access_category))
            .map_err(|_| UpperMacTxError::RouterFull)?;
        queue.ready().await.map_err(|_| UpperMacTxError::Poisoned)?;
        let Ok(registration) = self.router.register(self.router.next_id()) else {
            return Err(UpperMacTxError::RouterFull);
        };
        let attempt = TxAttempt {
            id: registration.id(),
            vif: self.vif,
            access_category,
            payload: AirReservation { duration },
            rate,
            protection: Protection::None,
            key: KeySelector::Plaintext,
            power,
            backoff: Backoff::Slots(0),
            coex,
        };
        if let Err(Refused { error, .. }) = self
            .port
            .submit_air_reservation(attempt)
            .map_err(UpperMacTxError::Port)?
        {
            return Err(UpperMacTxError::Refused(error));
        }
        self.completion(&registration, &mut [], |_| None).await
    }
}

impl<'r, 'p, P, B, const WAITERS: usize, const RX: usize> UpperMacTx<'r, 'p, P, B, WAITERS, RX>
where
    P: LowerMacAmpdu,
    B: HeTxopRtsBudget,
{
    /// Send one A-MPDU: `frames` holds the subframes of the request's
    /// [`TxBody::Ampdu`] in order; their bodies are back in `frames` when
    /// the exchange ends.
    pub async fn send_ampdu(
        &mut self,
        frames: AmpduFrames<'_, P::TxBody>,
        request: TxRequest,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<TxReport, UpperMacTxError<P::Error>> {
        let TxBody::Ampdu(ampdu) = request.body else {
            return Err(UpperMacTxError::InvalidFrames);
        };
        let AmpduFrames {
            headers,
            bodies,
            key,
            min_mpdu_start_spacing,
        } = frames;
        if headers.len() != usize::from(ampdu.subframes()) || bodies.len() != headers.len() {
            return Err(UpperMacTxError::InvalidFrames);
        }
        let queue = self
            .router
            .queue(self.port.capabilities().tx_queue(request.access_category))
            .map_err(|_| UpperMacTxError::RouterFull)?;
        queue.ready().await.map_err(|_| UpperMacTxError::Poisoned)?;
        let (mut exchange, mut plan) = self
            .planner
            .begin(request, ladder, entropy)
            .map_err(UpperMacTxError::Plan)?;
        loop {
            let completion = match plan.content {
                AttemptContent::Ampdu {
                    subframes: selected,
                    retry,
                } => {
                    let registration = self.submit_aggregate(
                        &plan,
                        headers,
                        bodies,
                        (key, min_mpdu_start_spacing),
                        selected,
                        retry,
                        ampdu.tid,
                    )?;
                    self.completion(&registration, bodies, |nth| nth_selected(selected, nth))
                        .await?
                }
                _ => {
                    let (registration, origin) =
                        self.submit_single(&exchange, &plan, headers, bodies, key)?;
                    self.completion(&registration, bodies, |_| origin).await?
                }
            };
            match self.step(&mut exchange, &completion, ladder, entropy)? {
                TxStep::Attempt(next) => plan = next,
                TxStep::Done(report) => return Ok(report),
            }
        }
    }

    /// Submit the aggregate of the `selected` subframes, the `retry` ones
    /// with the Retry bit, each with its body taken from `bodies`.
    #[allow(clippy::too_many_arguments)]
    fn submit_aggregate(
        &mut self,
        plan: &TxAttemptPlan,
        headers: &[&[u8]],
        bodies: &mut [Option<P::TxBody>],
        (key, min_mpdu_start_spacing): (KeySelector, u8),
        selected: u64,
        retry: u64,
        tid: u8,
    ) -> Result<Registration<'r, 'p, P, WAITERS, RX>, UpperMacTxError<P::Error>> {
        let mut buffer = self
            .port
            .ampdu_buffer()
            .map_err(UpperMacTxError::Port)?
            .ok_or(UpperMacTxError::NoBuffer)?;
        let mut remaining = selected;
        while remaining != 0 {
            let index = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            let (Some(header), Some(body)) = (headers.get(index), bodies.get_mut(index)) else {
                self.port.release_ampdu_buffer(buffer);
                return Err(UpperMacTxError::InvalidFrames);
            };
            let len = mpdu_len(header, body.as_ref());
            let bytes = match buffer.push_mpdu(len, body.take()) {
                Ok(bytes) => bytes,
                Err(refused) => {
                    *body = refused;
                    self.port.release_ampdu_buffer(buffer);
                    return Err(UpperMacTxError::NoBuffer);
                }
            };
            bytes.copy_from_slice(header);
            if retry & (1 << index) != 0 && !set_retry_bit(bytes) {
                self.port.release_ampdu_buffer(buffer);
                return Err(UpperMacTxError::InvalidFrames);
            }
        }
        let Ok(registration) = self.router.register(self.router.next_id()) else {
            self.port.release_ampdu_buffer(buffer);
            return Err(UpperMacTxError::RouterFull);
        };
        let id = registration.id();
        let attempt = self.attempt(
            id,
            plan,
            AmpduPayload {
                subframes: buffer,
                tid,
                min_mpdu_start_spacing,
            },
            key,
        );
        match self
            .port
            .submit_ampdu(attempt)
            .map_err(UpperMacTxError::Port)?
        {
            Ok(()) => Ok(registration),
            Err(Refused { error, attempt }) => {
                self.port.release_ampdu_buffer(attempt.payload.subframes);
                Err(UpperMacTxError::Refused(error))
            }
        }
    }
}
