//! Exact reviewed scheduler-item images for one Direct Test Mode event.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` and named same-chip
//! `r_ble_lll_dtm_sched_event` produce the positional transforms below before
//! entering the common scheduler. The model contains no list insertion,
//! hardware publication, retry loop or completion claim.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemReviewedWords;

use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmLinkStateReviewedWords;
use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
    BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmTxEventWindow, BluetoothSchedulerSoftwareConfig,
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

    pub(crate) const fn sequence_lead_raw_delta(self) -> u32 {
        self.sequence_lead_raw_delta
    }
}

/// Validated dynamic inputs to one DTM scheduler item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerItemEvent {
    frequency: u8,
    rate: u8,
    role: BluetoothDtmRole,
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
            BluetoothDtmRole::Transmitter,
            window.start().image(),
            window.end().image(),
        )
    }

    /// Convert typed HCI inputs into the reviewed positional field images.
    pub const fn new(
        channel: BluetoothDtmChannel,
        phy: BluetoothDtmPhy,
        role: BluetoothDtmRole,
        scheduler_start: u32,
        scheduler_end: u32,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        let rate = match phy.scheduler_rate_image(role) {
            Ok(rate) => rate,
            Err(_) => {
                return Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter);
            }
        };
        Ok(Self {
            frequency: channel.scheduler_frequency_image(),
            rate,
            role,
            scheduler_start,
            scheduler_end,
        })
    }

    /// Apply every complete reviewed transform preceding scheduler insertion.
    pub const fn apply(
        self,
        current: BluetoothDtmSchedulerItemReviewedWords,
        epoch: BluetoothControllerSchedulerEpoch,
    ) -> BluetoothDtmSchedulerItemReviewedWords {
        self.apply_raw_window(current, self.raw_start(epoch), self.raw_end(epoch))
    }

    pub(crate) const fn apply_raw_window(
        self,
        current: BluetoothDtmSchedulerItemReviewedWords,
        raw_start: u32,
        raw_end: u32,
    ) -> BluetoothDtmSchedulerItemReviewedWords {
        current.apply_event(self.frequency, self.rate, self.role, raw_start, raw_end)
    }

    /// Return the DTM role encoded by this validated scheduler item event.
    pub const fn role(self) -> BluetoothDtmRole {
        self.role
    }

    pub(crate) const fn raw_start(self, epoch: BluetoothControllerSchedulerEpoch) -> u32 {
        epoch.raw_time_for_scheduler_time(self.scheduler_start)
    }

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
        BluetoothDtmSchedulerItemReviewedWords, BluetoothDtmSchedulerTimingPolicy,
        apply_overlap_insertion_power,
    };
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRole, BluetoothSchedulerSoftwareConfig,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmLinkStateReviewedWords;

    const CURRENT: BluetoothDtmSchedulerItemReviewedWords =
        BluetoothDtmSchedulerItemReviewedWords {
            word_00: 0x12ff_5678,
            word_04: 0x0123_4567,
            word_08: 0xdead_beef,
            word_0c: 0x1357_9bdf,
            word_10: 0x2468_ace0,
            word_14: 0x0123_4567,
            word_18: 0xabcd_ef12,
            word_2c: 0x7654_3210,
            word_44: 0,
            word_48: 0,
            word_4c: 0x1234_56ff,
        };

    fn epoch() -> BluetoothControllerSchedulerEpoch {
        BluetoothControllerSchedulerEpoch::new(
            BluetoothControllerTimeSample::for_validation(100),
            1_000,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
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
    fn rx_event_matches_every_complete_scheduler_item_image() {
        let event = BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(21).expect("channel is in the DTM domain"),
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmRole::Receiver,
            1_012,
            1_020,
        )
        .expect("coded RX is accepted");
        let words = event.apply(CURRENT, epoch());

        assert_eq!(words.word_00, 0x12ef_5678);
        assert_eq!(words.word_04, 0x8123_4567);
        assert_eq!(words.word_08, 0xde0d_beef);
        assert_eq!(words.word_14, 0xf123_4567);
        assert_eq!(words.word_18, 0xabcd_aa13);
        assert_eq!(words.word_2c, 0x000f_0001);
        assert_eq!(words.word_44, 103);
        assert_eq!(words.word_48, 105);
        assert_eq!(words.word_4c, 0x1234_5600);
    }

    #[test]
    fn tx_event_preserves_rx_only_word_and_selects_tx_role_byte() {
        let event = BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(0).expect("channel zero is accepted"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRole::Transmitter,
            1_000,
            1_000,
        )
        .expect("1M TX is accepted");
        let words = event.apply(CURRENT, epoch());

        assert_eq!(words.word_00, 0x12bf_5678);
        assert_eq!(words.word_14, 0x0123_4567);
        assert_eq!(words.word_18, 0xabcd_8013);
        assert_eq!(words.word_2c, CURRENT.word_2c);
        assert_eq!(words.word_44, 100);
        assert_eq!(words.word_48, 100);
    }

    #[test]
    fn event_rejects_transmitter_only_phy_for_receiver_role() {
        assert_eq!(
            BluetoothDtmSchedulerItemEvent::new(
                BluetoothDtmChannel::new(39).expect("last channel is accepted"),
                BluetoothDtmPhy::LeCodedS2,
                BluetoothDtmRole::Receiver,
                0,
                0,
            ),
            Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter)
        );
    }

    #[test]
    fn overlap_insertion_projects_link_state_power_into_scheduler_item() {
        let link_state = BluetoothDtmLinkStateReviewedWords {
            word_00: 0,
            word_04: 0x0a80_0000,
            word_08: 0,
            word_14: 0,
            word_2c: 0,
            word_34: 0,
            word_38: 0,
            word_50: 0,
        };
        let projected = apply_overlap_insertion_power(CURRENT, link_state);

        assert_eq!(projected.word_14, 0x0153_4567);
        assert_eq!(projected.word_18, CURRENT.word_18);
    }

    #[test]
    fn transmitter_window_flows_into_both_epoch_projected_item_words() {
        use crate::{
            BluetoothDtmPayloadLength, BluetoothDtmSchedulerInstant, BluetoothDtmSchedulerMargin,
            BluetoothDtmTxTimingMicros,
        };

        let timing = BluetoothDtmTxTimingMicros::new(
            BluetoothDtmPayloadLength::from_hci_image(0),
            BluetoothDtmPhy::Le1M,
            0,
        )
        .scheduler_timing();
        let window = timing.initial_event_window(
            BluetoothDtmSchedulerInstant::from_image(1_000),
            BluetoothDtmSchedulerInstant::from_image(1_900),
            BluetoothDtmSchedulerMargin::from_image(20),
        );
        let event = BluetoothDtmSchedulerItemEvent::new_transmitter(
            BluetoothDtmChannel::new(0).expect("channel zero is accepted"),
            BluetoothDtmPhy::Le1M,
            window,
        )
        .expect("1M TX is accepted");
        let words = event.apply(CURRENT, epoch());

        assert_eq!(words.word_44, 335);
        assert_eq!(words.word_48, 874);
    }
}
