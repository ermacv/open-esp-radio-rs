use oer_esp32s31_hal::bluetooth::BluetoothSchedulerInsertionCommand;

use super::{
    SchedulerInsertionBeginOutcome, SchedulerInsertionBusyDecision, SchedulerInsertionFinalAction,
    SchedulerInsertionLockModifyGate, SchedulerInsertionSleepDecision,
};

#[test]
fn only_the_retained_execution_lock_can_enter_lock_modify() {
    assert_eq!(
        SchedulerInsertionBeginOutcome::Unlocked.lock_modify_gate(),
        SchedulerInsertionLockModifyGate::Skip
    );
    assert_eq!(
        SchedulerInsertionBeginOutcome::ExecutionLockRetained.lock_modify_gate(),
        SchedulerInsertionLockModifyGate::CheckEnvironmentAndMergeSelection
    );
    assert_eq!(
        SchedulerInsertionBeginOutcome::CurrentHeadReconciled.lock_modify_gate(),
        SchedulerInsertionLockModifyGate::Skip
    );
}

#[test]
fn insertion_end_preludes_preserve_command_and_head_ordering() {
    let unlocked = SchedulerInsertionBeginOutcome::Unlocked.insertion_end_prelude();
    assert!(!unlocked.publishes_submitted_head());
    assert_eq!(unlocked.command_to_clear(), None);

    let locked = SchedulerInsertionBeginOutcome::ExecutionLockRetained.insertion_end_prelude();
    assert!(!locked.publishes_submitted_head());
    assert_eq!(
        locked.command_to_clear(),
        Some(BluetoothSchedulerInsertionCommand::Zero)
    );

    let reconciled = SchedulerInsertionBeginOutcome::CurrentHeadReconciled.insertion_end_prelude();
    assert!(reconciled.publishes_submitted_head());
    assert_eq!(
        reconciled.command_to_clear(),
        Some(BluetoothSchedulerInsertionCommand::One)
    );
}

#[test]
fn busy_short_circuits_before_sleep_policy_or_item_status() {
    assert_eq!(
        SchedulerInsertionBeginOutcome::Unlocked
            .insertion_end_prelude()
            .observe_scheduler_busy(true),
        SchedulerInsertionBusyDecision::NoFurtherHardwareAction
    );
}

#[test]
fn idle_path_observes_sleep_policy_before_item_status() {
    let sleep_gate = match SchedulerInsertionBeginOutcome::Unlocked
        .insertion_end_prelude()
        .observe_scheduler_busy(false)
    {
        SchedulerInsertionBusyDecision::ObserveSleepPolicy(gate) => gate,
        SchedulerInsertionBusyDecision::NoFurtherHardwareAction => {
            panic!("idle insertion skipped its sleep-policy observation")
        }
    };
    assert_eq!(
        sleep_gate.observe(false),
        SchedulerInsertionSleepDecision::PublishManagerSoftwareHead
    );

    let status_gate = match sleep_gate.observe(true) {
        SchedulerInsertionSleepDecision::ObserveSubmittedItemStatus(gate) => gate,
        SchedulerInsertionSleepDecision::PublishManagerSoftwareHead => {
            panic!("sleep-enabled insertion skipped its submitted-item status")
        }
    };
    assert_eq!(
        status_gate.observe(false),
        SchedulerInsertionFinalAction::NoFurtherHardwareAction
    );
    assert_eq!(
        status_gate.observe(true),
        SchedulerInsertionFinalAction::PublishSubmittedHeadAndRun
    );
}
