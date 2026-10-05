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
pub mod queue;
pub mod reorder;
pub mod router;

pub use router::{Awaited, EventRouter, Registration, RouterFull};

use oer_ieee80211_lower_mac::{
    AmpduBuffer, AmpduPayload, CancelError, Ieee80211LowerMacPort, KeySelector, LowerMacAmpdu,
    Refused, SubmitError, TxAttempt, TxBuffer, TxCompletion, TxId, TxPayload, TxResponse, VifId,
};
use oer_ieee80211_mac::block_ack::encode_block_ack_request;
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
    /// The port cannot serve.
    Port(E),
}

/// The encoded subframes of one A-MPDU and how the port sends them.
#[derive(Clone, Copy, Debug)]
pub struct AmpduFrames<'f> {
    /// Subframe `i` of the request, from its header to the end of its body.
    pub subframes: &'f [&'f [u8]],
    pub key: KeySelector,
    /// The recipient's Minimum MPDU Start Spacing, IEEE encoding 0-7.
    pub min_mpdu_start_spacing: u8,
}

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

    /// Send one MPDU: `frame` from its header to the end of its body, the
    /// request's [`TxBody::Mpdu`] describing it, protected with `key`.
    pub async fn send_mpdu(
        &mut self,
        frame: &[u8],
        key: KeySelector,
        request: TxRequest,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<TxReport, UpperMacTxError<P::Error>> {
        if !matches!(request.body, TxBody::Mpdu(_)) {
            return Err(UpperMacTxError::InvalidFrames);
        }
        let (mut exchange, mut plan) = self
            .planner
            .begin(request, ladder, entropy)
            .map_err(UpperMacTxError::Plan)?;
        loop {
            let registration = self.submit_single(&exchange, &plan, &[frame], key)?;
            let completion = self.completion(&registration).await?;
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
    fn submit_single(
        &mut self,
        exchange: &TxExchange,
        plan: &TxAttemptPlan,
        frames: &[&[u8]],
        key: KeySelector,
    ) -> Result<Registration<'r, 'p, P, WAITERS, RX>, UpperMacTxError<P::Error>> {
        let request = exchange.request();
        let request_frame: [u8; oer_ieee80211_mac::block_ack::BLOCK_ACK_REQUEST_LEN];
        let (bytes, set_retry, response, key): (&[u8], bool, TxResponse, KeySelector) =
            match plan.content {
                AttemptContent::Mpdu { set_retry_bit } => {
                    let TxBody::Mpdu(mpdu) = request.body else {
                        return Err(UpperMacTxError::InvalidFrames);
                    };
                    (frames[0], set_retry_bit, mpdu.response, key)
                }
                AttemptContent::Subframe { index } => (
                    frames
                        .get(usize::from(index))
                        .copied()
                        .ok_or(UpperMacTxError::InvalidFrames)?,
                    true,
                    TxResponse::Ack,
                    key,
                ),
                AttemptContent::BlockAckRequest {
                    tid,
                    starting_sequence,
                } => {
                    let header = frames
                        .first()
                        .and_then(|frame| frame.get(4..16))
                        .ok_or(UpperMacTxError::InvalidFrames)?;
                    let mut receiver = [0; 6];
                    let mut transmitter = [0; 6];
                    receiver.copy_from_slice(&header[..6]);
                    transmitter.copy_from_slice(&header[6..]);
                    request_frame =
                        encode_block_ack_request(receiver, transmitter, tid, starting_sequence);
                    (
                        &request_frame[..],
                        false,
                        TxResponse::BlockAck,
                        KeySelector::Plaintext,
                    )
                }
                AttemptContent::Ampdu { .. } => return Err(UpperMacTxError::InvalidFrames),
            };
        let mut buffer = self
            .port
            .tx_buffer(bytes.len())
            .map_err(UpperMacTxError::Port)?
            .ok_or(UpperMacTxError::NoBuffer)?;
        buffer.frame_mut().copy_from_slice(bytes);
        if set_retry && !set_retry_bit(buffer.frame_mut()) {
            self.port.release_tx_buffer(buffer);
            return Err(UpperMacTxError::InvalidFrames);
        }
        let Ok(registration) = self.router.register(self.router.next_id()) else {
            self.port.release_tx_buffer(buffer);
            return Err(UpperMacTxError::RouterFull);
        };
        let id = registration.id();
        let attempt = self.attempt(
            id,
            plan,
            TxPayload {
                frame: buffer,
                response,
            },
            key,
        );
        match self.port.submit(attempt).map_err(UpperMacTxError::Port)? {
            Ok(()) => Ok(registration),
            Err(Refused { error, attempt }) => {
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

    /// Await the completion of a registered attempt from the router. After
    /// a loss, cancel the attempt: an admitted cancel produces its
    /// completion; a refusal proves it ended, and the router resolves
    /// whether its completion is still queued or was lost.
    async fn completion(
        &self,
        registration: &Registration<'r, 'p, P, WAITERS, RX>,
    ) -> Result<TxCompletion, UpperMacTxError<P::Error>> {
        let id = registration.id();
        loop {
            match self.router.completion(id).await {
                Awaited::Completed(completion) => return Ok(completion),
                Awaited::Poisoned => return Err(UpperMacTxError::Poisoned),
                Awaited::Lost => match self.port.cancel(id).map_err(UpperMacTxError::Port)? {
                    Ok(()) => {}
                    Err(CancelError::NotRunning) => {
                        return self
                            .router
                            .resolve(id)
                            .await
                            .ok_or(UpperMacTxError::CompletionLost { attempt: id });
                    }
                },
            }
        }
    }
}

impl<'r, 'p, P, B, const WAITERS: usize, const RX: usize> UpperMacTx<'r, 'p, P, B, WAITERS, RX>
where
    P: LowerMacAmpdu,
    B: HeTxopRtsBudget,
{
    /// Send one A-MPDU: `frames` holds the subframes of the request's
    /// [`TxBody::Ampdu`] in order.
    pub async fn send_ampdu(
        &mut self,
        frames: AmpduFrames<'_>,
        request: TxRequest,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<TxReport, UpperMacTxError<P::Error>> {
        let TxBody::Ampdu(ampdu) = request.body else {
            return Err(UpperMacTxError::InvalidFrames);
        };
        if frames.subframes.len() != usize::from(ampdu.subframes()) {
            return Err(UpperMacTxError::InvalidFrames);
        }
        let (mut exchange, mut plan) = self
            .planner
            .begin(request, ladder, entropy)
            .map_err(UpperMacTxError::Plan)?;
        loop {
            let registration = match plan.content {
                AttemptContent::Ampdu {
                    subframes: selected,
                    retry,
                } => self.submit_aggregate(&plan, &frames, selected, retry, ampdu.tid)?,
                _ => self.submit_single(&exchange, &plan, frames.subframes, frames.key)?,
            };
            let completion = self.completion(&registration).await?;
            match self.step(&mut exchange, &completion, ladder, entropy)? {
                TxStep::Attempt(next) => plan = next,
                TxStep::Done(report) => return Ok(report),
            }
        }
    }

    fn submit_aggregate(
        &mut self,
        plan: &TxAttemptPlan,
        frames: &AmpduFrames<'_>,
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
            let Some(frame) = frames.subframes.get(index) else {
                self.port.release_ampdu_buffer(buffer);
                return Err(UpperMacTxError::InvalidFrames);
            };
            let Some(bytes) = buffer.push_mpdu(frame.len()) else {
                self.port.release_ampdu_buffer(buffer);
                return Err(UpperMacTxError::NoBuffer);
            };
            bytes.copy_from_slice(frame);
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
                min_mpdu_start_spacing: frames.min_mpdu_start_spacing,
            },
            frames.key,
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
