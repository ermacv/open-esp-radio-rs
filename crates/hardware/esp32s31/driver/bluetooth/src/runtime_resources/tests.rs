use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerInstant;

use super::ControllerRuntimeResources;

#[test]
fn one_aggregate_starts_as_one_pristine_bounded_epoch() {
    let resources = ControllerRuntimeResources::<4>::new();

    assert_eq!(resources.modem_timer_capacity(), 4);
    assert!(resources.is_pristine());
}

#[test]
#[should_panic(expected = "at least one modem timer slot")]
fn zero_modem_timer_capacity_profile_is_rejected() {
    let _resources = ControllerRuntimeResources::<0>::new();
}

#[test]
fn split_borrows_one_matching_interrupt_and_task_epoch() {
    let mut resources = ControllerRuntimeResources::<4>::new();
    let (interrupt, mut task, modem_timer) = resources.split();

    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    assert!(!task.scheduler_finished_lists().is_active());
    assert!(modem_timer.queue_is_empty());
    drop((interrupt, task, modem_timer));
    assert!(resources.is_pristine());
}

#[test]
fn split_assigns_mutable_timer_queue_only_to_the_modem_task_endpoint() {
    let mut resources = ControllerRuntimeResources::<2>::new();
    let (interrupt, task, modem_timer) = resources.split();

    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    let token = modem_timer
        .queue
        .schedule(
            BluetoothModemLpTimerInstant::from_bits(10),
            BluetoothModemLpTimerInstant::from_bits(20),
        )
        .expect("the disjoint timer endpoint owns both fixed slots");
    assert!(!modem_timer.queue_is_empty());
    assert!(modem_timer.queue.cancel(token));
    assert!(modem_timer.queue_is_empty());

    drop((interrupt, task, modem_timer));
    assert!(resources.is_pristine());
}

#[test]
fn stale_readiness_does_not_impersonate_work_and_is_cleared_only_after_cold_release() {
    let mut resources = ControllerRuntimeResources::<2>::new();
    let (interrupt, task, _timer) = resources.split();
    assert_eq!(task.retirement_ready(), Ok(()));
    interrupt
        .scheduler_wake()
        .publish_from_interrupt(crate::interrupt::SchedulerWorkerWakeClass::Ordinary);
    assert_eq!(task.retirement_ready(), Ok(()));
    assert!(
        task.scheduler_wake().is_pending(),
        "admission must not discard a publication"
    );
    task.clear_notifications_after_cold_release();
    assert!(!task.scheduler_wake().is_pending());
    assert_eq!(task.retirement_ready(), Ok(()));
}
