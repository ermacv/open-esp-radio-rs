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
