//! Executor-neutral transmit policy and bounded retry state.
//!
//! This module owns the finite policy between one detached hardware
//! completion and the next publication decision. It deliberately does not
//! wait for interrupts, access MMIO, mutate DMA storage or produce entropy;
//! those remain separate hardware/executor boundaries.

use open_esp_radio_ieee80211::wmm::{
    WmmAccessCategory, WmmParameterSet, WmmTrafficClass, WmmUserPriority, classify_ethernet_wmm,
};

use crate::{
    edca::{EdcaAccessPolicy, EdcaContentionParameters, EdcaParametersError, EdcaQueues},
    rate_schedule::{RateScheduleKind, RateScheduleRef, schedule_rate_after_failures},
    tx::{
        HeEdcaTxopLimit, HtChannelWidth, HtPeerAmpduParameters, LegacyTxQueue,
        TxCompletionDisposition, TxPhyRate,
    },
    tx_ampdu::HtAmpduTxCompletion,
    tx_protection::WifiTxProtectionPolicy,
};

const IEEE80211_SEQUENCE_MASK: u16 = 0x0fff;
const HARDWARE_BLOCK_ACK_WINDOW: usize = 32;

/// Result of applying the peer's negotiated ACM policy to one classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmAdmissionDisposition {
    /// The selected AC does not require an admission.
    AdmissionNotRequired,
    Downgraded {
        requested: WmmAccessCategory,
    },
    /// A non-QoS association owns only the legacy best-effort sequence space.
    NonQosBestEffort,
}

/// Complete station-side queue/TID policy selected for one network frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxTraffic {
    pub requested: WmmTrafficClass,
    pub user_priority: WmmUserPriority,
    pub access_category: WmmAccessCategory,
    pub admission: WmmAdmissionDisposition,
    txop_limit_units_32_us: u16,
}

impl WifiTxTraffic {
    pub const fn queue(self) -> LegacyTxQueue {
        LegacyTxQueue::from_access_category(self.access_category)
    }

    pub const fn tid(self) -> u8 {
        self.user_priority.value()
    }

    pub const fn txop_limit_units_32_us(self) -> u16 {
        self.txop_limit_units_32_us
    }

    /// Resolve the peer's negotiated HE duration budget and an optional
    /// integration ceiling without widening either value.
    pub const fn he_txop_limit(
        self,
        configured_ceiling: HeEdcaTxopLimit,
    ) -> Result<HeEdcaTxopLimit, WmmTxopUnsupported> {
        let Some(negotiated) = HeEdcaTxopLimit::from_units_32_us(self.txop_limit_units_32_us)
        else {
            return Err(WmmTxopUnsupported::AdvertisedLimitTooWide {
                units_32_us: self.txop_limit_units_32_us,
            });
        };
        if negotiated.is_default() {
            return Ok(configured_ceiling);
        }
        if configured_ceiling.is_default()
            || negotiated.units_32_us() <= configured_ceiling.units_32_us()
        {
            Ok(negotiated)
        } else {
            Ok(configured_ceiling)
        }
    }

    /// HT aggregation has no reviewed negotiated-TXOP duration calculator.
    pub const fn require_ht_txop_support(self) -> Result<(), WmmTxopUnsupported> {
        if self.txop_limit_units_32_us == 0 {
            Ok(())
        } else {
            Err(WmmTxopUnsupported::HtAggregateDurationBudget {
                units_32_us: self.txop_limit_units_32_us,
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxTrafficError {
    /// Every possible AC at or below the requested priority requires an
    /// admission, while no TSPEC/ADDTS owner exists in this driver.
    AdmissionControlRequired { requested: WmmAccessCategory },
}

/// Unreviewed boundary which must not be inferred from WMM parsing alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmTxopUnsupported {
    AdvertisedLimitTooWide { units_32_us: u16 },
    HtAggregateDurationBudget { units_32_us: u16 },
    RtsCtsProtection,
    MultiPpduMediumOwnership,
}

/// Requests which would require an unproven hardware medium-ownership path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WmmHardwareMediumRequest {
    RtsCtsProtection,
    MultiPpduTxop,
}

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
    protection: WifiTxProtectionPolicy,
}

impl WifiTxRuntimePolicy {
    /// Start with the same cold values as the complete vendor LMAC init.
    pub const fn vendor_defaults() -> Self {
        Self {
            ht_ampdu: HtPeerAmpduParameters::from_capability_byte(0),
            he_bss_color: 0,
            edca: EdcaQueues::vendor_defaults(),
            protection: WifiTxProtectionPolicy::new(
                crate::tx_protection::ErpProtectionMode::None,
                crate::tx_protection::HtProtectionMode::None,
                None,
            ),
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

    /// Atomically replace the association/BSS protection facts.  Active TX
    /// owners retain this value across ordinary and A-MPDU retries.
    pub fn install_protection(&mut self, protection: WifiTxProtectionPolicy) {
        self.protection = protection;
    }

    pub const fn protection(&self) -> WifiTxProtectionPolicy {
        self.protection
    }

    /// Atomically validate and install all four WMM access categories.
    pub fn install_wmm(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        self.edca.configure_from_wmm(parameters)
    }

    pub fn contention_parameters(&self, queue: LegacyTxQueue) -> EdcaContentionParameters {
        self.edca.queue(queue).parameters()
    }

    pub const fn access_policy(&self, queue: LegacyTxQueue) -> EdcaAccessPolicy {
        self.edca.access_policy(queue)
    }

    /// Classify and admit one network frame under the active peer WMM policy.
    ///
    /// With no QoS peer, all markings collapse to legacy BE. With QoS, an ACM
    /// bit is never treated as an admission: the request walks to a lower AC
    /// and rewrites its TID to that AC's canonical value. If even BK requires
    /// admission, the request fails closed.
    pub fn select_network_traffic(
        &self,
        ethernet: &[u8],
        peer_qos: bool,
    ) -> Result<WifiTxTraffic, WifiTxTrafficError> {
        let requested = classify_ethernet_wmm(ethernet);
        if !peer_qos {
            let selected = self.edca.access_policy(LegacyTxQueue::BestEffort);
            return Ok(WifiTxTraffic {
                requested,
                user_priority: WmmUserPriority::UP0,
                access_category: WmmAccessCategory::BestEffort,
                admission: WmmAdmissionDisposition::NonQosBestEffort,
                txop_limit_units_32_us: selected.txop_limit_units_32_us(),
            });
        }

        let mut category = requested.access_category;
        loop {
            let selected = self
                .edca
                .access_policy(LegacyTxQueue::from_access_category(category));
            if !selected.admission_control_mandatory() {
                let downgraded = category != requested.access_category;
                return Ok(WifiTxTraffic {
                    requested,
                    user_priority: if downgraded {
                        category.canonical_user_priority()
                    } else {
                        requested.user_priority
                    },
                    access_category: category,
                    admission: if downgraded {
                        WmmAdmissionDisposition::Downgraded {
                            requested: requested.access_category,
                        }
                    } else {
                        WmmAdmissionDisposition::AdmissionNotRequired
                    },
                    txop_limit_units_32_us: selected.txop_limit_units_32_us(),
                });
            }
            let Some(lower) = category.downgrade() else {
                return Err(WifiTxTrafficError::AdmissionControlRequired {
                    requested: requested.access_category,
                });
            };
            category = lower;
        }
    }

    /// Explicit frontier for hardware operations not established by the
    /// recovered ordinary/A-MPDU publication contract.
    pub const fn request_hardware_medium(
        &self,
        request: WmmHardwareMediumRequest,
    ) -> Result<(), WmmTxopUnsupported> {
        match request {
            WmmHardwareMediumRequest::RtsCtsProtection => Err(WmmTxopUnsupported::RtsCtsProtection),
            WmmHardwareMediumRequest::MultiPpduTxop => {
                Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
            }
        }
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

/// Complete `lmacInit` defaults stored in `lmacConfMib[0x15]` and `[0x14]`.
pub const VENDOR_SHORT_RETRY_LIMIT: u8 = 0x20;
pub const VENDOR_LONG_RETRY_LIMIT: u8 = 0x20;
pub const VENDOR_RTS_THRESHOLD_BYTES: u32 = 0x092a;

/// Invalid construction or a missing normal-schedule rate entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryRetryError {
    ZeroMpduRetryLimit,
    RetryRateUnavailable {
        retry_index: u8,
    },
    P2pInitialRateMismatch {
        initial: TxPhyRate,
        scheduled: TxPhyRate,
    },
    P2pHtSgiFallbackMismatch {
        initial: TxPhyRate,
        scheduled: TxPhyRate,
    },
}

/// One validated recovered standard-rate P2P retry record.
///
/// LR records are intentionally excluded: a retry policy cannot establish
/// the missing LR PLCP, receive-status and scoped PHY ownership contracts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct P2pRetryRateSchedule(RateScheduleRef);

impl P2pRetryRateSchedule {
    pub const fn new(schedule: RateScheduleRef) -> Option<Self> {
        if matches!(
            schedule.kind,
            RateScheduleKind::P2pDot11G | RateScheduleKind::P2pDot11N
        ) && (schedule.index as usize) < schedule.kind.record_count()
        {
            Some(Self(schedule))
        } else {
            None
        }
    }

    pub const fn schedule(self) -> RateScheduleRef {
        self.0
    }
}

/// Rate-ladder ownership for one retained ordinary MPDU.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OrdinaryRetryRatePolicy {
    /// Use the ordinary associated-station Dot11G/Dot11N ladder selected from
    /// the initial hardware rate.
    #[default]
    Normal,
    /// Use one exact recovered P2P record for every retained publication.
    P2p(P2pRetryRateSchedule),
    /// Publish one explicitly selected HT20 SGI rate, then enter the exact
    /// same-MCS HT20 LGI P2P record after the first failed attempt.
    ///
    /// The recovered P2P arena contains an SGI record only for MCS7. The
    /// queue formatter nevertheless owns every HT20 SGI MCS0..7 code. This
    /// source-owned bridge does not invent a missing record: attempt zero is
    /// the caller's typed SGI rate, while every retry is selected from an
    /// existing LGI record at the original failure count. Construction below
    /// proves that the first scheduled retry is the same MCS at LGI.
    P2pHtSgiFallback(P2pRetryRateSchedule),
}

/// Driver-owned action after one ordinary MPDU attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryRetryDecision {
    Complete,
    /// Re-publish the same encoded MPDU. Only the ACK-timeout path sets the
    /// 802.11 Retry bit; CTS timeout and collision deliberately do not.
    Retry {
        set_retry_bit: bool,
    },
}

/// Vendor short/long classification used by the separate retry counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryFrameClass {
    Short,
    Long,
}

/// Descriptor retry bytes consumed by `rcGetRate`, `rcReachRetryLimit` and
/// the LMAC short/long retry-limit checks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrdinaryRetryCounters {
    pub mpdu: u8,
    pub short: u8,
    pub long: u8,
}

/// One bounded ordinary-MPDU retry transaction for the normal schedule path.
///
/// The caller retains the encoded MPDU and its DMA storage. This type owns
/// the three vendor descriptor counters, publication count, normal
/// `rcGetRate` ladder selection and EDCA CW changes.
pub struct OrdinaryMpduRetryState {
    queue: LegacyTxQueue,
    initial_rate: TxPhyRate,
    rate_policy: OrdinaryRetryRatePolicy,
    mpdu_retry_limit: u8,
    publications: u8,
    frame_class: OrdinaryFrameClass,
    counters: OrdinaryRetryCounters,
}

impl OrdinaryMpduRetryState {
    pub const fn new(
        queue: LegacyTxQueue,
        initial_rate: TxPhyRate,
        mpdu_retry_limit: u8,
        frame_class: OrdinaryFrameClass,
    ) -> Result<Self, OrdinaryRetryError> {
        if mpdu_retry_limit == 0 {
            return Err(OrdinaryRetryError::ZeroMpduRetryLimit);
        }
        Ok(Self {
            queue,
            initial_rate,
            rate_policy: OrdinaryRetryRatePolicy::Normal,
            mpdu_retry_limit,
            publications: 1,
            frame_class,
            counters: OrdinaryRetryCounters {
                mpdu: 0,
                short: 0,
                long: 0,
            },
        })
    }

    /// Construct an ordinary retry owner with an explicit standard P2P
    /// ladder. The first record rate must exactly match the published initial
    /// rate, preventing a caller from changing PHY only after the first ACK
    /// timeout.
    pub fn new_with_rate_policy(
        queue: LegacyTxQueue,
        initial_rate: TxPhyRate,
        rate_policy: OrdinaryRetryRatePolicy,
        mpdu_retry_limit: u8,
        frame_class: OrdinaryFrameClass,
    ) -> Result<Self, OrdinaryRetryError> {
        let mut state = Self::new(queue, initial_rate, mpdu_retry_limit, frame_class)?;
        match rate_policy {
            OrdinaryRetryRatePolicy::Normal => {}
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                let scheduled = select_p2p_retry_rate(schedule, 0)?;
                if scheduled != initial_rate {
                    return Err(OrdinaryRetryError::P2pInitialRateMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                let scheduled = select_p2p_retry_rate(schedule, 1)?;
                let valid = matches!(
                    (initial_rate, scheduled),
                    (TxPhyRate::Ht(initial), TxPhyRate::Ht(fallback))
                        if initial.channel_width == HtChannelWidth::Mhz20
                            && initial.guard_interval
                                == crate::tx::HtGuardInterval::Short400Ns
                            && fallback.channel_width == HtChannelWidth::Mhz20
                            && fallback.guard_interval
                                == crate::tx::HtGuardInterval::Long800Ns
                            && fallback.mcs == initial.mcs
                );
                if !valid {
                    return Err(OrdinaryRetryError::P2pHtSgiFallbackMismatch {
                        initial: initial_rate,
                        scheduled,
                    });
                }
            }
        }
        state.rate_policy = rate_policy;
        Ok(state)
    }

    pub const fn publications(&self) -> u8 {
        self.publications
    }

    pub const fn counters(&self) -> OrdinaryRetryCounters {
        self.counters
    }

    /// Select the rate for the current publication.
    ///
    /// Both normal and P2P records use the `rcGetRate` retry counters. The
    /// descriptor bypass and context-fixed-rate branches remain separate
    /// production modes and are not inferred here. Selection uses
    /// `max(desc[5], desc[6])`; a long collision changes only `desc[7]` and
    /// therefore retains its current rate.
    pub fn current_rate(&self) -> Result<TxPhyRate, OrdinaryRetryError> {
        let retry_index = self.counters.mpdu.max(self.counters.short);
        self.rate_after_failed_attempts(retry_index)
    }

    /// Inspect one possible retry-series rate without advancing ownership.
    ///
    /// Admission uses this before DMA publication so a later fallback cannot
    /// cross into a protection-required PHY after sequence/PN consumption.
    pub fn rate_after_failed_attempts(
        &self,
        failed_attempts: u8,
    ) -> Result<TxPhyRate, OrdinaryRetryError> {
        match self.rate_policy {
            OrdinaryRetryRatePolicy::Normal => select_ordinary_retry_rate(
                self.initial_rate,
                OrdinaryRetryCounters {
                    mpdu: failed_attempts,
                    short: failed_attempts,
                    long: 0,
                },
            ),
            OrdinaryRetryRatePolicy::P2p(schedule) => {
                select_p2p_retry_rate(schedule, failed_attempts)
            }
            OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule) => {
                if failed_attempts == 0 {
                    Ok(self.initial_rate)
                } else {
                    select_p2p_retry_rate(schedule, failed_attempts)
                }
            }
        }
    }

    /// Apply one typed completion after the raw status/detail dispatcher.
    pub fn observe_completion(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
        disposition: TxCompletionDisposition,
    ) -> OrdinaryRetryDecision {
        match disposition {
            TxCompletionDisposition::Success => {
                policy.record_success(self.queue);
                OrdinaryRetryDecision::Complete
            }
            TxCompletionDisposition::AckTimeout => self.observe_ack_timeout(policy),
            TxCompletionDisposition::CtsTimeout => self.observe_cts_timeout(policy),
            TxCompletionDisposition::Collision => self.observe_collision(policy),
            TxCompletionDisposition::Terminal(_) => {
                policy.reset_terminal_exchange(self.queue);
                OrdinaryRetryDecision::Complete
            }
        }
    }

    fn observe_ack_timeout(&mut self, policy: &mut WifiTxRuntimePolicy) -> OrdinaryRetryDecision {
        self.counters.mpdu = self.counters.mpdu.saturating_add(1);
        let class_limit_reached = match self.frame_class {
            OrdinaryFrameClass::Short => {
                self.counters.short = self.counters.short.saturating_add(1);
                self.counters.short >= VENDOR_SHORT_RETRY_LIMIT
            }
            OrdinaryFrameClass::Long => {
                self.counters.long = self.counters.long.saturating_add(1);
                self.counters.long >= VENDOR_LONG_RETRY_LIMIT
            }
        };
        self.finish_or_retry(
            policy,
            class_limit_reached || self.counters.mpdu >= self.mpdu_retry_limit,
            true,
        )
    }

    fn observe_cts_timeout(&mut self, policy: &mut WifiTxRuntimePolicy) -> OrdinaryRetryDecision {
        self.counters.short = self.counters.short.saturating_add(1);
        self.finish_or_retry(
            policy,
            self.counters.short >= VENDOR_SHORT_RETRY_LIMIT,
            false,
        )
    }

    /// Apply one detached ordinary-queue collision.
    pub fn observe_collision(&mut self, policy: &mut WifiTxRuntimePolicy) -> OrdinaryRetryDecision {
        let class_limit_reached = match self.frame_class {
            OrdinaryFrameClass::Short => {
                self.counters.short = self.counters.short.saturating_add(1);
                self.counters.short >= VENDOR_SHORT_RETRY_LIMIT
            }
            OrdinaryFrameClass::Long => {
                self.counters.long = self.counters.long.saturating_add(1);
                self.counters.long >= VENDOR_LONG_RETRY_LIMIT
            }
        };
        self.finish_or_retry(policy, class_limit_reached, false)
    }

    fn finish_or_retry(
        &mut self,
        policy: &mut WifiTxRuntimePolicy,
        limit_reached: bool,
        set_retry_bit: bool,
    ) -> OrdinaryRetryDecision {
        if limit_reached {
            policy.reset_terminal_exchange(self.queue);
            OrdinaryRetryDecision::Complete
        } else {
            policy.record_retry_failure(self.queue);
            self.publications = self.publications.saturating_add(1);
            OrdinaryRetryDecision::Retry { set_retry_bit }
        }
    }

    /// End ownership after a non-retryable executor or hardware error.
    pub fn abort(&self, policy: &mut WifiTxRuntimePolicy) {
        policy.reset_terminal_exchange(self.queue);
    }
}

/// Exact production rate-selection entry for the normal `rcGetRate` slice.
///
/// Keeping this as a named, non-inlined entry lets vendor comparison execute
/// the same compiled function used by [`OrdinaryMpduRetryState`] rather than
/// a shadow retry model.
#[inline(never)]
pub fn select_ordinary_retry_rate(
    initial_rate: TxPhyRate,
    counters: OrdinaryRetryCounters,
) -> Result<TxPhyRate, OrdinaryRetryError> {
    let retry_index = counters.mpdu.max(counters.short);
    match initial_rate {
        TxPhyRate::Legacy(rate) => rate
            .vendor_retry_rate(retry_index)
            .map(TxPhyRate::Legacy)
            .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index }),
        TxPhyRate::Ht(rate) => Ok(rate
            .vendor_retry_rate(retry_index)
            .unwrap_or(TxPhyRate::Ht(rate))),
        TxPhyRate::He(rate) => Ok(TxPhyRate::He(rate)),
    }
}

/// Select one attempt from an exact standard P2P record.
///
/// The dedicated P2P arenas never enter the proprietary LR or HE rate-code
/// domains. HT values are decoded as 20-MHz one-stream rates; MCS32 therefore
/// cannot be introduced through this path.
#[inline(never)]
pub fn select_p2p_retry_rate(
    schedule: P2pRetryRateSchedule,
    retry_index: u8,
) -> Result<TxPhyRate, OrdinaryRetryError> {
    let code = schedule_rate_after_failures(schedule.schedule(), retry_index)
        .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index })?;
    TxPhyRate::from_code(code, HtChannelWidth::Mhz20)
        .ok_or(OrdinaryRetryError::RetryRateUnavailable { retry_index })
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
    first_sequence: u16,
    /// Original aggregate indices still represented by the compacted
    /// descriptor chain. Hardware BlockAck windows are bounded to 32 MPDUs,
    /// so one mask preserves every non-contiguous sequence after compaction
    /// without carrying a movable 32-entry sequence table.
    pending_original_indices: u32,
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
        let pending_original_indices = if subframes == 32 {
            u32::MAX
        } else {
            (1_u32 << subframes) - 1
        };
        Ok(Self {
            first_sequence: first_sequence & IEEE80211_SEQUENCE_MASK,
            pending_original_indices,
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
        let mut retry_original_indices = 0_u32;
        let mut index = 0_usize;
        while index < observed_subframes as usize {
            let original_index = self.original_index(index as u8);
            let sequence = self.first_sequence.wrapping_add(u16::from(original_index))
                & IEEE80211_SEQUENCE_MASK;
            if completion.acknowledges(sequence) {
                self.acknowledged = self.acknowledged.saturating_add(1);
            } else {
                retry_mask |= 1_u32 << index;
                retry_original_indices |= 1_u32 << original_index;
            }
            index += 1;
        }
        let missing = retry_mask.count_ones() as u8;
        let retain = (missing >= 2 || (missing == 1 && self.policy.retain_single_mpdu))
            && self.aggregate_attempts < self.policy.attempt_limit;
        if !retain {
            return Ok(AmpduRetryDecision::Finish { retry_mask });
        }

        self.pending_original_indices = retry_original_indices;
        self.current_subframes = missing;
        self.aggregate_attempts = self.aggregate_attempts.saturating_add(1);
        Ok(AmpduRetryDecision::RetainAggregate { retry_mask })
    }

    pub const fn current_subframes(&self) -> u8 {
        self.current_subframes
    }

    pub const fn current_first_sequence(&self) -> u16 {
        self.first_sequence
            .wrapping_add(self.pending_original_indices.trailing_zeros() as u16)
            & IEEE80211_SEQUENCE_MASK
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

    fn original_index(&self, compacted_index: u8) -> u8 {
        let mut remaining = self.pending_original_indices;
        let mut position = compacted_index;
        loop {
            let original = remaining.trailing_zeros() as u8;
            if position == 0 {
                return original;
            }
            remaining &= remaining - 1;
            position -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        tx::{HtGuardInterval, HtMcs, HtRate, LegacyRate, TxCompletion, TxCookie},
        tx_ampdu::{HtAmpduTxCompletion, HtBlockAckRegisters, TxBlockAckBitmap},
    };
    use open_esp_radio_ieee80211::wmm::parse_wmm_parameter_element;

    const STANDARD_WMM: [u8; 26] = [
        221, 24, 0x00, 0x50, 0xf2, 0x02, 1, 1, 0x85, 0, 0x03, 0xa4, 0, 0, 0x27, 0xa4, 0, 0, 0x42,
        0x43, 94, 0, 0x72, 0x32, 47, 0,
    ];

    fn ipv4_with_dscp(dscp: u8) -> [u8; 16] {
        let mut frame = [0_u8; 16];
        frame[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
        frame[14] = 0x45;
        frame[15] = dscp << 2;
        frame
    }

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
    fn negotiated_acm_downgrades_tid_and_queue_without_claiming_admission() {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        policy
            .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
            .unwrap();

        let voice = policy
            .select_network_traffic(&ipv4_with_dscp(46), true)
            .unwrap();
        assert_eq!(voice.requested.access_category, WmmAccessCategory::Voice);
        assert_eq!(voice.access_category, WmmAccessCategory::Video);
        assert_eq!(voice.user_priority, WmmUserPriority::UP5);
        assert_eq!(voice.queue(), LegacyTxQueue::Video);
        assert_eq!(
            voice.admission,
            WmmAdmissionDisposition::Downgraded {
                requested: WmmAccessCategory::Voice
            }
        );
        assert_eq!(voice.txop_limit_units_32_us(), 94);

        let non_qos = policy
            .select_network_traffic(&ipv4_with_dscp(46), false)
            .unwrap();
        assert_eq!(non_qos.tid(), 0);
        assert_eq!(non_qos.queue(), LegacyTxQueue::BestEffort);
        assert_eq!(non_qos.admission, WmmAdmissionDisposition::NonQosBestEffort);
    }

    #[test]
    fn all_mandatory_access_categories_fail_closed() {
        let mut element = STANDARD_WMM;
        for offset in [10, 14, 18, 22] {
            element[offset] |= 0x10;
        }
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        policy
            .install_wmm(parse_wmm_parameter_element(&element).unwrap())
            .unwrap();
        assert_eq!(
            policy.select_network_traffic(&ipv4_with_dscp(0), true),
            Err(WifiTxTrafficError::AdmissionControlRequired {
                requested: WmmAccessCategory::BestEffort
            })
        );
    }

    #[test]
    fn negotiated_txop_is_bounded_and_medium_ownership_stays_unsupported() {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        policy
            .install_wmm(parse_wmm_parameter_element(&STANDARD_WMM).unwrap())
            .unwrap();
        let video = policy
            .select_network_traffic(&ipv4_with_dscp(40), true)
            .unwrap();
        assert_eq!(
            video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
            Ok(HeEdcaTxopLimit::from_units_32_us(94).unwrap())
        );
        assert_eq!(
            video.he_txop_limit(HeEdcaTxopLimit::from_units_32_us(47).unwrap()),
            Ok(HeEdcaTxopLimit::from_units_32_us(47).unwrap())
        );
        assert_eq!(
            video.require_ht_txop_support(),
            Err(WmmTxopUnsupported::HtAggregateDurationBudget { units_32_us: 94 })
        );
        assert_eq!(
            policy.request_hardware_medium(WmmHardwareMediumRequest::RtsCtsProtection),
            Err(WmmTxopUnsupported::RtsCtsProtection)
        );
        assert_eq!(
            policy.request_hardware_medium(WmmHardwareMediumRequest::MultiPpduTxop),
            Err(WmmTxopUnsupported::MultiPpduMediumOwnership)
        );

        let mut wide = STANDARD_WMM;
        wide[21] = 1;
        policy
            .install_wmm(parse_wmm_parameter_element(&wide).unwrap())
            .unwrap();
        let video = policy
            .select_network_traffic(&ipv4_with_dscp(40), true)
            .unwrap();
        assert_eq!(
            video.he_txop_limit(HeEdcaTxopLimit::DEFAULT),
            Err(WmmTxopUnsupported::AdvertisedLimitTooWide { units_32_us: 350 })
        );
    }

    #[test]
    fn ordinary_retry_owns_rate_ladder_and_edca_transitions() {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        let queue = LegacyTxQueue::BestEffort;
        let mut retry = OrdinaryMpduRetryState::new(
            queue,
            TxPhyRate::Legacy(LegacyRate::Ofdm54M),
            4,
            OrdinaryFrameClass::Short,
        )
        .unwrap();

        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
        );
        assert_eq!(
            retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
            OrdinaryRetryDecision::Retry {
                set_retry_bit: true
            }
        );
        assert_eq!(retry.publications(), 2);
        assert_eq!(
            retry.counters(),
            OrdinaryRetryCounters {
                mpdu: 1,
                short: 1,
                long: 0
            }
        );
        assert_eq!(policy.contention_exponent(queue), 5);
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm54M))
        );

        assert_eq!(
            retry.observe_completion(&mut policy, TxCompletionDisposition::CtsTimeout),
            OrdinaryRetryDecision::Retry {
                set_retry_bit: false
            }
        );
        assert_eq!(retry.publications(), 3);
        assert_eq!(
            retry.counters(),
            OrdinaryRetryCounters {
                mpdu: 1,
                short: 2,
                long: 0
            }
        );
        assert_eq!(policy.contention_exponent(queue), 6);
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm48M))
        );

        assert_eq!(
            retry.observe_completion(&mut policy, TxCompletionDisposition::Success),
            OrdinaryRetryDecision::Complete
        );
        assert_eq!(policy.contention_exponent(queue), 4);
    }

    #[test]
    fn ordinary_retry_limit_collision_and_abort_restore_the_minimum_cw() {
        let queue = LegacyTxQueue::Voice;
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        let mut retry = OrdinaryMpduRetryState::new(
            queue,
            TxPhyRate::Legacy(LegacyRate::Ofdm6M),
            2,
            OrdinaryFrameClass::Long,
        )
        .unwrap();
        assert_eq!(
            retry.observe_collision(&mut policy),
            OrdinaryRetryDecision::Retry {
                set_retry_bit: false
            }
        );
        assert_eq!(retry.publications(), 2);
        assert_eq!(retry.counters().long, 1);
        assert_eq!(retry.counters().mpdu, 0);
        assert_eq!(retry.counters().short, 0);
        assert_eq!(
            retry.current_rate(),
            Ok(TxPhyRate::Legacy(LegacyRate::Ofdm6M))
        );
        assert_eq!(policy.contention_exponent(queue), 3);
        assert_eq!(
            retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
            OrdinaryRetryDecision::Retry {
                set_retry_bit: true
            }
        );
        assert_eq!(retry.counters().mpdu, 1);
        assert_eq!(retry.counters().long, 2);

        retry.abort(&mut policy);
        assert_eq!(policy.contention_exponent(queue), 2);
        assert_eq!(
            OrdinaryMpduRetryState::new(
                queue,
                TxPhyRate::Legacy(LegacyRate::Ofdm6M),
                0,
                OrdinaryFrameClass::Short,
            )
            .err(),
            Some(OrdinaryRetryError::ZeroMpduRetryLimit)
        );
    }

    #[test]
    fn p2p_ht20_sgi_mcs0_through_mcs6_enter_same_mcs_lgi_retry_records() {
        for mcs_index in 0..=6 {
            let mcs = HtMcs::from_index(mcs_index).unwrap();
            let initial = TxPhyRate::Ht(HtRate::new(
                mcs,
                HtGuardInterval::Short400Ns,
                HtChannelWidth::Mhz20,
            ));
            let schedule = RateScheduleRef::new(
                RateScheduleKind::P2pDot11N,
                8_u8.checked_sub(mcs_index).unwrap(),
            )
            .unwrap();
            let schedule = P2pRetryRateSchedule::new(schedule).unwrap();
            let mut retry = OrdinaryMpduRetryState::new_with_rate_policy(
                LegacyTxQueue::Voice,
                initial,
                OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule),
                4,
                OrdinaryFrameClass::Short,
            )
            .unwrap();
            let mut policy = WifiTxRuntimePolicy::vendor_defaults();

            assert_eq!(retry.current_rate(), Ok(initial));
            assert_eq!(
                retry.observe_completion(&mut policy, TxCompletionDisposition::AckTimeout),
                OrdinaryRetryDecision::Retry {
                    set_retry_bit: true
                }
            );
            assert_eq!(
                retry.current_rate(),
                Ok(TxPhyRate::Ht(HtRate::new(
                    mcs,
                    HtGuardInterval::Long800Ns,
                    HtChannelWidth::Mhz20,
                )))
            );
        }
    }

    #[test]
    fn p2p_ht_sgi_bridge_rejects_a_different_mcs_retry_record() {
        let initial = TxPhyRate::Ht(HtRate::new(
            HtMcs::Mcs4,
            HtGuardInterval::Short400Ns,
            HtChannelWidth::Mhz20,
        ));
        let wrong = P2pRetryRateSchedule::new(
            RateScheduleRef::new(RateScheduleKind::P2pDot11N, 3).unwrap(),
        )
        .unwrap();
        assert_eq!(
            OrdinaryMpduRetryState::new_with_rate_policy(
                LegacyTxQueue::Voice,
                initial,
                OrdinaryRetryRatePolicy::P2pHtSgiFallback(wrong),
                4,
                OrdinaryFrameClass::Short,
            )
            .err(),
            Some(OrdinaryRetryError::P2pHtSgiFallbackMismatch {
                initial,
                scheduled: TxPhyRate::Ht(HtRate::new(
                    HtMcs::Mcs5,
                    HtGuardInterval::Long800Ns,
                    HtChannelWidth::Mhz20,
                )),
            })
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
    fn ack_timeout_completion_applies_a_received_block_ack_bitmap() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        assert_eq!(
            state.observe(completion(5, 20, u64::MAX), 2),
            Ok(AmpduRetryDecision::Finish { retry_mask: 0 })
        );
        assert_eq!(state.acknowledged(), 2);
    }

    #[test]
    fn missing_block_ack_result_ignores_a_stale_success_bitmap() {
        let mut state = AmpduRetryState::<4>::new(20, 2, HT_POLICY).unwrap();
        let mut stale = completion(5, 20, u64::MAX);
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
