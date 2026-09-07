use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample, DtmRxInitialEventWindow, SchedulerInstant,
    le::dtm::{DtmChannel, DtmPhy, DtmRole, DtmSchedulerItemEvent},
    scheduler::{
        SchedulerSoftwareConfig,
        timeline::{SchedulerTimeline, SchedulerTimingPolicy},
    },
};

use oer_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::DtmSchedulerReservation;

fn sample(raw_time: u32) -> ControllerTimeSample {
    ControllerTimeSample::for_validation(raw_time)
}

#[test]
fn dtm_envelope_retains_the_event_and_epoch_outside_the_common_timeline() {
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let epoch = ControllerSchedulerEpoch::new(sample(100), 1_000, scale);
    let event = DtmSchedulerItemEvent::new_initial_receiver(
        DtmChannel::new(5).expect("channel is valid"),
        DtmPhy::Le1M,
        DtmRxInitialEventWindow::new(
            SchedulerSoftwareConfig::reviewed_standalone(),
            SchedulerInstant::from_image(900),
            SchedulerInstant::from_image(1_020),
        ),
    )
    .expect("initial receiver event is valid");
    let policy = SchedulerTimingPolicy::from_scheduler_config(
        SchedulerSoftwareConfig::reviewed_standalone(),
        scale,
    );
    let mut timeline = SchedulerTimeline::<1>::new();
    let window = timeline
        .reserve_initial_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            policy,
            sample(92),
        )
        .expect("the projected event passes its initial deadline");
    let reservation = DtmSchedulerReservation::new(window, event, epoch);

    assert_eq!(reservation.window().start(), 310);
    assert_eq!(reservation.window().end(), 586);
    assert_eq!(reservation.event().role(), DtmRole::Receiver);
    assert_eq!(reservation.epoch(), epoch);
    assert!(timeline.release(reservation.into_window()).is_ok());
}
