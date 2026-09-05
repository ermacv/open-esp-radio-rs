use open_esp_radio_esp32s31_hal::BluetoothSchedulerInsertionCommand;

use super::{
    BluetoothSchedulerInsertionBeginOutcome, BluetoothSchedulerInsertionBusyDecision,
    BluetoothSchedulerInsertionFinalAction, BluetoothSchedulerInsertionLockModifyGate,
    BluetoothSchedulerInsertionSleepDecision,
};

#[test]
fn only_the_retained_execution_lock_can_enter_lock_modify() {
    assert_eq!(
        BluetoothSchedulerInsertionBeginOutcome::Unlocked.lock_modify_gate(),
        BluetoothSchedulerInsertionLockModifyGate::Skip
    );
    assert_eq!(
        BluetoothSchedulerInsertionBeginOutcome::ExecutionLockRetained.lock_modify_gate(),
        BluetoothSchedulerInsertionLockModifyGate::CheckEnvironmentAndMergeSelection
    );
    assert_eq!(
        BluetoothSchedulerInsertionBeginOutcome::CurrentHeadReconciled.lock_modify_gate(),
        BluetoothSchedulerInsertionLockModifyGate::Skip
    );
}

#[test]
fn insertion_end_preludes_preserve_command_and_head_ordering() {
    let unlocked = BluetoothSchedulerInsertionBeginOutcome::Unlocked.insertion_end_prelude();
    assert!(!unlocked.publishes_submitted_head());
    assert_eq!(unlocked.command_to_clear(), None);

    let locked =
        BluetoothSchedulerInsertionBeginOutcome::ExecutionLockRetained.insertion_end_prelude();
    assert!(!locked.publishes_submitted_head());
    assert_eq!(
        locked.command_to_clear(),
        Some(BluetoothSchedulerInsertionCommand::Zero)
    );

    let reconciled =
        BluetoothSchedulerInsertionBeginOutcome::CurrentHeadReconciled.insertion_end_prelude();
    assert!(reconciled.publishes_submitted_head());
    assert_eq!(
        reconciled.command_to_clear(),
        Some(BluetoothSchedulerInsertionCommand::One)
    );
}

#[test]
fn busy_short_circuits_before_sleep_policy_or_item_status() {
    assert_eq!(
        BluetoothSchedulerInsertionBeginOutcome::Unlocked
            .insertion_end_prelude()
            .observe_scheduler_busy(true),
        BluetoothSchedulerInsertionBusyDecision::NoFurtherHardwareAction
    );
}

#[test]
fn idle_path_observes_sleep_policy_before_item_status() {
    let sleep_gate = match BluetoothSchedulerInsertionBeginOutcome::Unlocked
        .insertion_end_prelude()
        .observe_scheduler_busy(false)
    {
        BluetoothSchedulerInsertionBusyDecision::ObserveSleepPolicy(gate) => gate,
        BluetoothSchedulerInsertionBusyDecision::NoFurtherHardwareAction => {
            panic!("idle insertion skipped its sleep-policy observation")
        }
    };
    assert_eq!(
        sleep_gate.observe(false),
        BluetoothSchedulerInsertionSleepDecision::PublishManagerSoftwareHead
    );

    let status_gate = match sleep_gate.observe(true) {
        BluetoothSchedulerInsertionSleepDecision::ObserveSubmittedItemStatus(gate) => gate,
        BluetoothSchedulerInsertionSleepDecision::PublishManagerSoftwareHead => {
            panic!("sleep-enabled insertion skipped its submitted-item status")
        }
    };
    assert_eq!(
        status_gate.observe(false),
        BluetoothSchedulerInsertionFinalAction::NoFurtherHardwareAction
    );
    assert_eq!(
        status_gate.observe(true),
        BluetoothSchedulerInsertionFinalAction::PublishSubmittedHeadAndRun
    );
}
