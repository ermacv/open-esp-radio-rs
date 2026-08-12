//! Executor-neutral transmit policy and bounded retry state.
//!
//! This module owns the finite policy between one detached hardware
//! completion and the next publication decision. It deliberately does not
//! wait for interrupts, access MMIO, mutate DMA storage or produce entropy;
//! those remain separate hardware/executor boundaries.

use open_esp_radio_ieee80211::wmm::WmmParameterSet;

use crate::{
    edca::{EdcaContentionParameters, EdcaParametersError, EdcaQueues},
    tx::{HtPeerAmpduParameters, LegacyTxQueue, TxPhyRate},
    tx_ampdu::HtAmpduTxCompletion,
};

const IEEE80211_SEQUENCE_MASK: u16 = 0x0fff;
const HARDWARE_BLOCK_ACK_WINDOW: usize = 32;

/// Association-derived state used by all ordinary and aggregate TX paths.
///
/// The state is kept together so the HIL cannot accidentally update a peer's
/// HT capability, BSS color and WMM contention policy through independent
/// ad-hoc fields. Entropy is supplied by the platform at the point where a
/// hardware queue is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxRuntimePolicy {
    ht_ampdu: HtPeerAmpduParameters,
    he_bss_color: u8,
    edca: EdcaQueues,
}

impl WifiTxRuntimePolicy {
    /// Start with the same cold values as the complete vendor LMAC init.
    pub const fn vendor_defaults() -> Self {
        Self {
            ht_ampdu: HtPeerAmpduParameters::from_capability_byte(0),
            he_bss_color: 0,
            edca: EdcaQueues::vendor_defaults(),
        }
    }

    pub fn install_ht_ampdu(&mut self, parameters: HtPeerAmpduParameters) {
        self.ht_ampdu = parameters;
    }

    pub const fn ht_ampdu(&self) -> HtPeerAmpduParameters {
        self.ht_ampdu
    }

    /// Install the six-bit HE BSS color decoded from the peer's BSS Color IE.
    pub fn install_he_bss_color(&mut self, bss_color: u8) {
        self.he_bss_color = bss_color & 0x3f;
    }

    pub const fn he_bss_color(&self) -> u8 {
        self.he_bss_color
    }

    /// Atomically validate and install all four WMM access categories.
    pub fn install_wmm(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        self.edca.configure_from_wmm(parameters)
    }

    pub fn contention_parameters(&self, queue: LegacyTxQueue) -> EdcaContentionParameters {
        self.edca.queue(queue).parameters()
    }

    pub fn contention_exponent(&self, queue: LegacyTxQueue) -> u8 {
        self.edca.queue(queue).current_exponent()
    }

    /// Select one hardware backoff slot from platform-provided entropy.
    pub fn select_backoff(&self, queue: LegacyTxQueue, entropy: u32) -> u16 {
        self.edca.select_slot(queue, entropy)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.edca.record_retry_failure(queue);
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.edca.record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.edca.reset_terminal_exchange(queue);
    }
}

impl Default for WifiTxRuntimePolicy {
    fn default() -> Self {
        Self::vendor_defaults()
    }
}

/// Invalid construction or a missing vendor retry-ladder entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnicastRetryError {
    ZeroAttemptLimit,
    RetryRateUnavailable { failed_attempts: u8 },
}

/// Driver-owned action after one ordinary MPDU attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnicastRetryDecision {
    Complete,
    /// Re-publish the same encoded MPDU after setting its Retry bit.
    Retry,
}

/// One bounded ordinary-MPDU retry transaction.
///
/// The caller retains the encoded MPDU and its DMA storage. This type owns
/// attempt counting, exact vendor rate-ladder selection and EDCA CW changes.
pub struct UnicastRetryState {
    queue: LegacyTxQueue,
    initial_rate: TxPhyRate,
    attempt_limit: u8,
    attempt: u8,
}

impl UnicastRetryState {
    pub const fn new(
        queue: LegacyTxQueue,
        initial_rate: TxPhyRate,
        attempt_limit: u8,
    ) -> Result<Self, UnicastRetryError> {
        if attempt_limit == 0 {
            return Err(UnicastRetryError::ZeroAttemptLimit);
        }
        Ok(Self {
            queue,
            initial_rate,
            attempt_limit,
            attempt: 1,
        })
    }

    pub const fn attempt(&self) -> u8 {
        self.attempt
    }

    /// Select the rate for the current publication.
    ///
    /// SOURCE: complete `libpp.a[trc.o]::rcGetRate` and the exact
    /// Rust-owned Dot11G/Dot11N schedule arenas. Explicit HT SGI MCS0..6 has
    /// no independent vendor record, so it retains the requested rate. The
    /// currently qualified HE ordinary path likewise retains its original
    /// rate until the full vendor HE transition is promoted.
    pub fn current_rate(&self) -> Result<TxPhyRate, UnicastRetryError> {
        let failed_attempts = self.attempt - 1;
        match self.initial_rate {
            TxPhyRate::Legacy(rate) => rate
                .vendor_retry_rate(failed_attempts)
                .map(TxPhyRate::Legacy)
                .ok_or(UnicastRetryError::RetryRateUnavailable { failed_attempts }),
            TxPhyRate::Ht(rate) => Ok(rate
                .vendor_retry_rate(failed_attempts)
                .unwrap_or(TxPhyRate::Ht(rate))),
            TxPhyRate::He(rate) => Ok(TxPhyRate::He(rate)),
        }
    }

    /// Apply one detached hardware completion.
    ///
    /// SOURCE: complete `libpp.a[lmac.o]::{lmacProcessTxComplete,
    /// lmacProcessAckTimeout,lmacProcessShortRetryFail,
    /// lmacProcessLongRetryFail}`. Status 0 succeeds; statuses 2 and 5 are
    /// bounded retry candidates; every other status terminates the exchange.
    pub fn observe_completion(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
        status: u8,
    ) -> UnicastRetryDecision {
        if status == 0 {
            policy.record_success(self.queue);
            return UnicastRetryDecision::Complete;
        }
        if matches!(status, 2 | 5) && self.attempt < self.attempt_limit {
            policy.record_retry_failure(self.queue);
            self.attempt += 1;
            return UnicastRetryDecision::Retry;
        }
        policy.reset_terminal_exchange(self.queue);
        UnicastRetryDecision::Complete
    }

    /// Treat a bounded software observation timeout like the vendor ACK/CTS
    /// timeout path while publications remain in the transaction budget.
    pub fn observe_hardware_timeout(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
    ) -> UnicastRetryDecision {
        if self.attempt < self.attempt_limit {
            policy.record_retry_failure(self.queue);
            self.attempt += 1;
            UnicastRetryDecision::Retry
        } else {
            policy.reset_terminal_exchange(self.queue);
            UnicastRetryDecision::Complete
        }
    }

    /// Apply one detached ordinary-queue collision.
    ///
    /// A collision consumes the same bounded publication/EDCA budget as the
    /// recovered timeout retry body, but the caller does not set the 802.11
    /// Retry bit: hardware never completed the original MPDU exchange.
    pub fn observe_collision(&mut self, policy: &mut WifiTxRuntimePolicy) -> UnicastRetryDecision {
        if self.attempt < self.attempt_limit {
            policy.record_retry_failure(self.queue);
            self.attempt += 1;
            UnicastRetryDecision::Retry
        } else {
            policy.reset_terminal_exchange(self.queue);
            UnicastRetryDecision::Complete
        }
    }

    /// End ownership after a non-retryable executor or hardware error.
    pub fn abort(&self, policy: &mut WifiTxRuntimePolicy) {
        policy.reset_terminal_exchange(self.queue);
    }
}

/// Policy for retained A-MPDU retries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduRetryPolicy {
    /// Maximum number of aggregate publications, including the first one.
    pub attempt_limit: u8,
    /// Keep one missing MPDU in the aggregate owner.
    ///
    /// The recovered HE path requires this because converting a one-member
    /// HE A-MPDU to the ordinary queue first needs the distinct
    /// `ppHEAMPDU2Normal` metadata transition. The qualified HT path instead
    /// sends one remaining MPDU through its ordinary retry owner.
    pub retain_single_mpdu: bool,
}

/// Invalid construction or a disagreement with the pinned DMA owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryError {
    ZeroAttemptLimit,
    EmptyAggregate,
    CapacityExceedsHardwareWindow { capacity: usize },
    AggregateExceedsCapacity { subframes: u8, capacity: usize },
    FrameCountChanged { expected: u8, observed: u8 },
}

/// Driver-owned action after one BlockAck completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduRetryDecision {
    /// Retain and compact the selected MPDUs, then publish another A-MPDU.
    RetainAggregate { retry_mask: u32 },
    /// End aggregate ownership; selected MPDUs require individual retry.
    Finish { retry_mask: u32 },
    /// End ownership through the vendor Trigger-based completion path.
    ///
    /// No ordinary BlockAck was received, so this must remain distinct from
    /// `Finish { retry_mask: 0 }` for statistics and rate-control purposes.
    FinishTriggerFlow,
}

impl AmpduRetryDecision {
    pub const fn retry_mask(self) -> u32 {
        match self {
            Self::RetainAggregate { retry_mask } | Self::Finish { retry_mask } => retry_mask,
            Self::FinishTriggerFlow => 0,
        }
    }

    pub const fn missing(self) -> u8 {
        self.retry_mask().count_ones() as u8
    }
}

/// One bounded BlockAck/retry transaction.
///
/// Sequence numbers are kept independently of descriptor indices because a
/// partial BlockAck retry compacts only missing MPDUs toward slot zero.
pub struct AmpduRetryState<const CAPACITY: usize> {
    sequences: [u16; CAPACITY],
    current_subframes: u8,
    policy: AmpduRetryPolicy,
    aggregate_attempts: u8,
    acknowledged: u8,
    block_ack_mpdu_attempts: u16,
    trigger_flow_completions: u8,
}

impl<const CAPACITY: usize> AmpduRetryState<CAPACITY> {
    /// Start at the first Sequence Control value already consumed by the
    /// encoded aggregate.
    pub fn new(
        first_sequence: u16,
        subframes: u8,
        policy: AmpduRetryPolicy,
    ) -> Result<Self, AmpduRetryError> {
        if policy.attempt_limit == 0 {
            return Err(AmpduRetryError::ZeroAttemptLimit);
        }
        if CAPACITY > HARDWARE_BLOCK_ACK_WINDOW {
            return Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: CAPACITY });
        }
        if subframes == 0 {
            return Err(AmpduRetryError::EmptyAggregate);
        }
        if usize::from(subframes) > CAPACITY {
            return Err(AmpduRetryError::AggregateExceedsCapacity {
                subframes,
                capacity: CAPACITY,
            });
        }
        let mut sequences = [0_u16; CAPACITY];
        let mut index = 0_usize;
        while index < subframes as usize {
            sequences[index] = first_sequence.wrapping_add(index as u16) & IEEE80211_SEQUENCE_MASK;
            index += 1;
        }
        Ok(Self {
            sequences,
            current_subframes: subframes,
            policy,
            aggregate_attempts: 1,
            acknowledged: 0,
            block_ack_mpdu_attempts: 0,
            trigger_flow_completions: 0,
        })
    }

    /// Apply one completion after the hardware queue has been detached.
    ///
    /// SOURCE: complete `libpp.a[pp.o]::ppResortTxAMPDU` preserves
    /// Sequence Control and compacts only the MPDUs absent from BlockAck.
    /// Complete `libpp.a[lmac.o]::lmacRetryTxFrame` skips
    /// `rcGetRate` for the state written by
    /// `lmacProcessLongRetryFail`, so a retained aggregate keeps its PHY rate.
    pub fn observe(
        &mut self,
        completion: HtAmpduTxCompletion,
        observed_subframes: u8,
    ) -> Result<AmpduRetryDecision, AmpduRetryError> {
        if observed_subframes != self.current_subframes {
            return Err(AmpduRetryError::FrameCountChanged {
                expected: self.current_subframes,
                observed: observed_subframes,
            });
        }

        // Complete `libpp.a[lmac.o]::lmacProcessTxComplete` maps
        // status five to `lmacProcessAckTimeout`. Both its short- and
        // long-frame leaves call `lmacProcessTBSuccess(queue, 0x7f)` instead
        // of retrying when the queue is in Trigger flow and its applicable
        // packet counts are zero. Keep this before BlockAck accounting: the
        // vendor path terminates the frame-exchange sequence without an
        // ordinary BlockAck or an ordinary MPDU attempt count.
        if completion.tx.completes_vendor_trigger_flow() {
            self.trigger_flow_completions = self.trigger_flow_completions.saturating_add(1);
            return Ok(AmpduRetryDecision::FinishTriggerFlow);
        }
        self.block_ack_mpdu_attempts = self
            .block_ack_mpdu_attempts
            .saturating_add(u16::from(observed_subframes));

        let mut retry_mask = 0_u32;
        let mut index = 0_usize;
        while index < observed_subframes as usize {
            if completion.acknowledges(self.sequences[index]) {
                self.acknowledged = self.acknowledged.saturating_add(1);
            } else {
                retry_mask |= 1_u32 << index;
            }
            index += 1;
        }
        let missing = retry_mask.count_ones() as u8;
        let retain = (missing >= 2 || (missing == 1 && self.policy.retain_single_mpdu))
            && self.aggregate_attempts < self.policy.attempt_limit;
        if !retain {
            return Ok(AmpduRetryDecision::Finish { retry_mask });
        }

        let mut destination = 0_usize;
        let mut source = 0_usize;
        while source < observed_subframes as usize {
            if retry_mask & (1_u32 << source) != 0 {
                self.sequences[destination] = self.sequences[source];
                destination += 1;
            }
            source += 1;
        }
        self.current_subframes = missing;
        self.aggregate_attempts = self.aggregate_attempts.saturating_add(1);
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask })
    }

    pub const fn current_subframes(&self) -> u8 {
        self.current_subframes
    }

    pub const fn current_first_sequence(&self) -> u16 {
        self.sequences[0]
    }

    pub const fn aggregate_attempts(&self) -> u8 {
        self.aggregate_attempts
    }

    pub const fn acknowledged(&self) -> u8 {
        self.acknowledged
    }

    pub const fn block_ack_mpdu_attempts(&self) -> u16 {
        self.block_ack_mpdu_attempts
    }

    /// Number of terminal completions handled through `lmacProcessTBSuccess`
    /// semantics rather than an ordinary BlockAck.
    pub const fn trigger_flow_completions(&self) -> u8 {
        self.trigger_flow_completions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tx::{LegacyRate, TxCompletion, TxCookie},
        tx_ampdu::{HtAmpduTxCompletion, HtBlockAckRegisters, TxBlockAckBitmap},
    };

    #[test]
    fn sta_runtime_policy_owns_peer_and_vendor_edca_state() {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        assert_eq!(policy.he_bss_color(), 0);
        assert_eq!(
            policy.contention_parameters(LegacyTxQueue::BestEffort),
            EdcaContentionParameters::new(3, 4, 10).unwrap()
        );
        assert_eq!(policy.contention_exponent(LegacyTxQueue::BestEffort), 4);

        policy.install_he_bss_color(0xff);
        policy.install_ht_ampdu(HtPeerAmpduParameters::from_capability_byte(0x17));
        assert_eq!(policy.he_bss_color(), 0x3f);
        assert_eq!(
            policy.ht_ampdu(),
            HtPeerAmpduParameters::from_capability_byte(0x17)
        );
        assert_eq!(
            policy.select_backoff(LegacyTxQueue::BestEffort, u32::MAX),
            15
        );
    }

    #[test]
    fn ordinary_retry_owns_rate_ladder_and_edca_transitions() {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        let queue = LegacyTxQueue::BestEffort;
        let mut retry =
            UnicastRetryState::new(queue, TxPhyRate::Legacy(LegacyRate::Ofdm54M), 4).unwrap();

        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
        );
        assert_eq!(
            retry.observe_completion(&mut policy, 5),
            UnicastRetryDecision::Retry
        );
        assert_eq!(retry.attempt(), 2);
        assert_eq!(policy.contention_exponent(queue), 5);
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
        );

        assert_eq!(
            retry.observe_completion(&mut policy, 2),
            UnicastRetryDecision::Retry
        );
        assert_eq!(retry.attempt(), 3);
        assert_eq!(policy.contention_exponent(queue), 6);
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm48M))
        );

        assert_eq!(
            retry.observe_completion(&mut policy, 0),
            UnicastRetryDecision::Complete
        );
        assert_eq!(policy.contention_exponent(queue), 4);
    }

    #[test]
    fn ordinary_retry_limit_and_abort_restore_the_minimum_cw() {
        let queue = LegacyTxQueue::Voice;
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        let mut retry =
            UnicastRetryState::new(queue, TxPhyRate::Legacy(LegacyRate::Ofdm6M), 2).unwrap();
        assert_eq!(
            retry.observe_hardware_timeout(&mut policy),
            UnicastRetryDecision::Retry
        );
        assert_eq!(policy.contention_exponent(queue), 3);
        assert_eq!(
            retry.observe_hardware_timeout(&mut policy),
            UnicastRetryDecision::Complete
        );
        assert_eq!(policy.contention_exponent(queue), 2);

        policy.record_retry_failure(queue);
        retry.abort(&mut policy);
        assert_eq!(policy.contention_exponent(queue), 2);
        assert_eq!(
            UnicastRetryState::new(queue, TxPhyRate::Legacy(LegacyRate::Ofdm6M), 0).err(),
            Some(UnicastRetryError::ZeroAttemptLimit)
        );
    }

    fn completion_with_tx(
        tx: TxCompletion,
        starting_sequence: u16,
        bitmap: u64,
    ) -> HtAmpduTxCompletion {
        HtAmpduTxCompletion {
            tx,
            block_ack: HtBlockAckRegisters {
                control: 0,
                block_ack: TxBlockAckBitmap::new(starting_sequence, bitmap),
            },
            block_ack_received: true,
        }
    }

    fn completion(status: u8, starting_sequence: u16, bitmap: u64) -> HtAmpduTxCompletion {
        completion_with_tx(
            TxCompletion {
                cookie: TxCookie(1),
                status,
                trigger_flow: false,
                used_alternate: false,
                auxiliary_a_word: 0,
                auxiliary_b_word: 0,
                auxiliary_c_word: 0,
                primary_word: 0,
                alternate_word: 0,
            },
            starting_sequence,
            bitmap,
        )
    }

    const HT_POLICY: AmpduRetryPolicy = AmpduRetryPolicy {
        attempt_limit: 4,
        retain_single_mpdu: false,
    };

    #[test]
    fn partial_block_ack_compacts_sequences_across_retained_attempts() {
        let mut state = AmpduRetryState::<32>::new(0x0ffe, 4, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(0, 0x0ffe, 0b0101), 4),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b1010 })
        );
        assert_eq!(state.current_subframes(), 2);
        assert_eq!(state.acknowledged(), 2);
        assert_eq!(state.block_ack_mpdu_attempts(), 4);

        // The retained sequences are 0x0fff and 1. A BlockAck starting at
        // 0x0fff acknowledges both across the 12-bit wrap.
        assert_eq!(
            state.observe(completion(0, 0x0fff, 0b0101), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
        );
        assert_eq!(state.acknowledged(), 4);
        assert_eq!(state.aggregate_attempts(), 2);
        assert_eq!(state.block_ack_mpdu_attempts(), 6);
    }

    #[test]
    fn advanced_block_ack_ssn_completes_preceding_mpdu_without_retry() {
        let mut state = AmpduRetryState::<4>::new(100, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(0, 102, 0), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
        );
        assert_eq!(state.acknowledged(), 2);
        assert_eq!(state.aggregate_attempts(), 1);
    }

    #[test]
    fn nonzero_status_ignores_a_stale_block_ack_bitmap() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(5, 20, u64::MAX), 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert_eq!(state.acknowledged(), 0);
    }

    #[test]
    fn missing_block_ack_result_ignores_a_stale_success_bitmap() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        let mut stale = completion(0, 20, u64::MAX);
        stale.block_ack_received = false;
        assert_eq!(
            state.observe(stale, 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
        );
        assert_eq!(state.acknowledged(), 0);
    }

    #[test]
    fn ht_finishes_one_missing_mpdu_but_he_retains_it() {
        let mut ht = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        assert_eq!(
            ht.observe(completion(0, 20, 0b01), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0b10 })
        );

        let mut he = AmpduRetryState::<4>::new(
            20,
            2,
            AmpduRetryPolicy {
                retain_single_mpdu: true,
                ..HT_POLICY
            },
        )
        .unwrap();
        assert_eq!(
            he.observe(completion(0, 20, 0b01), 2),
            Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b10 })
        );
        assert_eq!(he.current_subframes(), 1);
    }

    #[test]
    fn attempt_limit_finishes_the_last_failed_aggregate() {
        let mut state = AmpduRetryState::<4>::new(
            100,
            2,
            AmpduRetryPolicy {
                attempt_limit: 2,
                retain_single_mpdu: false,
            },
        )
        .unwrap();
        assert!(matches!(
            state.observe(completion(5, 100, 0), 2),
            Ok(AmpduRetryDecision::RetainAggregate { .. })
        ));
        assert_eq!(
            state.observe(completion(5, 100, 0), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0b11 })
        );
        assert_eq!(state.aggregate_attempts(), 2);
    }

    #[test]
    fn construction_and_dma_count_disagreements_fail_closed() {
        assert!(matches!(
            AmpduRetryState::<33>::new(0, 1, HT_POLICY),
            Err(AmpduRetryError::CapacityExceedsHardwareWindow { capacity: 33 })
        ));
        assert!(matches!(
            AmpduRetryState::<2>::new(0, 3, HT_POLICY),
            Err(AmpduRetryError::AggregateExceedsCapacity { .. })
        ));
        let mut state = AmpduRetryState::<2>::new(0, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(0, 0, 0b11), 1),
            Err(AmpduRetryError::FrameCountChanged {
                expected: 2,
                observed: 1
            })
        );
    }

    #[test]
    fn vendor_trigger_timeout_finishes_without_fabricating_block_ack() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        let trigger_timeout = completion_with_tx(
            TxCompletion {
                cookie: TxCookie(1),
                status: TxCompletion::ACK_TIMEOUT_STATUS,
                trigger_flow: true,
                used_alternate: false,
                auxiliary_a_word: 0,
                auxiliary_b_word: 0,
                auxiliary_c_word: 0,
                primary_word: 0,
                alternate_word: 0,
            },
            20,
            u64::MAX,
        );

        assert_eq!(
            state.observe(trigger_timeout, 2),
            Ok(AmpduRetryDecision::FinishTriggerFlow)
        );
        assert_eq!(state.trigger_flow_completions(), 1);
        assert_eq!(state.acknowledged(), 0);
        assert_eq!(state.block_ack_mpdu_attempts(), 0);
        assert_eq!(state.aggregate_attempts(), 1);
    }

    #[test]
    fn trigger_timeout_with_reported_packets_stays_on_retry_path() {
        for (auxiliary_b_word, auxiliary_c_word) in [
            (1 << 13, 0),
            ((1 << 20), 1 << 7),
            ((1 << 20) | (1 << 13), 0),
        ] {
            let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
            let completion = completion_with_tx(
                TxCompletion {
                    cookie: TxCookie(1),
                    status: TxCompletion::ACK_TIMEOUT_STATUS,
                    trigger_flow: true,
                    used_alternate: false,
                    auxiliary_a_word: 0,
                    auxiliary_b_word,
                    auxiliary_c_word,
                    primary_word: 0,
                    alternate_word: 0,
                },
                20,
                u64::MAX,
            );
            assert_eq!(
                state.observe(completion, 2),
                Ok(AmpduRetryDecision::RetainAggregate { retry_mask: 0b11 })
            );
            assert_eq!(state.trigger_flow_completions(), 0);
            assert_eq!(state.block_ack_mpdu_attempts(), 2);
        }
    }

    #[test]
    fn trigger_success_predicate_rejects_wrong_status_or_queue_state() {
        for (status, trigger_flow) in [(0, true), (4, true), (5, false)] {
            let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
            let completion = completion_with_tx(
                TxCompletion {
                    cookie: TxCookie(1),
                    status,
                    trigger_flow,
                    used_alternate: false,
                    auxiliary_a_word: 0,
                    auxiliary_b_word: 0,
                    auxiliary_c_word: 0,
                    primary_word: 0,
                    alternate_word: 0,
                },
                20,
                0,
            );
            assert_ne!(
                state.observe(completion, 2),
                Ok(AmpduRetryDecision::FinishTriggerFlow)
            );
        }
    }
}
