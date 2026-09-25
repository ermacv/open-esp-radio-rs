use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    clock::ClockedResources,
    resources::{BluetoothRadioHardware, BluetoothStopped},
    runtime_resources::ControllerRuntimeResources,
};

use std::{cell::RefCell, rc::Rc, vec::Vec};

use crate::scheduler::core::{
    BluetoothSchedulerHardwareListsCleared, SchedulerEmptyListMergeError,
    SchedulerExclusiveListEpoch, SchedulerFinishedListDrainState,
};

static PLATFORM_DROPS: AtomicUsize = AtomicUsize::new(0);

struct FakePlatform;

impl Drop for FakePlatform {
    fn drop(&mut self) {
        PLATFORM_DROPS.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn finished_list_drain_exposes_owner_only_after_the_capture_is_exhausted() {
    let drained_owner = Rc::new(());
    let drained_identity = Rc::clone(&drained_owner);
    let drained = SchedulerFinishedListDrainState::from_worker_step(drained_owner, false);
    let SchedulerFinishedListDrainState::Drained(drained_owner) = drained else {
        panic!("an exhausted capture must return the ordinary owner");
    };
    assert!(Rc::ptr_eq(&drained_owner, &drained_identity));

    let pending_owner = Rc::new(());
    let pending_identity = Rc::clone(&pending_owner);
    let pending = SchedulerFinishedListDrainState::from_worker_step(pending_owner, true);
    let SchedulerFinishedListDrainState::Pending(pending) = pending else {
        panic!("a retained capture must keep continuation provenance");
    };
    assert!(Rc::ptr_eq(pending.owner(), &pending_identity));
    assert!(Rc::ptr_eq(&pending.into_owner(), &pending_identity));
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ModelSingleItemIdentity {
    Expected,
    Foreign,
}

#[test]
fn single_item_identity_mismatch_returns_the_exact_owner() {
    let owner = Rc::new(());
    let identity = Rc::clone(&owner);
    let Err((expected, returned)) = crate::scheduler::core::retain_matching_single_item_identity(
        ModelSingleItemIdentity::Expected,
        ModelSingleItemIdentity::Foreign,
        owner,
    ) else {
        panic!("a foreign role item must fail closed");
    };
    assert!(matches!(expected, ModelSingleItemIdentity::Expected));
    assert!(Rc::ptr_eq(&returned, &identity));
}

#[test]
fn exclusive_empty_epoch_rejects_alias_and_wrong_identity_cancel() {
    let mut list =
        SchedulerExclusiveListEpoch::new(BluetoothSchedulerHardwareListsCleared::for_validation());
    let first = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("first item lies in controller SRAM");
    let other = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0200)
        .expect("second item lies in controller SRAM");

    assert_eq!(list.prepare_first_item(first), Ok(()));
    assert_eq!(
        list.prepare_first_item(other),
        Err(SchedulerEmptyListMergeError::ListNotEmpty)
    );
    assert!(!list.cancel_first_item(other));
    assert!(list.cancel_first_item(first));
    assert_eq!(list.prepare_first_item(other), Ok(()));
}

#[test]
fn published_first_item_cannot_be_cancelled_or_replaced() {
    let mut list =
        SchedulerExclusiveListEpoch::new(BluetoothSchedulerHardwareListsCleared::for_validation());
    let first = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("first item lies in controller SRAM");
    let other = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0200)
        .expect("second item lies in controller SRAM");

    assert_eq!(list.prepare_first_item(first), Ok(()));
    assert!(list.can_publish_first_item(first));
    assert!(!list.can_publish_first_item(other));
    list.retain_published_first_item(first);

    assert!(!list.can_publish_first_item(first));
    assert!(!list.cancel_first_item(first));
    list.retain_running_first_item(first);
    assert!(list.retains_running_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(SchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_completion_observed_first_item(first);
    assert!(list.retains_completion_observed_first_item(first));
    assert!(!list.retains_running_first_item(first));
    assert!(!list.cancel_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(SchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_hardware_head_empty_first_item(first);
    assert!(!list.retains_completion_observed_first_item(first));
    assert!(list.retains_hardware_head_empty_first_item(first));
    assert!(list.unlink_software_list_first_item(first));
    assert!(!list.unlink_software_list_first_item(first));
    assert!(list.retains_unlinked_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(SchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.retain_software_list_removal_ready_first_item(first);
    assert!(list.retains_software_list_removal_ready_first_item(first));
    assert_eq!(
        list.prepare_first_item(other),
        Err(SchedulerEmptyListMergeError::ListNotEmpty)
    );
    list.commit_recycled_first_item();
    assert_eq!(list.prepare_first_item(other), Ok(()));
}

#[test]
fn powered_task_split_retains_the_same_running_list_identity() {
    struct TaskSplitPlatform;

    let stopped = BluetoothStopped::from_hardware(
        TaskSplitPlatform,
        BluetoothRadioHardware::for_validation(),
    );
    let (registers, platform) = stopped.into_parts();
    let clocked = ClockedResources::for_validation(registers, platform);
    let initialized = clocked.initialize_controller_hal_with(|_, _| {});
    let mut scheduler =
        initialized.initialize_scheduler_for_validation(ControllerRuntimeResources::<1, 1>::new());
    let address = oer_esp32s31_hal::types::BluetoothControllerSramAddress::new(0x2f00_0100)
        .expect("test item lies in Controller SRAM");
    scheduler
        ._scheduler_list
        .prepare_first_item(address)
        .expect("exclusive list starts empty");
    scheduler
        ._scheduler_list
        .retain_published_first_item(address);

    let (interrupt, mut task, modem_timer, _platform) = scheduler
        .split_runtime()
        .expect("first task owner transfer");
    task.retain_running_first_item(address);
    drop((interrupt, task, modem_timer));

    assert!(
        scheduler
            ._scheduler_list
            .retains_running_first_item(address)
    );
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
        initialized.initialize_scheduler_with(ControllerRuntimeResources::<4, 3>::new(), |_| {
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
    assert_eq!(scheduler.scheduler_capacity(), 3);
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
