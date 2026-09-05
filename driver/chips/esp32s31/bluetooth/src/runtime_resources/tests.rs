use open_esp_radio_esp32s31_hal::BluetoothModemLpTimerInstant;
use open_esp_radio_esp32s31_pac::BluetoothControllerHalInitConfig;

use super::BluetoothControllerRuntimeResources;
use crate::{BluetoothSchedulerSoftwareConfig, BluetoothSchedulerTimingPolicy};

#[test]
fn one_aggregate_starts_as_one_pristine_bounded_epoch() {
    let resources = BluetoothControllerRuntimeResources::<4, 3>::new();

    assert_eq!(resources.modem_timer_capacity(), 4);
    assert_eq!(resources.scheduler_capacity(), 3);
    assert!(resources.is_pristine());
}

#[test]
#[should_panic(expected = "at least one modem timer slot")]
fn zero_modem_timer_capacity_profile_is_rejected() {
    let _resources = BluetoothControllerRuntimeResources::<0, 1>::new();
}

#[test]
#[should_panic(expected = "at least one scheduler slot")]
fn zero_scheduler_capacity_profile_is_rejected() {
    let _resources = BluetoothControllerRuntimeResources::<1, 0>::new();
}

#[test]
fn split_borrows_one_matching_interrupt_and_task_epoch() {
    let mut resources = BluetoothControllerRuntimeResources::<4, 3>::new();
    let (interrupt, mut task, modem_timer) = resources.split();

    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert!(core::ptr::eq(
        interrupt.scheduler_lock_modify_events(),
        task.scheduler_lock_modify_events()
    ));
    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    assert!(task.scheduler_lock_modify_worker().is_idle());
    assert!(!task.scheduler_finished_lists().is_active());
    assert!(task.scheduler_timeline_mut().is_empty());
    assert!(modem_timer.queue_is_empty());
    drop((interrupt, task, modem_timer));
    assert!(resources.is_pristine());
}

#[test]
fn split_assigns_mutable_timer_queue_only_to_the_modem_task_endpoint() {
    let mut resources = BluetoothControllerRuntimeResources::<2, 1>::new();
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
fn controller_reservation_remains_in_the_runtime_epoch_until_explicit_release() {
    let mut resources = BluetoothControllerRuntimeResources::<4, 2>::new();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    let timing_policy = BluetoothSchedulerTimingPolicy::from_scheduler_config(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        scale,
    );

    let (interrupt, mut task, modem_timer) = resources.split();
    let reservation = task
        .scheduler_timeline_mut()
        .reserve_recurring_window(45, 100, timing_policy)
        .expect("one runtime-owned scheduler slot is free");
    assert!(!task.scheduler_timeline_mut().is_empty());

    assert!(task.scheduler_timeline_mut().release(reservation).is_ok());
    drop((interrupt, task, modem_timer));
    assert!(resources.is_pristine());
}
