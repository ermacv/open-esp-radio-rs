//! Role scheduler scenarios over the validation hardware runtime.
#[allow(unused_imports)]
use crate::le::dtm::scheduler::lifecycle::DtmScheduling;

use crate::{
    ControllerSchedulerEpoch, ControllerTimeSample, DtmRxInitialEventWindow,
    DtmRxRecurringEventWindow, SchedulerInstant,
    clock::ClockedResources,
    controller_time::ControllerSchedulerNow,
    le::dtm::{DtmChannel, DtmPhy, DtmSchedulerItemEvent},
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use crate::le::dtm::scheduler::DtmControllerEventPreparationError;

#[test]
fn rejected_initial_sequence_gate_releases_the_controller_owned_reservation() {
    struct AdmissionPlatform;

    let stopped = BluetoothStopped::from_hardware(
        AdmissionPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let event = DtmSchedulerItemEvent::new_initial_receiver(
        DtmChannel::new(5).expect("channel five is valid"),
        DtmPhy::Le1M,
        DtmRxInitialEventWindow::new(
            crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
            SchedulerInstant::from_image(900),
            SchedulerInstant::from_image(1_020),
        ),
    )
    .expect("initial receiver event is role-valid");
    let time_scale = scheduler.controller_time_scale();
    let now = ControllerSchedulerNow::from_retained_epoch(
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 1_000, time_scale),
        ControllerTimeSample::for_validation(100),
    );
    assert_eq!(
        crate::le::dtm::scheduler::lifecycle::dtm_scheduler_current(&now),
        SchedulerInstant::from_image(1_000)
    );

    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let reservation = task
        .admit_initial_dtm_event(event, &now, ControllerTimeSample::for_validation(92))
        .expect("the fresh admission sample keeps the initial deadline open");
    let result = task.finish_dtm_sequence_authorization(
        reservation.authorize_sequence(ControllerTimeSample::for_validation(2_000)),
    );

    assert_eq!(
        result.expect_err("the deliberately late second sample must fail"),
        DtmControllerEventPreparationError::SequenceAuthorization(
            crate::scheduler::SchedulerSequenceAuthorizationError::DeadlineExpired,
        )
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}

#[test]
fn rejected_recurring_sequence_gate_releases_the_controller_owned_reservation() {
    struct RecurringPlatform;

    let stopped = BluetoothStopped::from_hardware(
        RecurringPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let event = DtmSchedulerItemEvent::new_recurring_receiver(
        DtmChannel::new(5).expect("channel five is valid"),
        DtmPhy::Le1M,
        DtmRxRecurringEventWindow::new(
            crate::scheduler::SchedulerSoftwareConfig::reviewed_standalone(),
            SchedulerInstant::from_image(900),
            SchedulerInstant::from_image(1_020),
        ),
    )
    .expect("receiver event is role-valid");
    let time_scale = scheduler.controller_time_scale();
    let now = ControllerSchedulerNow::from_retained_epoch(
        ControllerSchedulerEpoch::new(ControllerTimeSample::for_validation(100), 1_000, time_scale),
        ControllerTimeSample::for_validation(100),
    );
    assert_eq!(
        crate::le::dtm::scheduler::lifecycle::dtm_scheduler_current(&now),
        SchedulerInstant::from_image(1_000)
    );

    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    let reservation = task
        .reserve_recurring_dtm_event(event, &now)
        .expect("the exact recurring window is initially free");
    let result = task.finish_dtm_sequence_authorization(
        reservation.authorize_sequence(ControllerTimeSample::for_validation(2_000)),
    );

    assert_eq!(
        result.expect_err("the deliberately late sequence sample must fail"),
        DtmControllerEventPreparationError::SequenceAuthorization(
            crate::scheduler::SchedulerSequenceAuthorizationError::DeadlineExpired,
        )
    );
    drop((interrupt, task, modem_timer));
    assert!(scheduler.runtime_is_pristine());
}
