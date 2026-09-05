use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::BluetoothDtmSchedulerReservation;
use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
    BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmRxInitialEventWindow,
    BluetoothDtmSchedulerItemEvent, BluetoothSchedulerInstant, BluetoothSchedulerSoftwareConfig,
    scheduler::timeline::{BluetoothSchedulerTimeline, BluetoothSchedulerTimingPolicy},
};

fn sample(raw_time: u32) -> BluetoothControllerTimeSample {
    BluetoothControllerTimeSample::for_validation(raw_time)
}

#[test]
fn dtm_envelope_retains_the_event_and_epoch_outside_the_common_timeline() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    let event = BluetoothDtmSchedulerItemEvent::new_initial_receiver(
        BluetoothDtmChannel::new(5).expect("channel is valid"),
        BluetoothDtmPhy::Le1M,
        BluetoothDtmRxInitialEventWindow::new(
            BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
            BluetoothSchedulerInstant::from_image(900),
            BluetoothSchedulerInstant::from_image(1_020),
        ),
    )
    .expect("initial receiver event is valid");
    let policy = BluetoothSchedulerTimingPolicy::from_scheduler_config(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        scale,
    );
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            policy,
            sample(92),
        )
        .expect("the projected event passes its initial deadline");
    let reservation = BluetoothDtmSchedulerReservation::new(window, event, epoch);

    assert_eq!(reservation.window().start(), 310);
    assert_eq!(reservation.window().end(), 586);
    assert_eq!(reservation.event().role(), BluetoothDtmRole::Receiver);
    assert_eq!(reservation.epoch(), epoch);
    assert!(timeline.release(reservation.into_window()).is_ok());
}
