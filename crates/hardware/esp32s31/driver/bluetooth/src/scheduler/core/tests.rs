use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use std::{cell::RefCell, rc::Rc, vec::Vec};

use oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListsCleared;

static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

struct FakePlatform;

impl Drop for FakePlatform {
    fn drop(&mut self) {
        PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn controller_hal_precedes_complete_scheduler_init_and_arms_fail_stop() {
    PLATFORM_DROPS.store(0, Ordering::Relaxed);
    let stopped =
        BluetoothStopped::from_hardware(FakePlatform, BluetoothRadioHardware::for_validation());
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let operations = Rc::new(RefCell::new(Vec::new()));
    let hal_operations = Rc::clone(&operations);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {
        hal_operations.borrow_mut().push("controller-hal");
    });
    let time_scale = initialized.controller_time_scale();
    let scheduler_operations = Rc::clone(&operations);
    let mut scheduler =
        initialized.initialize_scheduler_with(ControllerRuntimeResources::<4>::new(), |_| {
            scheduler_operations.borrow_mut().push("scheduler-hardware");
            BluetoothSchedulerHardwareListsCleared::for_validation()
        });
    assert_eq!(
        operations.borrow().as_slice(),
        ["controller-hal", "scheduler-hardware"]
    );
    assert_eq!(scheduler.controller_time_scale(), time_scale);
    assert_eq!(
        scheduler.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!scheduler.controller_time_needs_recheck());
    assert_eq!(scheduler.modem_timer_capacity(), 4);
    assert!(scheduler.runtime_is_pristine());
    let (interrupt, task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    assert!(core::ptr::eq(
        interrupt.scheduler_wake(),
        task.scheduler_wake()
    ));
    assert_eq!(
        task.controller_time_phase(),
        crate::controller_time::ControllerTimeWorkerPhase::Idle
    );
    assert!(!task.controller_time_needs_recheck());
    drop((interrupt, task, modem_timer));
    drop(scheduler);
    assert_eq!(PLATFORM_DROPS.load(Ordering::Relaxed), 0);
}
