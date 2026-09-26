//! Insertion into a running hardware list.
//!
//! This follows the vendor insertion bracket. With a listed predecessor, the
//! executor first requests the execution lock at that predecessor. A retained
//! lock admits linking the new event and a lock-modify request for it; the
//! lock is released afterwards. Without a predecessor, or when the lock is not
//! retained, the executor requests execution modify for the list, links the
//! new event, publishes it as the hardware head and releases modify. The
//! vendor takes that path when hardware has already reached the predecessor,
//! so every earlier event is running or finished.
//!
//! Every hardware wait is a separate observation; the executor never polls.
//! The effect of each command on execution is a hardware contract that the
//! vendor bodies do not establish, so each result keeps its reviewed
//! classification and an unsupported result ends the insertion without
//! linking the event.

use oer_esp32s31_bluetooth_memory::ControllerSramLinkAddress;
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionModifyDisposition,
    BluetoothSchedulerLockModifyObservation, BluetoothSchedulerWorkObservation,
};

use super::{SchedulerExecutor, SchedulerItemAccess, SchedulerSubmitError};
use crate::scheduler::window::SchedulerRawWindow;

/// One hardware action of a live insertion, performed in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerLiveAction {
    /// Publish the execution lock at this listed item of list zero.
    PublishExecutionLock(ControllerSramLinkAddress),
    /// Clear the execution-lock START.
    ReleaseExecutionLock,
    /// Publish execution modify for list zero.
    PublishExecutionModify,
    /// Clear the execution-modify START.
    ReleaseExecutionModify,
    /// Publish a lock-modify request for this item of list zero.
    PublishLockModify(ControllerSramLinkAddress),
    /// Publish this item as the head of list zero.
    PublishHead(ControllerSramLinkAddress),
}

/// Hardware observation that a live insertion waits for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerLiveWait {
    ExecutionLock,
    ExecutionModify,
    LockModify,
}

/// What follows the actions of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerLiveNext {
    /// Deliver the next observation of this kind.
    Await(SchedulerLiveWait),
    /// The insertion is complete; the mirror accepts other operations again.
    Finished,
}

/// Actions to perform in order, then the next expectation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the actions must be performed before the next observation"]
pub struct SchedulerLiveStep {
    actions: [Option<SchedulerLiveAction>; 2],
    next: SchedulerLiveNext,
}

impl SchedulerLiveStep {
    const fn new(actions: [Option<SchedulerLiveAction>; 2], next: SchedulerLiveNext) -> Self {
        Self { actions, next }
    }

    const fn wait(wait: SchedulerLiveWait) -> Self {
        Self::new([None, None], SchedulerLiveNext::Await(wait))
    }

    /// Actions to perform, in order.
    pub fn actions(&self) -> impl Iterator<Item = SchedulerLiveAction> + '_ {
        self.actions.iter().flatten().copied()
    }

    pub const fn next(&self) -> SchedulerLiveNext {
        self.next
    }
}

/// Observation delivered to a waiting live insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerLiveObservation {
    ExecutionLock(BluetoothSchedulerExecutionLockDisposition),
    ExecutionModify(BluetoothSchedulerExecutionModifyDisposition),
    LockModify(BluetoothSchedulerLockModifyObservation),
}

/// Why a live insertion ended without linking its event, or why an
/// observation was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerLiveFault {
    /// No live insertion is in progress.
    NoInsertion,
    /// The observation does not match the awaited kind; the insertion
    /// continues to wait.
    UnexpectedObservation,
    /// The execution lock returned a result on which the vendor asserts. The
    /// caller must still clear the execution-lock START.
    UnsupportedExecutionLockResult,
    /// Execution modify reported the status the vendor treats as impossible.
    /// The caller must still clear the execution-modify START.
    ExecutionModifyRejected,
}

#[derive(Clone, Copy)]
pub(super) enum LivePhase<I> {
    Lock { id: I, window: SchedulerRawWindow },
    Modify { id: I, window: SchedulerRawWindow },
    LockModify,
}

impl<I: Copy + Eq, const CAPACITY: usize> SchedulerExecutor<I, CAPACITY> {
    /// Begin inserting `id` into the running list.
    ///
    /// The position is only planned here; nothing is linked until hardware
    /// grants the lock or modify.
    pub fn begin_live_insertion(
        &mut self,
        items: &impl SchedulerItemAccess<I>,
        scheduler: BluetoothSchedulerWorkObservation,
        id: I,
        window: SchedulerRawWindow,
    ) -> Result<SchedulerLiveStep, SchedulerSubmitError<I>> {
        if self.live.is_some() {
            return Err(SchedulerSubmitError::InsertionActive);
        }
        if !scheduler.is_busy() {
            return Err(SchedulerSubmitError::SchedulerIdle);
        }
        let placement = self
            .list
            .plan_insert(id, window)
            .map_err(SchedulerSubmitError::List)?;
        Ok(match placement.predecessor {
            Some(predecessor) => {
                self.live = Some(LivePhase::Lock { id, window });
                SchedulerLiveStep::new(
                    [
                        Some(SchedulerLiveAction::PublishExecutionLock(
                            items.link(predecessor),
                        )),
                        None,
                    ],
                    SchedulerLiveNext::Await(SchedulerLiveWait::ExecutionLock),
                )
            }
            None => {
                self.live = Some(LivePhase::Modify { id, window });
                SchedulerLiveStep::new(
                    [Some(SchedulerLiveAction::PublishExecutionModify), None],
                    SchedulerLiveNext::Await(SchedulerLiveWait::ExecutionModify),
                )
            }
        })
    }

    /// Advance the live insertion with the awaited observation.
    pub fn advance_live_insertion(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        observation: SchedulerLiveObservation,
    ) -> Result<SchedulerLiveStep, SchedulerLiveFault> {
        let phase = self.live.ok_or(SchedulerLiveFault::NoInsertion)?;
        match (phase, observation) {
            (
                LivePhase::Lock { id, window },
                SchedulerLiveObservation::ExecutionLock(disposition),
            ) => match disposition {
                BluetoothSchedulerExecutionLockDisposition::Pending => {
                    Ok(SchedulerLiveStep::wait(SchedulerLiveWait::ExecutionLock))
                }
                BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained => {
                    self.link_planned(items, id, window);
                    self.live = Some(LivePhase::LockModify);
                    Ok(SchedulerLiveStep::new(
                        [
                            Some(SchedulerLiveAction::PublishLockModify(items.link(id))),
                            None,
                        ],
                        SchedulerLiveNext::Await(SchedulerLiveWait::LockModify),
                    ))
                }
                BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead => {
                    self.live = Some(LivePhase::Modify { id, window });
                    Ok(SchedulerLiveStep::new(
                        [
                            Some(SchedulerLiveAction::ReleaseExecutionLock),
                            Some(SchedulerLiveAction::PublishExecutionModify),
                        ],
                        SchedulerLiveNext::Await(SchedulerLiveWait::ExecutionModify),
                    ))
                }
                BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult => {
                    self.live = None;
                    Err(SchedulerLiveFault::UnsupportedExecutionLockResult)
                }
            },
            (
                LivePhase::Modify { id, window },
                SchedulerLiveObservation::ExecutionModify(disposition),
            ) => match disposition {
                BluetoothSchedulerExecutionModifyDisposition::Pending => {
                    Ok(SchedulerLiveStep::wait(SchedulerLiveWait::ExecutionModify))
                }
                BluetoothSchedulerExecutionModifyDisposition::Ready => {
                    self.link_planned(items, id, window);
                    self.live = None;
                    Ok(SchedulerLiveStep::new(
                        [
                            Some(SchedulerLiveAction::PublishHead(items.link(id))),
                            Some(SchedulerLiveAction::ReleaseExecutionModify),
                        ],
                        SchedulerLiveNext::Finished,
                    ))
                }
                BluetoothSchedulerExecutionModifyDisposition::HardwareRejected => {
                    self.live = None;
                    Err(SchedulerLiveFault::ExecutionModifyRejected)
                }
            },
            (LivePhase::LockModify, SchedulerLiveObservation::LockModify(observation)) => {
                if observation.wait_active() {
                    Ok(SchedulerLiveStep::wait(SchedulerLiveWait::LockModify))
                } else {
                    self.live = None;
                    Ok(SchedulerLiveStep::new(
                        [Some(SchedulerLiveAction::ReleaseExecutionLock), None],
                        SchedulerLiveNext::Finished,
                    ))
                }
            }
            _ => Err(SchedulerLiveFault::UnexpectedObservation),
        }
    }

    /// Link the event planned by `begin_live_insertion`. The mirror is frozen
    /// while the insertion is active, so the planned position still holds.
    fn link_planned(
        &mut self,
        items: &mut impl SchedulerItemAccess<I>,
        id: I,
        window: SchedulerRawWindow,
    ) {
        self.link_into_list(items, id, window)
            .unwrap_or_else(|_| unreachable!("the frozen mirror keeps the planned position"));
    }
}
