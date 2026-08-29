//! Pure control model for the common scheduler insertion boundary.
//!
//! The current ESP32-S31 scheduler separates insertion-begin, list merging,
//! an optional lock/modify transaction and insertion-end. These types retain
//! that control flow without exposing positional result integers, descriptor
//! memory or register authority. They are plans only: live execution still
//! requires affine ownership of the submitted item, the merge-selected item
//! and the scheduler list.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::BluetoothSchedulerInsertionCommand;

/// Semantic outcome of the current scheduler insertion-begin stage.
///
/// The names deliberately describe the established ownership consequence,
/// not the historical positional return image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerInsertionBeginOutcome {
    /// No execution lock remains owned by this insertion.
    Unlocked,
    /// Command-zero execution lock remains owned by this insertion.
    ExecutionLockRetained,
    /// The current hardware head was captured and reconciled through the
    /// command-one modification path.
    CurrentHeadReconciled,
}

/// Whether the common wrapper may enter its conditional lock/modify branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerInsertionLockModifyGate {
    /// This insertion-begin outcome skips lock/modify.
    Skip,
    /// The wrapper must still check its environment gate and use the exact
    /// item selected by merge/list state.
    CheckEnvironmentAndMergeSelection,
}

/// Actions that insertion-end performs before checking scheduler sleep state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end prelude must be applied before its continuation"]
pub struct BluetoothSchedulerInsertionEndPrelude {
    publish_submitted_head: bool,
    command_to_clear: Option<BluetoothSchedulerInsertionCommand>,
}

impl BluetoothSchedulerInsertionEndPrelude {
    /// Whether insertion-end first publishes the originally submitted item as
    /// the hardware-list head.
    pub const fn publishes_submitted_head(self) -> bool {
        self.publish_submitted_head
    }

    /// Command START field cleared after any prelude head publication.
    pub const fn command_to_clear(self) -> Option<BluetoothSchedulerInsertionCommand> {
        self.command_to_clear
    }

    /// Continue after the prelude with the current scheduler sleep policy.
    pub const fn observe_sleep_policy(
        self,
        sleep_enabled: bool,
    ) -> BluetoothSchedulerInsertionSleepDecision {
        if sleep_enabled {
            BluetoothSchedulerInsertionSleepDecision::ObserveSchedulerBusy(
                BluetoothSchedulerInsertionBusyGate { _private: () },
            )
        } else {
            BluetoothSchedulerInsertionSleepDecision::PublishManagerSoftwareHead
        }
    }
}

/// Next action selected by the current scheduler sleep policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end sleep decision must be applied or advanced"]
pub enum BluetoothSchedulerInsertionSleepDecision {
    /// Sleep is disabled: publish the manager's software-list head without a
    /// hardware RUN command.
    PublishManagerSoftwareHead,
    /// Sleep is enabled: obtain a fresh scheduler BUSY observation.
    ObserveSchedulerBusy(BluetoothSchedulerInsertionBusyGate),
}

/// Permission to classify one fresh scheduler BUSY observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerInsertionBusyGate {
    _private: (),
}

impl BluetoothSchedulerInsertionBusyGate {
    /// Select the next current insertion-end action.
    pub const fn observe(self, scheduler_busy: bool) -> BluetoothSchedulerInsertionBusyDecision {
        if scheduler_busy {
            BluetoothSchedulerInsertionBusyDecision::NoFurtherHardwareAction
        } else {
            BluetoothSchedulerInsertionBusyDecision::ObserveSubmittedItemStatus(
                BluetoothSchedulerInsertionItemStatusGate { _private: () },
            )
        }
    }
}

/// Action after the sleep-enabled scheduler BUSY observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end busy decision must be applied or advanced"]
pub enum BluetoothSchedulerInsertionBusyDecision {
    /// The scheduler is still busy, so insertion-end performs no later head or
    /// RUN publication.
    NoFurtherHardwareAction,
    /// The scheduler is idle; inspect the submitted item's typed in-flight
    /// status before deciding whether publication is still required.
    ObserveSubmittedItemStatus(BluetoothSchedulerInsertionItemStatusGate),
}

/// Permission to classify the submitted item's semantic in-flight status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerInsertionItemStatusGate {
    _private: (),
}

impl BluetoothSchedulerInsertionItemStatusGate {
    /// Finish the current insertion-end decision without exposing the SRAM
    /// status image used to obtain `is_in_flight`.
    pub const fn observe(self, is_in_flight: bool) -> BluetoothSchedulerInsertionFinalAction {
        if is_in_flight {
            BluetoothSchedulerInsertionFinalAction::PublishSubmittedHeadAndRun
        } else {
            BluetoothSchedulerInsertionFinalAction::NoFurtherHardwareAction
        }
    }
}

/// Final insertion-end action after all required semantic observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerInsertionFinalAction {
    /// The item no longer carries the in-flight state, so no head or RUN write
    /// is issued on this edge.
    NoFurtherHardwareAction,
    /// Publish the submitted item as head and then perform the complete
    /// scheduler-run prefix before its final hardware RUN command.
    PublishSubmittedHeadAndRun,
}

impl BluetoothSchedulerInsertionBeginOutcome {
    /// Select whether the wrapper may consider lock/modify after list merging.
    pub const fn lock_modify_gate(self) -> BluetoothSchedulerInsertionLockModifyGate {
        match self {
            Self::ExecutionLockRetained => {
                BluetoothSchedulerInsertionLockModifyGate::CheckEnvironmentAndMergeSelection
            }
            Self::Unlocked | Self::CurrentHeadReconciled => {
                BluetoothSchedulerInsertionLockModifyGate::Skip
            }
        }
    }

    /// Select the ordered insertion-end prelude for this begin outcome.
    pub const fn insertion_end_prelude(self) -> BluetoothSchedulerInsertionEndPrelude {
        match self {
            Self::Unlocked => BluetoothSchedulerInsertionEndPrelude {
                publish_submitted_head: false,
                command_to_clear: None,
            },
            Self::ExecutionLockRetained => BluetoothSchedulerInsertionEndPrelude {
                publish_submitted_head: false,
                command_to_clear: Some(BluetoothSchedulerInsertionCommand::Zero),
            },
            Self::CurrentHeadReconciled => BluetoothSchedulerInsertionEndPrelude {
                publish_submitted_head: true,
                command_to_clear: Some(BluetoothSchedulerInsertionCommand::One),
            },
        }
    }
}

#[cfg(test)]
mod tests {
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
    fn sleep_disabled_uses_the_manager_head_without_observing_busy_or_status() {
        assert_eq!(
            BluetoothSchedulerInsertionBeginOutcome::Unlocked
                .insertion_end_prelude()
                .observe_sleep_policy(false),
            BluetoothSchedulerInsertionSleepDecision::PublishManagerSoftwareHead
        );
    }

    #[test]
    fn sleep_enabled_path_observes_busy_before_item_status() {
        let busy_gate = match BluetoothSchedulerInsertionBeginOutcome::Unlocked
            .insertion_end_prelude()
            .observe_sleep_policy(true)
        {
            BluetoothSchedulerInsertionSleepDecision::ObserveSchedulerBusy(gate) => gate,
            BluetoothSchedulerInsertionSleepDecision::PublishManagerSoftwareHead => {
                panic!("sleep-enabled insertion skipped its busy observation")
            }
        };
        assert_eq!(
            busy_gate.observe(true),
            BluetoothSchedulerInsertionBusyDecision::NoFurtherHardwareAction
        );

        let status_gate = match busy_gate.observe(false) {
            BluetoothSchedulerInsertionBusyDecision::ObserveSubmittedItemStatus(gate) => gate,
            BluetoothSchedulerInsertionBusyDecision::NoFurtherHardwareAction => {
                panic!("idle insertion skipped its submitted-item status")
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
}
