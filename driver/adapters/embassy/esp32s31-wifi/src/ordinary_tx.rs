//! Shared owner for one ESP32-S31 ordinary legacy/HT TX transaction.
//!
//! Protocol layers encode an MPDU while this owner is free, then hand it a
//! compact publication plan. Descriptor ownership, EDCA state, retry state,
//! calibrated power, entropy and executor deadlines remain here across both
//! the pre-connected control path and the connected data path.

use core::pin::Pin;

use open_esp_radio_esp32s31_wifi_mac::{
    edca::EdcaContentionParameters,
    tx::{
        HtTxConfig, LegacyTxConfig, LegacyTxQueue, TxCompletion, TxCookie, TxError, TxHardware,
        TxPhyRate, TxSlot, TxSlotState,
    },
    tx_runtime::{StaTxRuntimePolicy, UnicastRetryDecision, UnicastRetryError, UnicastRetryState},
};
pub use open_esp_radio_esp32s31_wifi_sta::tx::{
    WifiTxEntropy, WifiTxPowerPair, WifiTxPowerProfile, WifiTxResources, WifiTxTimer,
};
use open_esp_radio_wifi_softmac::{MacTxPlan, MacTxQueueState, MacTxResult, MacTxStatus};

use crate::connected_runner::{WifiTxProgress, WifiTxWake};

pub(crate) const TX_METADATA_SIZE: usize = 8;
pub(crate) const TX_CCMP_MIC_SIZE: usize = 8;
pub(crate) const TX_FCS_SIZE: usize = 4;
const TX_ABORT_SETTLE_US: u64 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrdinaryTxReport {
    /// Portable terminal exchange status consumed by HMAC policy.
    pub status: MacTxStatus<TxPhyRate>,
    /// Exact ESP32-S31 completion retained for low-level rate evidence.
    ///
    /// Timeout/collision terminal paths have no detached completion record.
    pub completion: Option<TxCompletion>,
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
    UnsupportedHeOrdinaryMpdu,
    BufferSizeOverflow,
    DeadlineOverflow,
    Tx(TxError),
    Retry(UnicastRetryError),
    RadioResetRequired(TxResetReason),
}

impl From<TxError> for OrdinaryTxError {
    fn from(error: TxError) -> Self {
        Self::Tx(error)
    }
}

impl From<UnicastRetryError> for OrdinaryTxError {
    fn from(error: UnicastRetryError) -> Self {
        Self::Retry(error)
    }
}

/// Everything needed to publish one already encoded MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OrdinaryTxPlan {
    pub frame_length: usize,
    pub descriptor_capacity: Option<u32>,
    /// Portable exchange policy translated by this ESP32-S31 adapter.
    pub exchange: MacTxPlan<TxPhyRate>,
    pub hardware_mic_length: usize,
    pub hardware_key_selector: u8,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

struct ActiveTx {
    cookie: TxCookie,
    retry: UnicastRetryState,
    frame_length: usize,
    descriptor_capacity: u32,
    transfer_length: u32,
    queue: LegacyTxQueue,
    hardware_mic_length: usize,
    hardware_key_selector: u8,
    scheduler_priority: u8,
    packet_priority: u8,
    group_receiver: bool,
    completion_timeout_us: u64,
    deadline_micros: u64,
}

/// Unique ordinary-MPDU descriptor and retry owner shared by protocol phases.
pub(crate) struct OrdinaryTxOwner<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub(crate) slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    policy: StaTxRuntimePolicy,
    power: P,
    entropy: E,
    pub(crate) timer: T,
    active: Option<ActiveTx>,
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

    pub const fn active(&self) -> bool {
        self.active.is_some()
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

    pub const fn policy(&self) -> &StaTxRuntimePolicy {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut StaTxRuntimePolicy {
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
    pub(crate) fn try_into_resources(
        self,
    ) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        if self.active() {
            return Err(self);
        }
        let Self {
            slot,
            policy,
            power,
            entropy,
            timer,
            active: _,
            last_outcome: _,
        } = self;
        Ok(WifiTxResources {
            slot,
            policy,
            power,
            entropy,
            timer,
        })
    }

    pub(crate) fn contention_publication(
        &mut self,
        queue: LegacyTxQueue,
    ) -> (EdcaContentionParameters, u16) {
        let parameters = self.policy.contention_parameters(queue);
        let backoff = self.policy.select_backoff(queue, self.entropy.next_u32());
        (parameters, backoff)
    }

    pub(crate) fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.policy.record_retry_failure(queue);
    }

    pub(crate) fn record_success(&mut self, queue: LegacyTxQueue) {
        self.policy.record_success(queue);
    }

    pub(crate) fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.policy.reset_terminal_exchange(queue);
    }

    pub(crate) fn after_micros(&mut self, micros: u64) -> impl Future<Output = ()> + '_ {
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

    pub fn start<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        plan: OrdinaryTxPlan,
    ) -> Result<WifiTxProgress, OrdinaryTxError> {
        if self.active.is_some() {
            return Err(OrdinaryTxError::Busy);
        }
        if matches!(plan.exchange.initial_rate, TxPhyRate::He(_)) {
            return Err(OrdinaryTxError::UnsupportedHeOrdinaryMpdu);
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
        let group_receiver = {
            let buffer = self.slot.as_mut().buffer_mut()?;
            buffer[..4].copy_from_slice(&hardware_frame_length.to_le_bytes());
            buffer[4..TX_METADATA_SIZE].fill(0);
            buffer[TX_METADATA_SIZE + plan.frame_length
                ..TX_METADATA_SIZE + hardware_frame_length as usize]
                .fill(0);
            buffer[TX_METADATA_SIZE + 4] & 1 != 0
        };

        let mut active = ActiveTx {
            cookie: TxCookie(0),
            retry: UnicastRetryState::new(
                LegacyTxQueue::from_access_category(plan.exchange.access_category),
                plan.exchange.initial_rate,
                plan.exchange.publication_limit,
            )?,
            frame_length: plan.frame_length,
            descriptor_capacity,
            transfer_length: u32::try_from(transfer_length)
                .map_err(|_| OrdinaryTxError::BufferSizeOverflow)?,
            queue: LegacyTxQueue::from_access_category(plan.exchange.access_category),
            hardware_mic_length: plan.hardware_mic_length,
            hardware_key_selector: plan.hardware_key_selector,
            scheduler_priority: plan.scheduler_priority,
            packet_priority: plan.packet_priority,
            group_receiver,
            completion_timeout_us: plan.exchange.publication_timeout_micros,
            deadline_micros: 0,
        };
        self.publish_attempt(hardware, &mut active)?;
        self.last_outcome = None;
        self.active = Some(active);
        Ok(WifiTxProgress::Pending)
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
        let attempts = active.retry.attempt();
        let final_rate = active.retry.current_rate()?;
        match active
            .retry
            .observe_completion(&mut self.policy, completion.status)
        {
            UnicastRetryDecision::Complete => {
                let success = completion.status == 0;
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
                };
                self.last_outcome = Some(if success {
                    OrdinaryTxOutcome::Success(report)
                } else {
                    OrdinaryTxOutcome::HardwareFailure(report)
                });
                Ok(WifiTxProgress::Complete)
            }
            UnicastRetryDecision::Retry => {
                self.mark_retry_bit()?;
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
        let attempts = active.retry.attempt();
        let final_rate = active.retry.current_rate()?;
        let decision = if timeout {
            active.retry.observe_hardware_timeout(&mut self.policy)
        } else {
            active.retry.observe_collision(&mut self.policy)
        };
        match decision {
            UnicastRetryDecision::Retry => {
                if timeout {
                    self.mark_retry_bit()?;
                }
                self.publish_attempt(hardware, &mut active)?;
                self.active = Some(active);
                Ok(WifiTxProgress::Pending)
            }
            UnicastRetryDecision::Complete => {
                let report = OrdinaryTxReport {
                    status: MacTxStatus {
                        result: if timeout {
                            MacTxResult::HardwareTimeout
                        } else {
                            MacTxResult::CollisionLimit
                        },
                        attempts,
                        final_rate,
                        acknowledged: if timeout && !active.group_receiver {
                            Some(false)
                        } else {
                            None
                        },
                        ack_snr_db: None,
                        airtime_micros: None,
                    },
                    completion: None,
                };
                self.last_outcome = Some(if timeout {
                    OrdinaryTxOutcome::HardwareTimeout(report)
                } else {
                    OrdinaryTxOutcome::CollisionLimit(report)
                });
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
        let queue = active.queue;
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
                self.slot
                    .as_mut()
                    .submit_ht(hardware, cookie, queue, config)?;
            }
            TxPhyRate::He(_) => return Err(OrdinaryTxError::UnsupportedHeOrdinaryMpdu),
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
