//! Shared owner for one ESP32-S31 ordinary legacy/HT/HE TX transaction.
//!
//! Protocol layers encode an MPDU while this owner is free, then hand it a
//! compact publication plan. Descriptor ownership, EDCA state, retry state,
//! calibrated power, entropy and executor deadlines remain here across both
//! the pre-connected control path and the connected data path.

use core::pin::Pin;

pub use crate::tx::{
    WifiTxEntropy, WifiTxPowerPair, WifiTxPowerProfile, WifiTxResources, WifiTxTimer,
};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    edca::EdcaContentionParameters,
    tx::{
        HeSmpduTxConfig, HtTxConfig, LegacyTxConfig, LegacyTxQueue, TxCompletion, TxCookie,
        TxError, TxHardware, TxPhyRate, TxSlot, TxSlotState,
    },
    tx_protection::{TxProtectionAdmissionError, TxProtectionReceiver},
    tx_runtime::{
        OrdinaryFrameClass, OrdinaryMpduRetryState, OrdinaryRetryCounters, OrdinaryRetryDecision,
        OrdinaryRetryError, OrdinaryRetryRatePolicy, VENDOR_RTS_THRESHOLD_BYTES,
        WifiTxRuntimePolicy,
    },
};
use open_esp_radio_wifi_softmac::{MacTxPlan, MacTxQueueState, MacTxResult, MacTxStatus};

use crate::tx::{WifiTxProgress, WifiTxWake};

pub const TX_METADATA_SIZE: usize = 8;
/// Hardware-appended CCMP MIC bytes accounted for by descriptor publication.
pub const TX_CCMP_MIC_SIZE: usize = 8;
pub const TX_FCS_SIZE: usize = 4;
const TX_ABORT_SETTLE_US: u64 = 16;
/// Metadata bit set by the complete HE S-MPDU preparation leaf before DMA
/// publication. It selects the single-MPDU container geometry while the low
/// twenty bits retain MPDU+MIC+FCS length.
const HE_SMPDU_METADATA_FLAG: u32 = 1 << 24;

/// ESP32-S31 MAC interface context selected for one ordinary TX queue.
///
/// This selector is independent from the hardware key-slot number. Vendor
/// `ppInstallKey` installs station keys in context zero and AP keys in context
/// one; the TX queue must select the same context for hardware CCMP to find
/// the installed key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxInterface {
    Station,
    AccessPoint,
}

impl OrdinaryTxInterface {
    const fn hardware_index(self) -> u8 {
        match self {
            Self::Station => 0,
            Self::AccessPoint => 1,
        }
    }
}

/// Compact hardware route retained across retries.
///
/// Queue and interface each fit in two bits. Keeping them in one byte avoids
/// growing every suspended TX/supervisor future merely to retain the AP/STA
/// selector until a retry publication.
#[derive(Clone, Copy)]
struct ActiveTxRoute(u8);

impl ActiveTxRoute {
    const fn new(queue: LegacyTxQueue, interface: OrdinaryTxInterface) -> Self {
        Self((queue as u8) | (interface.hardware_index() << 2))
    }

    const fn queue(self) -> LegacyTxQueue {
        match self.0 & 0x03 {
            0 => LegacyTxQueue::Voice,
            1 => LegacyTxQueue::Video,
            2 => LegacyTxQueue::BestEffort,
            _ => LegacyTxQueue::Background,
        }
    }

    const fn mac_interface(self) -> MacInterface {
        match (self.0 >> 2) & 0x03 {
            0 => MacInterface::Station,
            1 => MacInterface::AccessPoint,
            _ => panic!("corrupt active TX interface route"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxReport {
    /// Portable terminal exchange status consumed by HMAC policy.
    pub status: MacTxStatus<TxPhyRate>,
    /// Exact ESP32-S31 completion retained for low-level rate evidence.
    ///
    /// Timeout/collision terminal paths have no detached completion record.
    pub completion: Option<TxCompletion>,
    /// Re-publications performed while this transaction retained its MPDU.
    pub retries: OrdinaryTxRetryReport,
}

/// Exact causes of re-publication within one ordinary-MPDU transaction.
///
/// These counters exclude the initial publication and terminal failures that
/// are not re-published. Keeping the causes separate lets qualification
/// distinguish ordinary ACK failure, CTS failure and collision without
/// changing the recovered retry policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OrdinaryTxRetryReport {
    pub cts_timeouts: u8,
    pub ack_timeouts: u8,
    pub collisions: u8,
}

/// Read-only state of one active production ordinary-MPDU transaction.
///
/// This projection is intentionally narrower than [`OrdinaryTxOwner`]: it
/// exposes no descriptor, DMA buffer, PAC capability or mutation route. HIL
/// and compiled-vendor comparison use it to observe the retry state retained
/// by the real owner instead of maintaining a parallel verification model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxActiveSnapshot {
    pub counters: OrdinaryRetryCounters,
    pub publications: u8,
    pub current_rate: TxPhyRate,
    pub retry_bit_set: bool,
    pub retries: OrdinaryTxRetryReport,
}

impl OrdinaryTxRetryReport {
    pub const fn total(self) -> u8 {
        self.cts_timeouts
            .saturating_add(self.ack_timeouts)
            .saturating_add(self.collisions)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxOutcome {
    Success(OrdinaryTxReport),
    HardwareFailure(OrdinaryTxReport),
    HardwareTimeout(OrdinaryTxReport),
    CollisionLimit(OrdinaryTxReport),
}

impl OrdinaryTxOutcome {
    pub const fn report(self) -> OrdinaryTxReport {
        match self {
            Self::Success(report)
            | Self::HardwareFailure(report)
            | Self::HardwareTimeout(report)
            | Self::CollisionLimit(report) => report,
        }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxResetReason {
    CompletionInterruptWithoutState,
    TimeoutInterruptWithoutState,
    CollisionInterruptWithoutState,
    ConflictingInterruptEvents(u32),
    ExecutorDeadline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryTxError {
    Busy,
    BufferSizeOverflow,
    DeadlineOverflow,
    Tx(TxError),
    Retry(OrdinaryRetryError),
    Protection(TxProtectionAdmissionError),
    RadioResetRequired(TxResetReason),
}

impl From<TxError> for OrdinaryTxError {
    fn from(error: TxError) -> Self {
        Self::Tx(error)
    }
}

impl From<OrdinaryRetryError> for OrdinaryTxError {
    fn from(error: OrdinaryRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<TxProtectionAdmissionError> for OrdinaryTxError {
    fn from(error: TxProtectionAdmissionError) -> Self {
        Self::Protection(error)
    }
}

/// Everything needed to publish one already encoded MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxPlan {
    pub frame_length: usize,
    pub descriptor_capacity: Option<u32>,
    /// Portable exchange policy translated by this ESP32-S31 adapter.
    pub exchange: MacTxPlan<TxPhyRate>,
    pub hardware_mic_length: usize,
    pub hardware_key_selector: u8,
    pub interface: OrdinaryTxInterface,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

struct ActiveTx {
    cookie: TxCookie,
    retry: OrdinaryMpduRetryState,
    frame_length: usize,
    descriptor_capacity: u32,
    transfer_length: u32,
    route: ActiveTxRoute,
    hardware_mic_length: usize,
    hardware_key_selector: u8,
    scheduler_priority: u8,
    packet_priority: u8,
    group_receiver: bool,
    completion_timeout_us: u64,
    deadline_micros: u64,
    retries: OrdinaryTxRetryReport,
}

/// Unique ordinary-MPDU descriptor and retry owner shared by protocol phases.
pub struct OrdinaryTxOwner<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    policy: WifiTxRuntimePolicy,
    power: P,
    entropy: E,
    pub timer: T,
    active: Option<ActiveTx>,
    last_outcome: Option<OrdinaryTxOutcome>,
}

/// Opaque logical completion state detached from idle physical TX resources.
///
/// There is intentionally no public constructor: only a real terminal owner
/// can produce this token, so a role handoff cannot fabricate an outcome.
pub struct OrdinaryTxParked {
    last_outcome: Option<OrdinaryTxOutcome>,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: WifiTxTimer,
{
    pub fn new(resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>) -> Self {
        let WifiTxResources {
            slot,
            policy,
            power,
            entropy,
            timer,
        } = resources;
        Self {
            slot,
            policy,
            power,
            entropy,
            timer,
            active: None,
            last_outcome: None,
        }
    }

    /// Rejoin idle physical resources with the exact opaque role-local state
    /// emitted by [`Self::try_park`].
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: OrdinaryTxParked,
    ) -> Self {
        let mut owner = Self::new(resources);
        owner.last_outcome = parked.last_outcome;
        owner
    }

    pub const fn active(&self) -> bool {
        self.active.is_some()
    }

    /// Observe the bounded retry state retained by the active TX owner.
    ///
    /// `retry_bit_set` follows from a completed ACK-timeout re-publication:
    /// the owner increments that counter, writes the Retry bit and publishes
    /// again before restoring `active`. If either write or publication fails,
    /// no active snapshot is returned for that failed transition.
    pub fn active_snapshot(&self) -> Result<Option<OrdinaryTxActiveSnapshot>, OrdinaryRetryError> {
        self.active
            .as_ref()
            .map(|active| {
                Ok(OrdinaryTxActiveSnapshot {
                    counters: active.retry.counters(),
                    publications: active.retry.publications(),
                    current_rate: active.retry.current_rate()?,
                    retry_bit_set: active.retries.ack_timeouts != 0,
                    retries: active.retries,
                })
            })
            .transpose()
    }

    /// Exact descriptor lifecycle state retained for ownership diagnostics.
    pub fn slot_state(&self) -> TxSlotState {
        self.slot.as_ref().get_ref().state()
    }

    /// Current hardware-visible descriptor ownership word.
    pub fn descriptor_word0(&self) -> u32 {
        self.slot.as_ref().get_ref().descriptor_word0()
    }

    /// Portable distinction between normal queue pressure and a quarantined
    /// descriptor that requires a new radio epoch.
    pub fn queue_state(&self) -> MacTxQueueState {
        if self.slot.as_ref().get_ref().state() == TxSlotState::ResetRequired {
            MacTxQueueState::ResetRequired
        } else if self.active.is_some() || self.slot.as_ref().get_ref().state() != TxSlotState::Free
        {
            MacTxQueueState::Backpressured
        } else {
            MacTxQueueState::Ready
        }
    }

    pub const fn policy(&self) -> &WifiTxRuntimePolicy {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut WifiTxRuntimePolicy {
        &mut self.policy
    }

    pub const fn power(&self) -> &P {
        &self.power
    }

    /// Recover the phase-independent TX resources while DMA is idle.
    ///
    /// Returning `self` on failure preserves a live descriptor transaction;
    /// callers must drive it to a terminal outcome or reset the radio before
    /// attempting a station lifecycle transition again.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(self) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, _)| resources)
    }

    /// Detach an idle descriptor owner without discarding its terminal
    /// observation. Physical resources and logical callback state become two
    /// distinct, uniquely owned capabilities.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            OrdinaryTxParked,
        ),
        Self,
    > {
        if self.queue_state() != MacTxQueueState::Ready {
            return Err(self);
        }
        let Self {
            slot,
            policy,
            power,
            entropy,
            timer,
            active: _,
            last_outcome,
        } = self;
        Ok((
            WifiTxResources {
                slot,
                policy,
                power,
                entropy,
                timer,
            },
            OrdinaryTxParked { last_outcome },
        ))
    }

    pub fn contention_publication(
        &mut self,
        queue: LegacyTxQueue,
    ) -> (EdcaContentionParameters, u16) {
        let parameters = self.policy.contention_parameters(queue);
        let backoff = self.policy.select_backoff(queue, self.entropy.next_u32());
        (parameters, backoff)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.policy.record_retry_failure(queue);
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.policy.record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.policy.reset_terminal_exchange(queue);
    }

    pub fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
        self.timer.after_micros(micros)
    }

    pub fn now_micros(&self) -> u64 {
        self.timer.now_micros()
    }

    pub fn wait_until(&mut self, deadline_micros: u64) -> impl Future<Output = ()> + '_ {
        self.timer.wait_until(deadline_micros)
    }

    pub fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        let deadline = self
            .active
            .as_ref()
            .map(|active| active.deadline_micros)
            .unwrap_or_else(|| self.timer.now_micros());
        self.timer.wait_until(deadline)
    }

    pub fn take_last_outcome(&mut self) -> Option<OrdinaryTxOutcome> {
        self.last_outcome.take()
    }

    /// Borrow the terminal outcome without consuming another owner's public
    /// observation of it.
    pub const fn last_outcome(&self) -> Option<OrdinaryTxOutcome> {
        self.last_outcome
    }

    pub fn buffer_mut(&mut self) -> Result<&mut [u8; BUFFER_SIZE], OrdinaryTxError> {
        if self.active.is_some() {
            return Err(OrdinaryTxError::Busy);
        }
        self.slot.as_mut().buffer_mut().map_err(Into::into)
    }

    /// Preflight every rate which one upper-layer encode may publish before
    /// it consumes sequence or CCMP state. The common start edge repeats this
    /// check against its retained retry owner, so this is an early ownership
    /// boundary rather than a bypassable enforcement point.
    pub fn require_unprotected_retry_series(
        &self,
        initial_rate: TxPhyRate,
        retry_rate_policy: OrdinaryRetryRatePolicy,
        publication_limit: u8,
        group_receiver: bool,
    ) -> Result<(), OrdinaryTxError> {
        let retry = OrdinaryMpduRetryState::new_with_rate_policy(
            LegacyTxQueue::BestEffort,
            initial_rate,
            retry_rate_policy,
            publication_limit,
            OrdinaryFrameClass::Short,
        )?;
        self.require_unprotected_retry_state(&retry, publication_limit, group_receiver)
    }

    pub fn start<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.start_with_retry_rate_policy(hardware, plan, OrdinaryRetryRatePolicy::Normal)
    }

    /// Start one ordinary MPDU with an explicitly owned retry-rate policy.
    ///
    /// Protocols should use [`Self::start`] unless they have a distinct,
    /// reviewed rate arena such as the standard ESP-NOW P2P profile. DMA,
    /// EDCA and terminal ownership remain identical to the normal path.
    pub fn start_with_retry_rate_policy<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
        retry_rate_policy: OrdinaryRetryRatePolicy,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        if self.active.is_some() {
            return Err(OrdinaryTxError::Busy);
        }
        let hardware_frame_length = plan
            .frame_length
            .checked_add(plan.hardware_mic_length + TX_FCS_SIZE)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let transfer_length = TX_METADATA_SIZE
            .checked_add(hardware_frame_length)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let minimum_capacity = transfer_length
            .checked_add(3)
            .map(|length| length & !3)
            .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
        let descriptor_capacity = plan.descriptor_capacity.unwrap_or(
            u32::try_from(minimum_capacity).map_err(|_| OrdinaryTxError::BufferSizeOverflow)?,
        );
        if usize::try_from(descriptor_capacity).map_err(|_| OrdinaryTxError::BufferSizeOverflow)?
            < transfer_length
            || descriptor_capacity as usize > BUFFER_SIZE
        {
            return Err(OrdinaryTxError::BufferSizeOverflow);
        }
        let hardware_frame_length = u32::try_from(hardware_frame_length)
            .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
        let group_receiver = self.slot.as_mut().buffer_mut()?[TX_METADATA_SIZE + 4] & 1 != 0;
        let retry = OrdinaryMpduRetryState::new_with_rate_policy(
            LegacyTxQueue::from_access_category(plan.exchange.access_category),
            plan.exchange.initial_rate,
            retry_rate_policy,
            plan.exchange.publication_limit,
            if hardware_frame_length > VENDOR_RTS_THRESHOLD_BYTES {
                OrdinaryFrameClass::Long
            } else {
                OrdinaryFrameClass::Short
            },
        )?;
        self.require_unprotected_retry_state(
            &retry,
            plan.exchange.publication_limit,
            group_receiver,
        )?;
        {
            let buffer = self.slot.as_mut().buffer_mut()?;
            buffer[..4].copy_from_slice(&hardware_frame_length.to_le_bytes());
            buffer[4..TX_METADATA_SIZE].fill(0);
            buffer[TX_METADATA_SIZE + plan.frame_length
                ..TX_METADATA_SIZE + hardware_frame_length as usize]
                .fill(0);
        }

        let mut active = ActiveTx {
            cookie: TxCookie(0),
            retry,
            frame_length: plan.frame_length,
            descriptor_capacity,
            transfer_length: u32::try_from(transfer_length)
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?,
            route: ActiveTxRoute::new(
                LegacyTxQueue::from_access_category(plan.exchange.access_category),
                plan.interface,
            ),
            hardware_mic_length: plan.hardware_mic_length,
            hardware_key_selector: plan.hardware_key_selector,
            scheduler_priority: plan.scheduler_priority,
            packet_priority: plan.packet_priority,
            group_receiver,
            completion_timeout_us: plan.exchange.publication_timeout_micros,
            deadline_micros: 0,
            retries: OrdinaryTxRetryReport::default(),
        };
        self.publish_attempt(hardware, &mut active)?;
        self.last_outcome = None;
        self.active = Some(active);
        Ok(WifiTxProgress::Pending)
    }

    fn require_unprotected_retry_state(
        &self,
        retry: &OrdinaryMpduRetryState,
        publication_limit: u8,
        group_receiver: bool,
    ) -> Result<(), OrdinaryTxError> {
        let receiver = if group_receiver {
            TxProtectionReceiver::Group
        } else {
            TxProtectionReceiver::Individual
        };
        for failed_attempts in 0..publication_limit {
            let rate = retry.rate_after_failed_attempts(failed_attempts)?;
            self.policy
                .protection()
                .require_unprotected(rate, receiver, None)?;
        }
        Ok(())
    }

    /// Consume one IRQ/deadline edge and retain or release DMA ownership.
    pub async fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self.active.take().ok_or(OrdinaryTxError::Busy)?;
        let interrupt_events = match wake {
            WifiTxWake::Interrupt { events } => events,
            WifiTxWake::Deadline => 0,
        };
        let tx_events = interrupt_events
            & (open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_COMPLETE
                | open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_TX_TIMEOUT
                | open_esp_radio_esp32s31_wifi_mac::irq::MAC_INT_COLLISION);
        if tx_events.count_ones() > 1 {
            return self
                .reset_required(active, TxResetReason::ConflictingInterruptEvents(tx_events));
        }

        if let Some(completion) = self.slot.as_mut().acknowledge_completion(hardware)? {
            self.slot
                .as_mut()
                .detach_completed(hardware, active.cookie)?;
            return self.finish_completion(hardware, active, completion);
        }

        use open_esp_radio_esp32s31_wifi_mac::irq::{
            MAC_INT_COLLISION, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
        };
        if tx_events == MAC_INT_TX_COMPLETE {
            return self.reset_required(active, TxResetReason::CompletionInterruptWithoutState);
        }
        if tx_events == MAC_INT_TX_TIMEOUT || matches!(wake, WifiTxWake::Deadline) {
            if !self
                .slot
                .as_mut()
                .begin_timeout_abort(hardware, active.cookie)?
            {
                let reason = if matches!(wake, WifiTxWake::Deadline) {
                    TxResetReason::ExecutorDeadline
                } else {
                    TxResetReason::TimeoutInterruptWithoutState
                };
                return self.reset_required(active, reason);
            }
            self.timer.after_micros(TX_ABORT_SETTLE_US).await;
            self.slot
                .as_mut()
                .finish_timeout_abort(hardware, active.cookie)?;
            return self.finish_aborted_attempt(hardware, active, true);
        }
        if tx_events == MAC_INT_COLLISION {
            if !self
                .slot
                .as_mut()
                .abort_collision(hardware, active.cookie)?
            {
                return self.reset_required(active, TxResetReason::CollisionInterruptWithoutState);
            }
            return self.finish_aborted_attempt(hardware, active, false);
        }

        self.active = Some(active);
        Ok(WifiTxProgress::Pending)
    }

    /// Polling adapter used only before the production IRQ runner is active.
    ///
    /// It observes the same completion/timeout registers as the IRQ path and
    /// quarantines the descriptor when an executor deadline expires without a
    /// qualified hardware timeout edge.
    pub async fn service_polling<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        poll_interval_us: u64,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let active = self.active.take().ok_or(OrdinaryTxError::Busy)?;
        if let Some(completion) = self.slot.as_mut().acknowledge_completion(hardware)? {
            self.slot
                .as_mut()
                .detach_completed(hardware, active.cookie)?;
            return self.finish_completion(hardware, active, completion);
        }
        if self
            .slot
            .as_mut()
            .begin_timeout_abort(hardware, active.cookie)?
        {
            self.timer.after_micros(TX_ABORT_SETTLE_US).await;
            self.slot
                .as_mut()
                .finish_timeout_abort(hardware, active.cookie)?;
            return self.finish_aborted_attempt(hardware, active, true);
        }
        if self.timer.now_micros() >= active.deadline_micros {
            return self.reset_required(active, TxResetReason::ExecutorDeadline);
        }
        self.active = Some(active);
        self.timer.after_micros(poll_interval_us).await;
        Ok(WifiTxProgress::Pending)
    }

    fn finish_completion<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        mut active: ActiveTx,
        completion: TxCompletion,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let attempts = active.retry.publications();
        let final_rate = active.retry.current_rate()?;
        let disposition = completion.disposition();
        let decision = active
            .retry
            .observe_completion(&mut self.policy, disposition);
        match decision {
            OrdinaryRetryDecision::Complete => {
                let success = matches!(
                    disposition,
                    open_esp_radio_esp32s31_wifi_mac::tx::TxCompletionDisposition::Success
                );
                let report = OrdinaryTxReport {
                    status: MacTxStatus {
                        result: if success {
                            MacTxResult::Transmitted
                        } else {
                            MacTxResult::HardwareFailure(completion.status)
                        },
                        attempts,
                        final_rate,
                        acknowledged: (!active.group_receiver).then_some(success),
                        ack_snr_db: completion.ack_snr_sample(),
                        airtime_micros: None,
                    },
                    completion: Some(completion),
                    retries: active.retries,
                };
                self.last_outcome = Some(if success {
                    OrdinaryTxOutcome::Success(report)
                } else {
                    OrdinaryTxOutcome::HardwareFailure(report)
                });
                Ok(WifiTxProgress::Complete)
            }
            OrdinaryRetryDecision::Retry { set_retry_bit } => {
                use open_esp_radio_esp32s31_wifi_mac::tx::TxCompletionDisposition;
                match disposition {
                    TxCompletionDisposition::AckTimeout => {
                        active.retries.ack_timeouts = active.retries.ack_timeouts.saturating_add(1);
                    }
                    TxCompletionDisposition::CtsTimeout => {
                        active.retries.cts_timeouts = active.retries.cts_timeouts.saturating_add(1);
                    }
                    TxCompletionDisposition::Collision => {
                        active.retries.collisions = active.retries.collisions.saturating_add(1);
                    }
                    TxCompletionDisposition::Success | TxCompletionDisposition::Terminal(_) => {}
                }
                if set_retry_bit {
                    self.mark_retry_bit()?;
                }
                self.publish_attempt(hardware, &mut active)?;
                self.active = Some(active);
                Ok(WifiTxProgress::Pending)
            }
        }
    }

    fn finish_aborted_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        mut active: ActiveTx,
        timeout: bool,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        let attempts = active.retry.publications();
        let final_rate = active.retry.current_rate()?;
        if timeout {
            active.retry.abort(&mut self.policy);
            let report = OrdinaryTxReport {
                status: MacTxStatus {
                    result: MacTxResult::HardwareTimeout,
                    attempts,
                    final_rate,
                    acknowledged: (!active.group_receiver).then_some(false),
                    ack_snr_db: None,
                    airtime_micros: None,
                },
                completion: None,
                retries: active.retries,
            };
            self.last_outcome = Some(OrdinaryTxOutcome::HardwareTimeout(report));
            return Ok(WifiTxProgress::Complete);
        }

        let decision = active.retry.observe_collision(&mut self.policy);
        match decision {
            OrdinaryRetryDecision::Retry { set_retry_bit } => {
                debug_assert!(!set_retry_bit);
                active.retries.collisions = active.retries.collisions.saturating_add(1);
                self.publish_attempt(hardware, &mut active)?;
                self.active = Some(active);
                Ok(WifiTxProgress::Pending)
            }
            OrdinaryRetryDecision::Complete => {
                let report = OrdinaryTxReport {
                    status: MacTxStatus {
                        result: MacTxResult::CollisionLimit,
                        attempts,
                        final_rate,
                        acknowledged: None,
                        ack_snr_db: None,
                        airtime_micros: None,
                    },
                    completion: None,
                    retries: active.retries,
                };
                self.last_outcome = Some(OrdinaryTxOutcome::CollisionLimit(report));
                Ok(WifiTxProgress::Complete)
            }
        }
    }

    fn mark_retry_bit(&mut self) -> Result<(), OrdinaryTxError> {
        self.slot.as_mut().buffer_mut()?[TX_METADATA_SIZE + 1] |= 0x08;
        Ok(())
    }

    fn publish_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        active: &mut ActiveTx,
    ) -> Result<(), OrdinaryTxError> {
        let deadline_micros = self
            .timer
            .now_micros()
            .checked_add(active.completion_timeout_us)
            .ok_or(OrdinaryTxError::DeadlineOverflow)?;
        let rate = active.retry.current_rate()?;
        let metadata = self.slot.as_mut().buffer_mut()?;
        let mut metadata_word = u32::from_le_bytes(
            metadata[..4]
                .try_into()
                .expect("TX metadata word has a fixed four-byte prefix"),
        );
        if matches!(rate, TxPhyRate::He(_)) {
            metadata_word |= HE_SMPDU_METADATA_FLAG;
        } else {
            metadata_word &= !HE_SMPDU_METADATA_FLAG;
        }
        metadata[..4].copy_from_slice(&metadata_word.to_le_bytes());
        let cookie = self
            .slot
            .as_mut()
            .reserve(active.descriptor_capacity, active.transfer_length)?;
        let result = self.submit_reserved_attempt(hardware, cookie, active);
        if let Err(error) = result {
            self.slot.as_mut().cancel_reservation(cookie)?;
            return Err(error);
        }
        active.cookie = cookie;
        active.deadline_micros = deadline_micros;
        Ok(())
    }

    fn submit_reserved_attempt<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        cookie: TxCookie,
        active: &ActiveTx,
    ) -> Result<(), OrdinaryTxError> {
        let queue = active.route.queue();
        let rate = active.retry.current_rate()?;
        let contention = self.policy.contention_parameters(queue);
        let contention_window = self.policy.select_backoff(queue, self.entropy.next_u32());
        match rate {
            TxPhyRate::Legacy(rate) => {
                let signal = u16::try_from(
                    active
                        .frame_length
                        .checked_add(active.hardware_mic_length + TX_FCS_SIZE)
                        .ok_or(OrdinaryTxError::BufferSizeOverflow)?,
                )
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mut config = LegacyTxConfig::management_1m(signal);
                config.rate = rate;
                config.rts_rate = rate.vendor_rts_rate();
                let data_power = self.power.power_pair(rate.code());
                let rts_power = self.power.power_pair(config.rts_rate.code());
                config.data_power = data_power.primary as u8;
                config.rts_power_low = rts_power.primary as u8;
                config.rts_power_high = rts_power.alternate as u8;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = 1;
                config.group_receiver = active.group_receiver;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_legacy(hardware, cookie, queue, config)?;
            }
            TxPhyRate::Ht(rate) => {
                let frame_length = u16::try_from(active.frame_length)
                    .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mic_length = u8::try_from(active.hardware_mic_length)
                    .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mut config = HtTxConfig::single_mpdu(rate, frame_length, mic_length)
                    .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
                let data_power = self.power.power_pair(rate.power_lookup_code());
                let rts_rate = rate.vendor_rts_rate();
                let rts_power = self.power.power_pair(rts_rate.code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.protection_spacing = self.policy.ht_ampdu().protection_spacing();
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = 1;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_ht(hardware, cookie, queue, config)?;
            }
            TxPhyRate::He(rate) => {
                let mpdu_length = u16::try_from(
                    active
                        .frame_length
                        .checked_add(active.hardware_mic_length)
                        .ok_or(OrdinaryTxError::BufferSizeOverflow)?,
                )
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?;
                let mut config =
                    HeSmpduTxConfig::new(rate, self.policy.he_bss_color(), mpdu_length)
                        .ok_or(OrdinaryTxError::BufferSizeOverflow)?;
                let data_power = self.power.power_pair(rate.power_lookup_code());
                let rts_rate = rate.vendor_rts_rate();
                let rts_power = self.power.power_pair(rts_rate.code());
                config.data_power_primary = data_power.primary as u8;
                config.data_power_alternate = data_power.alternate as u8;
                config.rts_power_primary = rts_power.primary as u8;
                config.rts_power_alternate = rts_power.alternate as u8;
                config.aifsn = contention.aifsn();
                config.contention_window = contention_window;
                config.scheduler_priority = active.scheduler_priority;
                config.pti = active.packet_priority;
                config.pti_count = 1;
                config.hardware_key_selector = active.hardware_key_selector;
                config.interface = active.route.mac_interface();
                self.slot
                    .as_mut()
                    .submit_he_smpdu(hardware, cookie, queue, config)?;
            }
        }
        Ok(())
    }

    fn reset_required(
        &mut self,
        active: ActiveTx,
        reason: TxResetReason,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        self.slot.as_mut().require_reset(active.cookie)?;
        Err(OrdinaryTxError::RadioResetRequired(reason))
    }
}
