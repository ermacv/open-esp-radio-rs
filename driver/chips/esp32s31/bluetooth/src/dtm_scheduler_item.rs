//! Exact reviewed scheduler-item images for one Direct Test Mode event.
//!
//! Current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` and named same-chip
//! `r_ble_lll_dtm_sched_event` produce the positional transforms below before
//! entering the common scheduler. The model contains no list insertion,
//! hardware publication, retry loop or completion claim.

#![forbid(unsafe_code)]

pub use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmSchedulerItemReviewedWords;

#[cfg(any(target_arch = "riscv32", test))]
use crate::BluetoothControllerSchedulerEpoch;
use crate::{
    BluetoothDtmChannel, BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmRxInitialEventWindow,
    BluetoothDtmRxRecurringEventWindow, BluetoothDtmTxEventWindow,
};
#[cfg(any(target_arch = "riscv32", test))]
use open_esp_radio_esp32s31_bluetooth_memory::BluetoothDtmLinkStateReviewedWords;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothDtmReceiverEventPhase, BluetoothDtmSchedulerItemEventType,
};

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
    event_type: BluetoothDtmSchedulerItemEventType,
    start_micros: u32,
    end_micros: u32,
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
            BluetoothDtmSchedulerItemEventType::Transmitter(phy.scheduler_transmitter_phy()),
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
        let phy = match phy.scheduler_receiver_phy() {
            Ok(phy) => phy,
            Err(_) => {
                return Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter);
            }
        };
        Self::new(
            channel,
            BluetoothDtmSchedulerItemEventType::Receiver {
                phase: BluetoothDtmReceiverEventPhase::Initial,
                phy,
            },
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
        let phy = match phy.scheduler_receiver_phy() {
            Ok(phy) => phy,
            Err(_) => {
                return Err(BluetoothDtmSchedulerItemEventError::LeCodedS2RequiresTransmitter);
            }
        };
        Self::new(
            channel,
            BluetoothDtmSchedulerItemEventType::Receiver {
                phase: BluetoothDtmReceiverEventPhase::Recurring,
                phy,
            },
            window.start().image(),
            window.end().image(),
        )
    }

    /// Convert typed internal inputs into the reviewed positional field images.
    const fn new(
        channel: BluetoothDtmChannel,
        event_type: BluetoothDtmSchedulerItemEventType,
        start_micros: u32,
        end_micros: u32,
    ) -> Result<Self, BluetoothDtmSchedulerItemEventError> {
        Ok(Self {
            frequency: channel.scheduler_frequency_image(),
            event_type,
            start_micros,
            end_micros,
        })
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn apply_raw_window(
        self,
        current: BluetoothDtmSchedulerItemReviewedWords,
        raw_start: u32,
        raw_end: u32,
    ) -> BluetoothDtmSchedulerItemReviewedWords {
        current.apply_event(self.frequency, self.event_type, raw_start, raw_end)
    }

    /// Return the DTM role encoded by this validated scheduler item event.
    pub const fn role(self) -> BluetoothDtmRole {
        self.event_type.role()
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_start(self, epoch: BluetoothControllerSchedulerEpoch) -> u32 {
        epoch.raw_ticks_for_micros(self.start_micros)
    }

    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn raw_end(self, epoch: BluetoothControllerSchedulerEpoch) -> u32 {
        epoch.raw_ticks_for_micros(self.end_micros)
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
    use super::{BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerItemEventError};
    use crate::{
        BluetoothDtmChannel, BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmRxInitialEventWindow,
        BluetoothDtmRxRecurringEventWindow, BluetoothSchedulerInstant,
        BluetoothSchedulerSoftwareConfig,
    };
    use open_esp_radio_esp32s31_bluetooth_memory::{
        BluetoothDtmReceiverEventPhase, BluetoothDtmSchedulerItemEventType,
        BluetoothDtmSchedulerReceiverPhy,
    };

    fn receiver_window() -> BluetoothDtmRxRecurringEventWindow {
        BluetoothDtmRxRecurringEventWindow::new(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothSchedulerInstant::from_image(900),
            BluetoothSchedulerInstant::from_image(1_020),
        )
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
            BluetoothDtmSchedulerItemEventType::Receiver {
                phase: BluetoothDtmReceiverEventPhase::Recurring,
                phy: BluetoothDtmSchedulerReceiverPhy::LeCoded,
            }
        );
    }

    #[test]
    fn initial_receiver_window_retains_the_initial_phase() {
        let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
            BluetoothDtmChannel::new(21).expect("channel is in the DTM domain"),
            BluetoothDtmPhy::LeCoded,
            BluetoothDtmRxInitialEventWindow::new(
                BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
                BluetoothSchedulerInstant::from_image(64),
                BluetoothSchedulerInstant::from_image(1_020),
            ),
        )
        .expect("coded RX is accepted");

        assert_eq!(event.role(), BluetoothDtmRole::Receiver);
        assert_eq!(
            event.event_type,
            BluetoothDtmSchedulerItemEventType::Receiver {
                phase: BluetoothDtmReceiverEventPhase::Initial,
                phy: BluetoothDtmSchedulerReceiverPhy::LeCoded,
            }
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
