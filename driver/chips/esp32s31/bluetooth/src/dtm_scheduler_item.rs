//! Exact reviewed scheduler-item images for one Direct Test Mode event.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` and named same-chip
//! `r_ble_lll_dtm_sched_event` produce the positional transforms below before
//! entering the common scheduler. The model contains no list insertion,
//! hardware publication, retry loop or completion claim.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothDtmChannel, BluetoothDtmPhy, BluetoothDtmRole,
    BluetoothDtmTxEventWindow,
};

const FREQUENCY_MASK: u32 = 0x0000_7f00;
const RATE_LANES_MASK: u32 = 0xf000_0000;

/// Why one DTM scheduler-item event cannot be represented exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmSchedulerItemEventError {
    /// HCI selector four is a transmitter-only DTM extension.
    LeCodedS2RequiresTransmitter,
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
        let current_role_byte = ((current.word_00 >> 16) & 0xff) as u8;
        let role_byte = (current_role_byte & 0xaf)
            | match self.role {
                BluetoothDtmRole::Transmitter => 0x10,
                BluetoothDtmRole::Receiver => 0x40,
            };
        let rate = self.rate as u32;

        BluetoothDtmSchedulerItemReviewedWords {
            word_00: (current.word_00 & 0xff00_ffff) | ((role_byte as u32) << 16),
            word_04: current.word_04 | 0x8000_0000,
            word_08: current.word_08 & 0xff0f_ffff,
            word_14: (current.word_14 & !RATE_LANES_MASK) | (rate << 28) | (rate << 30),
            word_18: (current.word_18 & !(FREQUENCY_MASK | 0x0f))
                | ((self.frequency as u32) << 8)
                | 0x03,
            word_2c: match self.role {
                BluetoothDtmRole::Transmitter => current.word_2c,
                BluetoothDtmRole::Receiver => 0x000f_0001,
            },
            word_44: epoch.raw_time_for_scheduler_time(self.scheduler_start),
            word_48: epoch.raw_time_for_scheduler_time(self.scheduler_end),
            word_4c: current.word_4c & 0xffff_ff00,
        }
    }
}

/// The nine scheduler-item words whose DTM event transform is complete.
///
/// Names are byte offsets. This is not the complete scheduler object and has
/// no API for list linkage, controller publication or ownership transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmSchedulerItemReviewedWords {
    /// Complete word at byte offset `+0x00`; only byte `+0x02` is transformed.
    pub word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub word_04: u32,
    /// Complete word at byte offset `+0x08`.
    pub word_08: u32,
    /// Complete word at byte offset `+0x14`.
    pub word_14: u32,
    /// Complete word at byte offset `+0x18`.
    pub word_18: u32,
    /// Complete word at byte offset `+0x2c`.
    pub word_2c: u32,
    /// Complete raw-time word at byte offset `+0x44`.
    pub word_44: u32,
    /// Complete raw-time word at byte offset `+0x48`.
    pub word_48: u32,
    /// Complete word at byte offset `+0x4c`; only its low byte is cleared.
    pub word_4c: u32,
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerHalInitConfig, BluetoothControllerLatchedTime,
        BluetoothControllerTimeLatchObservation, BluetoothControllerTimeLatchRequest,
    };

    use super::{
        BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerItemEventError,
        BluetoothDtmSchedulerItemReviewedWords,
    };
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeLatchProgress,
        BluetoothControllerTimeLatchPublication, BluetoothDtmChannel, BluetoothDtmPhy,
        BluetoothDtmRole,
    };

    const CURRENT: BluetoothDtmSchedulerItemReviewedWords =
        BluetoothDtmSchedulerItemReviewedWords {
            word_00: 0x12ff_5678,
            word_04: 0x0123_4567,
            word_08: 0xdead_beef,
            word_14: 0x0123_4567,
            word_18: 0xabcd_ef12,
            word_2c: 0x7654_3210,
            word_44: 0,
            word_48: 0,
            word_4c: 0x1234_56ff,
        };

    fn epoch() -> BluetoothControllerSchedulerEpoch {
        let ready = match BluetoothControllerTimeLatchPublication::new(
            BluetoothControllerTimeLatchRequest::new(),
        )
        .published()
        .observe(BluetoothControllerTimeLatchObservation::from_control_bits(
            0,
        )) {
            BluetoothControllerTimeLatchProgress::Ready(ready) => ready,
            BluetoothControllerTimeLatchProgress::Waiting(_) => panic!("clear latch stalled"),
        };
        BluetoothControllerSchedulerEpoch::new(
            ready.complete(BluetoothControllerLatchedTime::from_bits(100)),
            1_000,
            BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale(),
        )
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
