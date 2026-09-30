#![no_std]
#![forbid(unsafe_code)]

//! The driver of the IEEE 802.11 transmit planner over a lower-MAC port.
//!
//! [`UpperMacTx`] runs one frame exchange of `oer-ieee80211-upper-mac`'s
//! [`TxPlanner`] over any [`Ieee80211LowerMacPort`]: it encodes each planned
//! attempt into a port buffer, submits it, awaits its completion among the
//! port's events, feeds the completion to the planner and repeats until the
//! planner reports the exchange's end. One attempt is one submission.
//! Aggregates need the [`LowerMacAmpdu`] extension, which
//! [`UpperMacTx::send_ampdu`] requires as a bound.
//!
//! The caller keeps its encoded MPDUs, header to end of body without the
//! FCS, and the driver copies them into a fresh buffer for every attempt,
//! setting the Retry bit where the plan says so; a retransmission therefore
//! repeats the first encoding's sequence number and CCMP packet number.
//! Events that are not the exchange's completion go to the caller's
//! handler. The driver waits only on the port; it never reads another clock
//! (the planner ages aggregates on the port's radio clock) and runs under any
//! executor.

use oer_ieee80211_lower_mac::{
    AmpduBuffer, AmpduPayload, EventsLost, Ieee80211LowerMacPort, KeySelector, LowerMacAmpdu,
    LowerMacEvent, Refused, SubmitError, TxAttempt, TxBuffer, TxCompletion, TxId, TxPayload,
    TxResponse, VifId,
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
    /// The port lost events, the attempt's completion possibly among them;
    /// the attempt may still be in flight.
    EventsLost { attempt: TxId },
    /// The port is poisoned.
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

/// The transmit driver of one interface over a lower-MAC port.
pub struct UpperMacTx<'p, P, B> {
    port: &'p P,
    vif: VifId,
    planner: TxPlanner<B>,
    next_id: u32,
}

impl<'p, P, B> UpperMacTx<'p, P, B>
where
    P: Ieee80211LowerMacPort,
    B: HeTxopRtsBudget,
{
    /// A driver of `vif`'s transmissions through `port`, whose attempt
    /// identities start at `first_id`.
    pub const fn new(port: &'p P, vif: VifId, planner: TxPlanner<B>, first_id: u32) -> Self {
        Self {
            port,
            vif,
            planner,
            next_id: first_id,
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
        mut other_event: impl FnMut(&P::Event),
    ) -> Result<TxReport, UpperMacTxError<P::Error>> {
        if !matches!(request.body, TxBody::Mpdu(_)) {
            return Err(UpperMacTxError::InvalidFrames);
        }
        let (mut exchange, mut plan) = self
            .planner
            .begin(request, ladder, entropy)
            .map_err(UpperMacTxError::Plan)?;
        loop {
            let id = self.submit_single(&exchange, &plan, &[frame], key)?;
            let completion = self.completion(id, &mut other_event).await?;
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

    fn take_id(&mut self) -> TxId {
        let id = TxId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Submit an attempt that carries one MPDU: the request's MPDU, one
    /// subframe of it, or a BlockAckReq for it.
    fn submit_single(
        &mut self,
        exchange: &TxExchange,
        plan: &TxAttemptPlan,
        frames: &[&[u8]],
        key: KeySelector,
    ) -> Result<TxId, UpperMacTxError<P::Error>> {
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
            .ok_or(UpperMacTxError::NoBuffer)?;
        buffer.frame_mut().copy_from_slice(bytes);
        if set_retry && !set_retry_bit(buffer.frame_mut()) {
            self.port.release_tx_buffer(buffer);
            return Err(UpperMacTxError::InvalidFrames);
        }
        let id = self.take_id();
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
            Ok(()) => Ok(id),
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

    /// Await the completion of attempt `id`, handing every other event to
    /// the caller.
    async fn completion(
        &self,
        id: TxId,
        other_event: &mut impl FnMut(&P::Event),
    ) -> Result<TxCompletion, UpperMacTxError<P::Error>> {
        loop {
            let event = self
                .port
                .next_event()
                .await
                .map_err(|EventsLost| UpperMacTxError::EventsLost { attempt: id })?;
            match P::view(&event) {
                LowerMacEvent::TxCompleted(completion) if completion.id == id => {
                    return Ok(completion);
                }
                _ => other_event(&event),
            }
        }
    }
}

impl<'p, P, B> UpperMacTx<'p, P, B>
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
        mut other_event: impl FnMut(&P::Event),
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
            let id = match plan.content {
                AttemptContent::Ampdu {
                    subframes: selected,
                    retry,
                } => self.submit_aggregate(&plan, &frames, selected, retry, ampdu.tid)?,
                _ => self.submit_single(&exchange, &plan, frames.subframes, frames.key)?,
            };
            let completion = self.completion(id, &mut other_event).await?;
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
    ) -> Result<TxId, UpperMacTxError<P::Error>> {
        let mut buffer = self.port.ampdu_buffer().ok_or(UpperMacTxError::NoBuffer)?;
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
        let id = self.take_id();
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
            Ok(()) => Ok(id),
            Err(Refused { error, attempt }) => {
                self.port.release_ampdu_buffer(attempt.payload.subframes);
                Err(UpperMacTxError::Refused(error))
            }
        }
    }
}
