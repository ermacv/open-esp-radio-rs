use crate::{
    DtmRxInitialEventWindow, DtmRxRecurringEventWindow, SchedulerInstant,
    le::dtm::{DtmChannel, DtmPhy, DtmRole},
    scheduler::SchedulerSoftwareConfig,
};

use oer_esp32s31_bluetooth_memory::{
    DtmReceiverEventPhase, DtmSchedulerItemEventType, DtmSchedulerReceiverPhy,
};

use super::{DtmSchedulerItemEvent, DtmSchedulerItemEventError};

fn receiver_window() -> DtmRxRecurringEventWindow {
    DtmRxRecurringEventWindow::new(
        SchedulerSoftwareConfig::reviewed_standalone(),
        SchedulerInstant::from_image(900),
        SchedulerInstant::from_image(1_020),
    )
}

#[test]
fn recurring_receiver_window_retains_the_receiver_phase() {
    let event = DtmSchedulerItemEvent::new_recurring_receiver(
        DtmChannel::new(21).expect("channel is in the DTM domain"),
        DtmPhy::LeCoded,
        receiver_window(),
    )
    .expect("coded RX is accepted");

    assert_eq!(event.role(), DtmRole::Receiver);
    assert_eq!(
        event.event_type,
        DtmSchedulerItemEventType::Receiver {
            phase: DtmReceiverEventPhase::Recurring,
            phy: DtmSchedulerReceiverPhy::LeCoded,
        }
    );
}

#[test]
fn initial_receiver_window_retains_the_initial_phase() {
    let event = DtmSchedulerItemEvent::new_initial_receiver(
        DtmChannel::new(21).expect("channel is in the DTM domain"),
        DtmPhy::LeCoded,
        DtmRxInitialEventWindow::new(
            SchedulerSoftwareConfig::reviewed_standalone(),
            SchedulerInstant::from_image(64),
            SchedulerInstant::from_image(1_020),
        ),
    )
    .expect("coded RX is accepted");

    assert_eq!(event.role(), DtmRole::Receiver);
    assert_eq!(
        event.event_type,
        DtmSchedulerItemEventType::Receiver {
            phase: DtmReceiverEventPhase::Initial,
            phy: DtmSchedulerReceiverPhy::LeCoded,
        }
    );
}

#[test]
fn event_rejects_transmitter_only_phy_for_receiver_role() {
    assert_eq!(
        DtmSchedulerItemEvent::new_recurring_receiver(
            DtmChannel::new(39).expect("last channel is accepted"),
            DtmPhy::LeCodedS2,
            receiver_window(),
        ),
        Err(DtmSchedulerItemEventError::LeCodedS2RequiresTransmitter)
    );
}
