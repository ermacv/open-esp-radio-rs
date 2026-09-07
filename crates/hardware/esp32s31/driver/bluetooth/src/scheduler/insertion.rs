//! Pure control model for the common scheduler insertion boundary.
//!
//! The current ESP32-S31 scheduler separates insertion-begin, list merging,
//! an optional lock/modify transaction and insertion-end. These types retain
//! that control flow without exposing positional result integers, descriptor
//! memory or register authority. They are plans only: live execution still
//! requires affine ownership of the submitted item, the merge-selected item
//! and the scheduler list.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::bluetooth::BluetoothSchedulerInsertionCommand;

/// Semantic outcome of the current scheduler insertion-begin stage.
///
/// The names deliberately describe the established ownership consequence,
/// not the historical positional return image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerInsertionBeginOutcome {
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
pub enum SchedulerInsertionLockModifyGate {
    /// This insertion-begin outcome skips lock/modify.
    Skip,
    /// The wrapper must still check its environment gate and use the exact
    /// item selected by merge/list state.
    CheckEnvironmentAndMergeSelection,
}

/// Actions that insertion-end performs before checking scheduler BUSY state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end prelude must be applied before its continuation"]
pub struct SchedulerInsertionEndPrelude {
    publish_submitted_head: bool,
    command_to_clear: Option<BluetoothSchedulerInsertionCommand>,
}

impl SchedulerInsertionEndPrelude {
    /// Whether insertion-end first publishes the originally submitted item as
    /// the hardware-list head.
    pub const fn publishes_submitted_head(self) -> bool {
        self.publish_submitted_head
    }

    /// Command START field cleared after any prelude head publication.
    pub const fn command_to_clear(self) -> Option<BluetoothSchedulerInsertionCommand> {
        self.command_to_clear
    }

    /// Continue after the prelude with one fresh scheduler BUSY observation.
    pub const fn observe_scheduler_busy(
        self,
        scheduler_busy: bool,
    ) -> SchedulerInsertionBusyDecision {
        if scheduler_busy {
            SchedulerInsertionBusyDecision::NoFurtherHardwareAction
        } else {
            SchedulerInsertionBusyDecision::ObserveSleepPolicy(SchedulerInsertionSleepGate {
                _private: (),
            })
        }
    }
}

/// Action after the scheduler BUSY observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end busy decision must be applied or advanced"]
pub enum SchedulerInsertionBusyDecision {
    /// The scheduler is still busy, so insertion-end performs no later head or
    /// RUN publication.
    NoFurtherHardwareAction,
    /// The scheduler is idle; obtain the current sleep-policy observation.
    ObserveSleepPolicy(SchedulerInsertionSleepGate),
}

/// Permission to classify the current scheduler sleep policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerInsertionSleepGate {
    _private: (),
}

impl SchedulerInsertionSleepGate {
    /// Select the next current insertion-end action.
    pub const fn observe(self, sleep_enabled: bool) -> SchedulerInsertionSleepDecision {
        if sleep_enabled {
            SchedulerInsertionSleepDecision::ObserveSubmittedItemStatus(
                SchedulerInsertionItemStatusGate { _private: () },
            )
        } else {
            SchedulerInsertionSleepDecision::PublishManagerSoftwareHead
        }
    }
}

/// Action after the scheduler is idle and sleep policy has been observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the insertion-end sleep decision must be applied or advanced"]
pub enum SchedulerInsertionSleepDecision {
    /// Sleep is disabled: publish the manager's software-list head without a
    /// hardware RUN command.
    PublishManagerSoftwareHead,
    /// Sleep is enabled: inspect the submitted item's typed in-flight status
    /// before deciding whether publication is still required.
    ObserveSubmittedItemStatus(SchedulerInsertionItemStatusGate),
}

/// Permission to classify the submitted item's semantic in-flight status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerInsertionItemStatusGate {
    _private: (),
}

impl SchedulerInsertionItemStatusGate {
    /// Finish the current insertion-end decision without exposing the SRAM
    /// status image used to obtain `is_in_flight`.
    pub const fn observe(self, is_in_flight: bool) -> SchedulerInsertionFinalAction {
        if is_in_flight {
            SchedulerInsertionFinalAction::PublishSubmittedHeadAndRun
        } else {
            SchedulerInsertionFinalAction::NoFurtherHardwareAction
        }
    }
}

/// Final insertion-end action after all required semantic observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerInsertionFinalAction {
    /// The item no longer carries the in-flight state, so no head or RUN write
    /// is issued on this edge.
    NoFurtherHardwareAction,
    /// Publish the submitted item as head and then perform the complete
    /// scheduler-run prefix before its final hardware RUN command.
    PublishSubmittedHeadAndRun,
}

impl SchedulerInsertionBeginOutcome {
    /// Select whether the wrapper may consider lock/modify after list merging.
    pub const fn lock_modify_gate(self) -> SchedulerInsertionLockModifyGate {
        match self {
            Self::ExecutionLockRetained => {
                SchedulerInsertionLockModifyGate::CheckEnvironmentAndMergeSelection
            }
            Self::Unlocked | Self::CurrentHeadReconciled => SchedulerInsertionLockModifyGate::Skip,
        }
    }

    /// Select the ordered insertion-end prelude for this begin outcome.
    pub const fn insertion_end_prelude(self) -> SchedulerInsertionEndPrelude {
        match self {
            Self::Unlocked => SchedulerInsertionEndPrelude {
                publish_submitted_head: false,
                command_to_clear: None,
            },
            Self::ExecutionLockRetained => SchedulerInsertionEndPrelude {
                publish_submitted_head: false,
                command_to_clear: Some(BluetoothSchedulerInsertionCommand::Zero),
            },
            Self::CurrentHeadReconciled => SchedulerInsertionEndPrelude {
                publish_submitted_head: true,
                command_to_clear: Some(BluetoothSchedulerInsertionCommand::One),
            },
        }
    }
}

#[cfg(test)]
mod tests;
