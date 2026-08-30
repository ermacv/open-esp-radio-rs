//! Exact reviewed scheduler-item images for one Direct Test Mode event.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` and named same-chip
//! `r_ble_lll_dtm_sched_event` produce the positional transforms below before
//! entering the common scheduler. The model contains no list insertion,
//! hardware publication, retry loop or completion claim.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemReviewedWords;

#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmLinkStateReviewedWords;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmReceiverEventPhase, BluetoothDtmSchedulerItemEventType,
};
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

#[cfg(any(target_arch = "riscv32", test))]
use crate::{BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample};
use crate::{
    BluetoothDtmChannel, BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmRxInitialEventWindow,
    BluetoothDtmRxRecurringEventWindow, BluetoothDtmTxEventWindow,
    BluetoothSchedulerSoftwareConfig,
};

/// Why one DTM scheduler-item event cannot be represented exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemEventError {
    /// HCI selector four is a transmitter-only DTM extension.
    LeCodedS2RequiresTransmitter,
}

/// Raw insertion timing policy derived from the common scheduler epoch.
///
/// Complete overlap admission converts the scheduler environment's first
/// policy delta through the live Controller time scale for its late-start
/// guard. `r_btdm_sched_calc_seq_time` converts the second delta before adding
/// it to every item. Construction requires both source-owned values, so the two
/// decisions cannot be drawn from different scheduler epochs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerTimingPolicy {
    late_start_guard_raw_delta: u32,
    sequence_lead_raw_delta: u32,
}

impl BluetoothDtmSchedulerTimingPolicy {
    /// Derive both raw timing deltas for one initialized scheduler epoch.
    pub const fn from_scheduler_config(
        config: BluetoothSchedulerSoftwareConfig,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            late_start_guard_raw_delta: scale
                .raw_delta_from_scheduler(config.late_start_guard_scheduler_delta())
                .whole,
            sequence_lead_raw_delta: scale
                .raw_delta_from_scheduler(config.sequence_lead_scheduler_delta())
                .whole,
        }
    }

    /// Whether one fresh sample still precedes the guarded item start.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn initial_deadline_is_open(
        self,
        sample: BluetoothControllerTimeSample,
        raw_item_start: u32,
    ) -> bool {
        (sample
            .raw_time()
            .wrapping_add(self.late_start_guard_raw_delta)
            .wrapping_sub(raw_item_start) as i32)
            < 0
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn sequence_lead_raw_delta(self) -> u32 {
        self.sequence_lead_raw_delta
    }
}

/// Validated dynamic inputs to one DTM scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerItemEvent {
    frequency: u8,
    rate: u8,
    event_type: BluetoothDtmSchedulerItemEventType,
    scheduler_start: u32,
    scheduler_end: u32,
}

impl BluetoothDtmSchedulerItemEvent {
    /// Bind a prepared transmitter window to the reviewed item transforms.
    pub const fn new_transmitter(
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        window: BluetoothDtmTxEventWindow,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        Self::new(
            channel,
            phy,
            BluetoothDtmSchedulerItemEventType::Transmitter,
            window.start().image(),
            window.end().image(),
        )
    }

    /// Bind the first receiver window to the full initial item transform.
    pub const fn new_initial_receiver(
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        window: BluetoothDtmRxInitialEventWindow,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        Self::new(
            channel,
            phy,
            BluetoothDtmSchedulerItemEventType::Receiver(BluetoothDtmReceiverEventPhase::Initial),
            window.start().image(),
            window.end().image(),
        )
    }

    /// Bind a recurring receiver window to the narrower reuse transform.
    pub const fn new_recurring_receiver(
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        window: BluetoothDtmRxRecurringEventWindow,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        Self::new(
            channel,
            phy,
            BluetoothDtmSchedulerItemEventType::Receiver(BluetoothDtmReceiverEventPhase::Recurring),
            window.start().image(),
            window.end().image(),
        )
    }

    /// Convert typed internal inputs into the reviewed positional field images.
    const fn new(
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        event_type: BluetoothDtmSchedulerItemEventType,
        scheduler_start: u32,
        scheduler_end: u32,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        let rate = match phy.scheduler_rate_image(event_type.role()) {
            Ok(rate) => rate,
            Err(_) => {
                return Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter);
            }
        };
        Ok(Self {
            frequency: channel.scheduler_frequency_image(),
            rate,
            event_type,
            scheduler_start,
            scheduler_end,
        })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn apply_raw_window(
        self,
        current: BluetoothDtmSchedulerItemReviewedWords,
        raw_start: u32,
        raw_end: u32,
    ) -> BluetoothDtmSchedulerItemReviewedWords {
        current.apply_event(
            self.frequency,
            self.rate,
            self.event_type,
            raw_start,
            raw_end,
        )
    }

    /// Return the DTM role encoded by this validated scheduler item event.
    pub const fn role(self) -> BluetoothDtmRole {
        self.event_type.role()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_start(self, epoch: BluetoothControllerSchedulerEpoch) -> u32 {
        epoch.raw_time_for_scheduler_time(self.scheduler_start)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_end(self, epoch: BluetoothControllerSchedulerEpoch) -> u32 {
        epoch.raw_time_for_scheduler_time(self.scheduler_end)
    }
}

/// Apply the common scheduler overlap-insertion power projection.
///
/// Complete current `r_sym_ble_iHRqSCIgChmgSHj5W8W3` and named same-chip
/// `r_sched_txn_rmOverlapInsert` copy the link-state five-bit rounded-power
/// image into scheduler-item bits 24:20 and clear the adjacent bits 27:25.
/// Both arguments remain CPU-owned controller-SRAM descriptor images; this
/// transform performs no MMIO and grants no publication ownership.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) const fn apply_overlap_insertion_power(
    scheduler_item: BluetoothDtmSchedulerItemReviewedWords,
    link_state: BluetoothDtmLinkStateReviewedWords,
) -> BluetoothDtmSchedulerItemReviewedWords {
    scheduler_item.apply_overlap_insertion_power(link_state)
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

    use super::{
        BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerItemEventError,
        BluetoothDtmSchedulerTimingPolicy,
    };
    use crate::{
        BluetoothControllerTimeSample, BluetoothDtmChannel, BluetoothDtmPhy, BluetoothDtmRole,
        BluetoothDtmRxInitialEventWindow, BluetoothDtmRxRecurringEventWindow,
        BluetoothDtmSchedulerInstant, BluetoothSchedulerSoftwareConfig,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmReceiverEventPhase, BluetoothDtmSchedulerItemEventType,
    };

    fn receiver_window() -> BluetoothDtmRxRecurringEventWindow {
        BluetoothDtmRxRecurringEventWindow::new(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothDtmSchedulerInstant::from_image(900),
            BluetoothDtmSchedulerInstant::from_image(1_020),
        )
    }

    #[test]
    fn insertion_policy_uses_one_initialized_scheduler_epoch_scale() {
        let policy = BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        );

        assert_eq!(policy.sequence_lead_raw_delta(), 11);
        assert!(
            policy.initial_deadline_is_open(BluetoothControllerTimeSample::for_validation(92), 103)
        );
        assert!(
            !policy
                .initial_deadline_is_open(BluetoothControllerTimeSample::for_validation(93), 103)
        );
    }

    #[test]
    fn recurring_receiver_window_retains_the_receiver_phase() {
        let event = BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
            BluetoothDtmChannel::new(21).expect("channel is in the DTM domain"),
            BluetoothDtmPhy::LeCoded,
            receiver_window(),
        )
        .expect("coded RX is accepted");

        assert_eq!(event.role(), BluetoothDtmRole::Receiver);
        assert_eq!(
            event.event_type,
            BluetoothDtmSchedulerItemEventType::Receiver(BluetoothDtmReceiverEventPhase::Recurring)
        );
    }

    #[test]
    fn initial_receiver_window_retains_the_initial_phase() {
        let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
            BluetoothDtmChannel::new(21).expect("channel is in the DTM domain"),
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmRxInitialEventWindow::new(
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothDtmSchedulerInstant::from_image(64),
                BluetoothDtmSchedulerInstant::from_image(1_020),
            ),
        )
        .expect("coded RX is accepted");

        assert_eq!(event.role(), BluetoothDtmRole::Receiver);
        assert_eq!(
            event.event_type,
            BluetoothDtmSchedulerItemEventType::Receiver(BluetoothDtmReceiverEventPhase::Initial)
        );
    }

    #[test]
    fn event_rejects_transmitter_only_phy_for_receiver_role() {
        assert_eq!(
            BluetoothDtmSchedulerItemEvent::new_recurring_receiver(
                BluetoothDtmChannel::new(39).expect("last channel is accepted"),
                BluetoothDtmPhy::LeCodedS2,
                receiver_window(),
            ),
            Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter)
        );
    }
}
