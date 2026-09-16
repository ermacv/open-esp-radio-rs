use crate::{
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
    scheduler::SchedulerInitialized,
};

fn scheduler() -> SchedulerInitialized<(), 4, 3> {
    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    ClockedResources::for_validation(registers, platform)
        .initialize_controller_hal_with(|_, _| {})
        .initialize_scheduler_for_validation(ControllerRuntimeResources::new())
}

#[test]
fn scheduler_runtime_split_contains_only_hardware_services() {
    let mut scheduler = scheduler();
    assert!(scheduler.runtime_is_pristine());
    let (interrupt, task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert!(core::ptr::eq(
        interrupt.modem_lp_timer_worker_wake(),
        modem_timer.worker_wake()
    ));
    assert!(modem_timer.queue_is_empty());
}

#[test]
fn low_power_hardware_stays_in_the_same_pristine_controller_epoch() {
    let (mut controller, timer_hardware) = match scheduler()
        .try_initialize_low_power_hardware_with(|_| Ok::<_, ()>("timer-hardware"))
    {
        Ok(initialized) => initialized,
        Err(_) => panic!("the injected low-power component must complete"),
    };

    assert_eq!(timer_hardware, "timer-hardware");
    assert!(controller.runtime_is_pristine());
    assert_eq!(controller.modem_timer_capacity(), 4);
    let (interrupt, task, _, _platform) = controller
        .split_runtime()
        .expect("first task owner transfer");
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
}

#[test]
fn low_power_hardware_failure_returns_the_complete_scheduler_epoch() {
    let (controller, error) = match scheduler()
        .try_initialize_low_power_hardware_with(|_| Err::<(), _>("timer-owner-separated"))
    {
        Ok(_) => panic!("the injected lower failure must remain visible"),
        Err(failure) => failure,
    };

    assert_eq!(error, "timer-owner-separated");
    assert!(controller.runtime_is_pristine());
    assert_eq!(controller.modem_timer_capacity(), 4);
    assert_eq!(controller.scheduler_capacity(), 3);
}

#[test]
fn dropping_runtime_never_recreates_the_transferred_task_owner() {
    let mut scheduler = scheduler();
    {
        let endpoints = scheduler.split_runtime().expect("first transfer");
        drop(endpoints);
    }
    assert!(scheduler.split_runtime().is_none());
    assert!(
        scheduler.runtime_is_pristine(),
        "no software work was fabricated"
    );
    assert!(
        scheduler.split_runtime().is_none(),
        "rejection must remain sticky"
    );
}

#[test]
fn task_hardware_outlives_software_storage_without_recovering_a_borrow() {
    let hardware = {
        let mut scheduler = scheduler();
        let (_, mut task, _, _platform) = scheduler.split_runtime().expect("first transfer");
        // Moving this field must transfer the actual owner, not a reference
        // whose lifetime would force the original scheduler to remain alive.
        let (hardware, ()) = task.task.try_retire(|| Ok::<_, ()>(())).unwrap();
        hardware
    };
    assert_eq!(
        hardware.controller_time_phase(),
        crate::controller::time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!hardware.controller_time_needs_recheck());
}
