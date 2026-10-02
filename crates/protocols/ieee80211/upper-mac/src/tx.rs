//! The transmit planner: one frame exchange as a sequence of lower-MAC
//! attempts.
//!
//! A lower-MAC attempt is one hardware transmission. [`TxPlanner`] turns an
//! outbound MPDU or A-MPDU into those attempts. [`TxPlanner::begin`] returns
//! the exchange's state ([`TxExchange`]) and its first [`TxAttemptPlan`]: the
//! rate the peer's [`RateLadder`] gives, the protection exchange the
//! [`ProtectionPolicy`] selects, the backoff the access category's
//! [`EdcaContention`] draws from the caller's entropy, and which frame
//! content with which Retry bits the attempt carries. [`TxPlanner::complete`]
//! takes that attempt's [`TxCompletion`] and returns either the next plan or
//! the exchange's [`TxReport`]:
//!
//! - an MPDU follows [`MpduRetryState`]: retried at the ladder's next rate
//!   until delivered or a retry limit is reached;
//! - an A-MPDU follows [`AmpduRetryState`]: retransmitted as an aggregate of
//!   the unacknowledged subframes, republished unchanged after a failed
//!   protection exchange, asked for its BlockAck with a BlockAckReq, and
//!   finished or handed over to individual transmission of its missing
//!   subframes, each again an MPDU exchange.
//!
//! The planner never holds frame bytes and never waits: the caller encodes
//! each attempt from its own copy of the frames, submits it and feeds the
//! completion back. A retransmission carries the bytes first encoded, so
//! its sequence number and CCMP packet number repeat.

use oer_ieee80211_lower_mac::{
    Backoff, BlockAckReport, CoexPriority, TxCompletion, TxPower, TxResponse, TxStatus,
};
use oer_ieee80211_mac::{phy::PhyRate, qos::WmmAccessCategory, sequence::SequenceNumber};
use oer_ieee80211_softmac::{
    BackoffEntropy, EdcaContention, MacAmpduTxResult, MacAmpduTxStatus, MacIndividualRetries,
    MacTxResult, MacTxStatus,
};
use oer_time::RadioInstant;

use crate::{
    ampdu::{
        AmpduAttemptResult, AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
        MAX_AMPDU_SUBFRAMES,
    },
    protection::{
        HeTxopRtsBudget, ProtectedPpdu, ProtectionPolicy, TxProtection, TxProtectionDecision,
        TxPsdu, TxReceiver,
    },
    retry::{
        ContentionUpdate, FrameClass, MpduRetryState, RateLadder, RetryDecision, RetryLimits,
        RetryOutcome, RetryStateError,
    },
};

/// Octets an A-MPDU subframe adds before its MPDU: the MPDU delimiter.
const AMPDU_DELIMITER: u32 = 4;

/// One outbound MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduRequest {
    /// Octets on air, MAC header to FCS, MIC included.
    pub length: u32,
    /// The response the MPDU solicits: `Ack` for an individually addressed
    /// frame, `None` for a group-addressed one.
    pub response: TxResponse,
}

/// One outbound A-MPDU of consecutive sequence numbers under one TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRequest {
    pub tid: u8,
    /// The sequence number of subframe zero; subframe `i` carries
    /// `first_sequence + i`.
    pub first_sequence: SequenceNumber,
    subframes: u8,
    lengths: [u16; MAX_AMPDU_SUBFRAMES as usize],
    /// When the MSDUs were committed: the aggregate's lifetime runs from
    /// here, on the backend's radio clock.
    pub committed_at: RadioInstant,
    /// Whether the Block Ack agreement is operational; while it is not,
    /// missing subframes leave the aggregate.
    pub block_ack_operational: bool,
}

impl AmpduRequest {
    /// An aggregate of subframes whose MPDUs are `lengths` octets on air
    /// (MAC header to FCS, MIC included); `None` for no subframe or more
    /// than [`MAX_AMPDU_SUBFRAMES`].
    pub fn new(
        tid: u8,
        first_sequence: SequenceNumber,
        lengths: &[u16],
        committed_at: RadioInstant,
        block_ack_operational: bool,
    ) -> Option<Self> {
        if lengths.is_empty() || lengths.len() > usize::from(MAX_AMPDU_SUBFRAMES) {
            return None;
        }
        let mut owned = [0; MAX_AMPDU_SUBFRAMES as usize];
        owned[..lengths.len()].copy_from_slice(lengths);
        Some(Self {
            tid,
            first_sequence,
            subframes: lengths.len() as u8,
            lengths: owned,
            committed_at,
            block_ack_operational,
        })
    }

    pub const fn subframes(&self) -> u8 {
        self.subframes
    }

    /// The sequence number of subframe `index`.
    pub const fn sequence(&self, index: u8) -> SequenceNumber {
        self.first_sequence.wrapping_add(index as u16)
    }

    /// Octets on air of subframe `index`'s MPDU.
    pub fn mpdu_length(&self, index: u8) -> u32 {
        u32::from(self.lengths[usize::from(index)])
    }

    /// The A-MPDU of the subframes of `mask`, in order: each MPDU after its
    /// delimiter, padded to four octets except the last.
    pub fn aggregate_length(&self, mask: u64) -> u32 {
        let mut length = 0_u32;
        let mut remaining = mask & crate::ampdu::all_subframes(self.subframes);
        while remaining != 0 {
            let index = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            let subframe = AMPDU_DELIMITER + self.mpdu_length(index);
            length += if remaining == 0 {
                subframe
            } else {
                subframe.next_multiple_of(4)
            };
        }
        length
    }
}

/// What an exchange sends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBody {
    Mpdu(MpduRequest),
    Ampdu(AmpduRequest),
}

/// One outbound frame exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxRequest {
    pub access_category: WmmAccessCategory,
    /// The rate the peer's rate control chose for the first attempt.
    pub initial_rate: PhyRate,
    pub receiver: TxReceiver,
    pub power: TxPower,
    pub coex: CoexPriority,
    /// Unacknowledged transmissions one MPDU of the exchange may make.
    pub mpdu_retry_limit: u8,
    pub body: TxBody,
}

/// The frame content of one attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptContent {
    /// The request's MPDU; with its Retry bit set once it was on air.
    Mpdu { set_retry_bit: bool },
    /// An A-MPDU of the request's subframes in `subframes` (bit `i` is
    /// subframe `i`), in order; those in `retry` carry the Retry bit.
    Ampdu { subframes: u64, retry: u64 },
    /// Subframe `index` of the request alone, answered by an ACK, with its
    /// Retry bit set: it was on air in an aggregate.
    Subframe { index: u8 },
    /// A BlockAckReq for the aggregate's TID, starting at
    /// `starting_sequence`, answered by a BlockAck.
    BlockAckRequest {
        tid: u8,
        starting_sequence: SequenceNumber,
    },
}

/// Everything one lower-MAC attempt needs besides its frame bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAttemptPlan {
    pub access_category: WmmAccessCategory,
    pub rate: PhyRate,
    pub protection: TxProtectionDecision,
    pub backoff: Backoff,
    pub power: TxPower,
    pub coex: CoexPriority,
    pub content: AttemptContent,
}

/// The statistics of a finished exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxReport {
    Mpdu(MacTxStatus),
    Ampdu(MacAmpduTxStatus),
}

/// The next step of an exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxStep {
    /// Submit this attempt and feed its completion back.
    Attempt(TxAttemptPlan),
    /// The exchange ended.
    Done(TxReport),
}

/// Why an exchange could not begin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxPlanError {
    Retry(RetryStateError),
    Ampdu(AmpduRetryError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Mpdu(MpduRetryState),
    Aggregate(AmpduRetryState),
    BlockAckRequest(AmpduRetryState),
    Individual {
        aggregate: AmpduRetryState,
        /// Original subframes still to send, the current one included.
        remaining: u64,
        retry: MpduRetryState,
    },
    Done,
}

/// The state of one frame exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxExchange {
    request: TxRequest,
    phase: Phase,
    /// Original subframes (bit zero for an MPDU) that were on air.
    transmitted: u64,
    /// The rate of the last attempt.
    rate: PhyRate,
    /// The aggregate of the last A-MPDU attempt.
    aggregate: u64,
    aggregate_attempts: u8,
    individual: MacIndividualRetries,
    ack_snr_db: Option<i8>,
    /// Whether the last A-MPDU attempt ended without any BlockAck.
    unanswered: bool,
    report: Option<TxReport>,
}

impl TxExchange {
    pub const fn request(&self) -> &TxRequest {
        &self.request
    }

    pub const fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// The report of a finished exchange.
    pub const fn report(&self) -> Option<TxReport> {
        self.report
    }

    /// Mark whether the Block Ack agreement is operational; the next
    /// completion of the aggregate reads it.
    pub fn set_block_ack_operational(&mut self, operational: bool) {
        if let TxBody::Ampdu(request) = &mut self.request.body {
            request.block_ack_operational = operational;
        }
    }
}

/// The transmit planner of one interface: the EDCA state of the four access
/// categories and the retry and protection policies every exchange follows.
pub struct TxPlanner<B = crate::protection::ProtectEveryHeTxop> {
    contention: [EdcaContention; 4],
    retry_limits: RetryLimits,
    ampdu_policy: AmpduRetryPolicy,
    protection: ProtectionPolicy,
    he_budget: B,
}

impl<B: HeTxopRtsBudget> TxPlanner<B> {
    /// A planner whose access categories contend with `contention`, indexed
    /// by ACI.
    pub const fn new(
        contention: [EdcaContention; 4],
        retry_limits: RetryLimits,
        ampdu_policy: AmpduRetryPolicy,
        protection: ProtectionPolicy,
        he_budget: B,
    ) -> Self {
        Self {
            contention,
            retry_limits,
            ampdu_policy,
            protection,
            he_budget,
        }
    }

    /// The contention state of an access category.
    pub const fn contention(&self, access_category: WmmAccessCategory) -> EdcaContention {
        self.contention[access_category as usize]
    }

    /// Replace an access category's contention parameters, as a new EDCA
    /// Parameter Set advertises them.
    pub fn set_contention(
        &mut self,
        access_category: WmmAccessCategory,
        contention: EdcaContention,
    ) {
        self.contention[access_category as usize] = contention;
    }

    pub fn protection(&self) -> &ProtectionPolicy {
        &self.protection
    }

    pub fn protection_mut(&mut self) -> &mut ProtectionPolicy {
        &mut self.protection
    }

    pub const fn ampdu_policy(&self) -> AmpduRetryPolicy {
        self.ampdu_policy
    }

    pub fn set_ampdu_policy(&mut self, policy: AmpduRetryPolicy) {
        self.ampdu_policy = policy;
    }

    /// Start an exchange and plan its first attempt.
    pub fn begin(
        &mut self,
        request: TxRequest,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> Result<(TxExchange, TxAttemptPlan), TxPlanError> {
        let phase = match request.body {
            TxBody::Mpdu(mpdu) => Phase::Mpdu(self.mpdu_retry(&request, mpdu.length)?),
            TxBody::Ampdu(ampdu) => Phase::Aggregate(
                AmpduRetryState::new(
                    ampdu.first_sequence,
                    ampdu.subframes,
                    self.ampdu_policy,
                    ampdu.committed_at.as_micros(),
                )
                .map_err(TxPlanError::Ampdu)?,
            ),
        };
        let mut exchange = TxExchange {
            request,
            phase,
            transmitted: 0,
            rate: request.initial_rate,
            aggregate: 0,
            aggregate_attempts: 0,
            individual: MacIndividualRetries::NONE,
            ack_snr_db: None,
            unanswered: false,
            report: None,
        };
        let plan = self.plan(&mut exchange, ladder, entropy);
        Ok((exchange, plan))
    }

    /// Apply the completion of the exchange's last attempt, observed at
    /// `now` on the backend's radio clock, and plan the next step.
    ///
    /// A completion after the exchange ended returns its report again.
    pub fn complete(
        &mut self,
        exchange: &mut TxExchange,
        completion: &TxCompletion,
        now: RadioInstant,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> TxStep {
        if completion.ack_snr_db.is_some() {
            exchange.ack_snr_db = completion.ack_snr_db;
        }
        let now = now.as_micros();
        let next = match exchange.phase {
            Phase::Mpdu(mut retry) => {
                let step = retry.observe(completion.status);
                self.update_contention(exchange, step.contention);
                match step.decision {
                    RetryDecision::Retry { set_retry_bit } => {
                        if set_retry_bit {
                            exchange.transmitted = 1;
                        }
                        Phase::Mpdu(retry)
                    }
                    RetryDecision::Complete(outcome) => {
                        let report = TxReport::Mpdu(Self::mpdu_status(exchange, &retry, outcome));
                        exchange.phase = Phase::Done;
                        exchange.report = Some(report);
                        return TxStep::Done(report);
                    }
                }
            }
            Phase::Aggregate(mut state) => {
                let result = match (completion.block_ack, completion.status) {
                    (Some(report), _) => AmpduAttemptResult::Answered(Some(report)),
                    (None, TxStatus::Success) => AmpduAttemptResult::Answered(None),
                    (None, TxStatus::CtsTimeout) => AmpduAttemptResult::ProtectionFailure,
                    (None, TxStatus::AckTimeout | TxStatus::Collision) => {
                        AmpduAttemptResult::NoResponse
                    }
                    (None, TxStatus::Aborted | TxStatus::Fault(_)) => {
                        self.update_contention(exchange, ContentionUpdate::Reset);
                        return self.finish_ampdu(exchange, &state, MacAmpduTxResult::Incomplete);
                    }
                };
                if !matches!(result, AmpduAttemptResult::ProtectionFailure) {
                    exchange.transmitted |= exchange.aggregate;
                }
                exchange.unanswered = matches!(result, AmpduAttemptResult::NoResponse);
                let current = state.current_subframes();
                let operational = self.block_ack_operational(exchange);
                match state.observe(result, current, now, operational) {
                    Ok(decision) => match self.after_aggregate(exchange, state, decision) {
                        Some(phase) => phase,
                        None => return self.finished(exchange, &state),
                    },
                    Err(_) => {
                        self.update_contention(exchange, ContentionUpdate::Reset);
                        return self.finish_ampdu(exchange, &state, MacAmpduTxResult::Incomplete);
                    }
                }
            }
            Phase::BlockAckRequest(mut state) => {
                let report = match completion.status {
                    TxStatus::Success => completion.block_ack,
                    _ => None,
                };
                exchange.unanswered = false;
                let operational = self.block_ack_operational(exchange);
                let decision = state.observe_block_ack_request(report, now, operational);
                match self.after_aggregate(exchange, state, decision) {
                    Some(phase) => phase,
                    None => return self.finished(exchange, &state),
                }
            }
            Phase::Individual {
                aggregate,
                mut remaining,
                mut retry,
            } => {
                let step = retry.observe(completion.status);
                self.update_contention(exchange, step.contention);
                match step.decision {
                    RetryDecision::Retry { .. } => Phase::Individual {
                        aggregate,
                        remaining,
                        retry,
                    },
                    RetryDecision::Complete(outcome) => {
                        let status = Self::mpdu_status(exchange, &retry, outcome);
                        exchange.individual.record(status);
                        remaining &= remaining - 1;
                        if remaining == 0 {
                            let result = if exchange.individual.failed == 0 {
                                MacAmpduTxResult::Delivered
                            } else {
                                MacAmpduTxResult::Incomplete
                            };
                            return self.finish_ampdu(exchange, &aggregate, result);
                        }
                        let index = remaining.trailing_zeros() as u8;
                        match self.subframe_retry(exchange, index) {
                            Ok(retry) => Phase::Individual {
                                aggregate,
                                remaining,
                                retry,
                            },
                            Err(_) => {
                                return self.finish_ampdu(
                                    exchange,
                                    &aggregate,
                                    MacAmpduTxResult::Incomplete,
                                );
                            }
                        }
                    }
                }
            }
            Phase::Done => {
                return TxStep::Done(exchange.report.expect("a finished exchange has its report"));
            }
        };
        exchange.phase = next;
        TxStep::Attempt(self.plan(exchange, ladder, entropy))
    }

    /// The phase after an aggregate decision; `None` when the exchange ends.
    fn after_aggregate(
        &mut self,
        exchange: &mut TxExchange,
        state: AmpduRetryState,
        decision: AmpduRetryDecision,
    ) -> Option<Phase> {
        match decision {
            AmpduRetryDecision::RetainAggregate { .. }
            | AmpduRetryDecision::RepublishUnchanged { .. } => {
                self.update_contention(exchange, ContentionUpdate::Double);
                Some(Phase::Aggregate(state))
            }
            AmpduRetryDecision::RequestBlockAck { .. } => {
                // The exhausted protection retries end this frame exchange
                // sequence; the BlockAckReq starts the next one.
                self.update_contention(exchange, ContentionUpdate::Reset);
                Some(Phase::BlockAckRequest(state))
            }
            AmpduRetryDecision::Unaggregate { retry_mask } => {
                let mut remaining = 0_u64;
                for position in 0..state.current_subframes() {
                    if retry_mask & (1 << position) != 0 {
                        remaining |= 1 << state.original_index(position);
                    }
                }
                self.update_contention(exchange, ContentionUpdate::Double);
                let index = remaining.trailing_zeros() as u8;
                match self.subframe_retry(exchange, index) {
                    Ok(retry) => Some(Phase::Individual {
                        aggregate: state,
                        remaining,
                        retry,
                    }),
                    Err(_) => None,
                }
            }
            AmpduRetryDecision::Finish { .. } | AmpduRetryDecision::FinishTriggerFlow => {
                self.update_contention(exchange, ContentionUpdate::Reset);
                None
            }
        }
    }

    fn finished(&self, exchange: &mut TxExchange, state: &AmpduRetryState) -> TxStep {
        let result = if state.acknowledged() == self.original_subframes(exchange) {
            MacAmpduTxResult::Delivered
        } else if exchange.unanswered {
            MacAmpduTxResult::HardwareTimeout
        } else {
            MacAmpduTxResult::Incomplete
        };
        self.finish_ampdu(exchange, state, result)
    }

    fn finish_ampdu(
        &self,
        exchange: &mut TxExchange,
        state: &AmpduRetryState,
        result: MacAmpduTxResult,
    ) -> TxStep {
        let report = TxReport::Ampdu(MacAmpduTxStatus {
            result,
            original_subframes: u16::from(self.original_subframes(exchange)),
            aggregate_attempts: exchange.aggregate_attempts,
            aggregate_rate: exchange.request.initial_rate,
            block_acknowledged_subframes: u16::from(state.acknowledged()),
            individual_retries: exchange.individual,
        });
        exchange.phase = Phase::Done;
        exchange.report = Some(report);
        TxStep::Done(report)
    }

    fn original_subframes(&self, exchange: &TxExchange) -> u8 {
        match exchange.request.body {
            TxBody::Mpdu(_) => 1,
            TxBody::Ampdu(ampdu) => ampdu.subframes,
        }
    }

    fn block_ack_operational(&self, exchange: &TxExchange) -> bool {
        match exchange.request.body {
            TxBody::Ampdu(ampdu) => ampdu.block_ack_operational,
            TxBody::Mpdu(_) => false,
        }
    }

    fn mpdu_retry(&self, request: &TxRequest, length: u32) -> Result<MpduRetryState, TxPlanError> {
        let frame_class = match self.protection.rts_length_threshold() {
            Some(threshold) if length > u32::from(threshold.bytes()) => FrameClass::Long,
            _ => FrameClass::Short,
        };
        MpduRetryState::new(self.retry_limits, request.mpdu_retry_limit, frame_class)
            .map_err(TxPlanError::Retry)
    }

    fn subframe_retry(
        &self,
        exchange: &TxExchange,
        index: u8,
    ) -> Result<MpduRetryState, TxPlanError> {
        let length = match exchange.request.body {
            TxBody::Ampdu(ampdu) => ampdu.mpdu_length(index),
            TxBody::Mpdu(mpdu) => mpdu.length,
        };
        self.mpdu_retry(&exchange.request, length)
    }

    fn update_contention(&mut self, exchange: &TxExchange, update: ContentionUpdate) {
        let contention = &mut self.contention[exchange.request.access_category as usize];
        match update {
            ContentionUpdate::Reset => contention.reset(),
            ContentionUpdate::Double => contention.record_failure(),
        }
    }

    fn mpdu_status(
        exchange: &TxExchange,
        retry: &MpduRetryState,
        outcome: RetryOutcome,
    ) -> MacTxStatus {
        let solicits_ack = match exchange.request.body {
            TxBody::Mpdu(mpdu) => mpdu.response != TxResponse::None,
            TxBody::Ampdu(_) => true,
        };
        let (result, delivered) = match outcome {
            RetryOutcome::Delivered => (MacTxResult::Transmitted, true),
            RetryOutcome::RetryLimit(TxStatus::Collision) => (MacTxResult::CollisionLimit, false),
            RetryOutcome::RetryLimit(_) => (MacTxResult::HardwareTimeout, false),
            RetryOutcome::Terminal(status) => (MacTxResult::HardwareFailure(status), false),
        };
        MacTxStatus {
            result,
            attempts: retry.attempts(),
            final_rate: exchange.rate,
            acknowledged: solicits_ack.then_some(delivered),
            ack_snr_db: if delivered { exchange.ack_snr_db } else { None },
            airtime_micros: None,
        }
    }

    /// Plan the attempt of the exchange's current phase.
    fn plan(
        &mut self,
        exchange: &mut TxExchange,
        ladder: &impl RateLadder,
        entropy: &mut impl BackoffEntropy,
    ) -> TxAttemptPlan {
        let request = exchange.request;
        let (rate, psdu, content) = match (exchange.phase, request.body) {
            (Phase::Mpdu(retry), TxBody::Mpdu(mpdu)) => (
                retry.rate(ladder, request.initial_rate, exchange.rate),
                TxPsdu::Mpdu {
                    length: mpdu.length,
                },
                AttemptContent::Mpdu {
                    set_retry_bit: exchange.transmitted != 0,
                },
            ),
            (Phase::Aggregate(state), TxBody::Ampdu(ampdu)) => {
                let subframes = state.pending_original_indices();
                exchange.aggregate = subframes;
                exchange.aggregate_attempts = exchange.aggregate_attempts.saturating_add(1);
                // A retained aggregate keeps its rate.
                (
                    request.initial_rate,
                    TxPsdu::Ampdu {
                        length: ampdu.aggregate_length(subframes),
                    },
                    AttemptContent::Ampdu {
                        subframes,
                        retry: exchange.transmitted & subframes,
                    },
                )
            }
            (Phase::BlockAckRequest(state), TxBody::Ampdu(ampdu)) => {
                exchange.aggregate = 0;
                let rate = PhyRate::Legacy(self.protection.control_rate(request.initial_rate));
                let backoff = self.contention[request.access_category as usize].draw(entropy);
                exchange.rate = rate;
                return TxAttemptPlan {
                    access_category: request.access_category,
                    rate,
                    protection: TxProtectionDecision::UNPROTECTED,
                    backoff,
                    power: request.power,
                    coex: request.coex,
                    content: AttemptContent::BlockAckRequest {
                        tid: ampdu.tid,
                        starting_sequence: state.current_first_sequence(),
                    },
                };
            }
            (
                Phase::Individual {
                    remaining, retry, ..
                },
                TxBody::Ampdu(ampdu),
            ) => {
                let index = remaining.trailing_zeros() as u8;
                let previous = if retry.attempts() == 1 {
                    request.initial_rate
                } else {
                    exchange.rate
                };
                (
                    retry.rate(ladder, request.initial_rate, previous),
                    TxPsdu::Mpdu {
                        length: ampdu.mpdu_length(index),
                    },
                    AttemptContent::Subframe { index },
                )
            }
            _ => unreachable!("the phase follows the request's body"),
        };
        exchange.rate = rate;
        let protection = self.protection.select(
            ProtectedPpdu {
                rate,
                receiver: request.receiver,
                psdu,
            },
            &self.he_budget,
        );
        let backoff = self.contention[request.access_category as usize].draw(entropy);
        TxAttemptPlan {
            access_category: request.access_category,
            rate,
            protection,
            backoff,
            power: request.power,
            coex: request.coex,
            content,
        }
    }
}

impl TxAttemptPlan {
    /// The protection exchange of the attempt as the port requests it.
    pub const fn port_protection(&self) -> oer_ieee80211_lower_mac::Protection {
        self.protection.protection.port_protection()
    }

    /// Whether the attempt is protected by a control exchange.
    pub const fn is_protected(&self) -> bool {
        !matches!(self.protection.protection, TxProtection::None)
    }
}

/// The report's view of a BlockAck, for callers that keep their own
/// statistics: the subframes of `request` that `report` acknowledges.
pub fn acknowledged_subframes(request: &AmpduRequest, report: BlockAckReport) -> u64 {
    let mut acknowledged = 0_u64;
    for index in 0..request.subframes {
        if report.acknowledges(request.sequence(index)) {
            acknowledged |= 1 << index;
        }
    }
    acknowledged
}

#[cfg(test)]
mod tests;
