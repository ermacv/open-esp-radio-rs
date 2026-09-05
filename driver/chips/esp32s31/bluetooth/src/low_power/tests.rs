use crate::{
    BluetoothClockedResources, BluetoothControllerRuntimeResources, BluetoothRadioHardware,
    BluetoothSchedulerInitialized, BluetoothStopped,
};

fn scheduler() -> BluetoothSchedulerInitialized<(), 4, 3> {
    let stopped = BluetoothStopped::from_hardware((), BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    BluetoothClockedResources::for_validation(registers, platform)
        .initialize_controller_hal_with(|_, _| {})
        .initialize_scheduler_for_validation(BluetoothControllerRuntimeResources::new())
}

#[test]
fn scheduler_runtime_split_contains_only_hardware_services() {
    let mut scheduler = scheduler();
    assert!(scheduler.runtime_is_pristine());
    let (interrupt, task, modem_timer) = scheduler.split_runtime();
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
    let (interrupt, task, _) = controller.split_runtime();
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
