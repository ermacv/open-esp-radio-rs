//! Shared lifecycle for one scheduler item from `RUN` to removal readiness.
//!
//! The spine owns only protocol order. A role backend retains the concrete
//! scheduler graph and classifies completion; packet extraction, status
//! interpretation and memory reclamation remain role-specific after
//! [`SingleItemCompletionStep::RemovalReady`].

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::{
    SingleItemSchedulerCompletionObserved, SingleItemSchedulerHardwareHeadEmptyObserved,
    SingleItemSchedulerRole, SingleItemSchedulerRunning,
    SingleItemSchedulerSoftwareListRemovalReady, SingleItemSchedulerSoftwareListUnlinked,
};

use crate::scheduler::{
    BluetoothSchedulerFinishedHardwareListObserved,
    core::{SchedulerFinishedListDrainPending, SchedulerFinishedListDrainState},
};

pub trait SingleItemCompletionRole {
    type Wake;
    type Running;
    type CompletionObserved;
    type HardwareHeadEmpty;
    type PostUnlinkAwaiting;
    type RemovalReady;
}

#[cfg(target_arch = "riscv32")]
impl<Role> SingleItemCompletionRole for Role
where
    Role: SingleItemSchedulerRole,
{
    type Wake = crate::interrupt::SchedulerWakeBatch;
    type Running = SingleItemSchedulerRunning<Role>;
    type CompletionObserved = SingleItemSchedulerCompletionObserved<Role>;
    type HardwareHeadEmpty = SingleItemSchedulerHardwareHeadEmptyObserved<Role>;
    type PostUnlinkAwaiting = crate::scheduler::post_unlink::BluetoothPostUnlinkAwaiting<
        SingleItemSchedulerSoftwareListUnlinked<Role>,
    >;
    type RemovalReady = SingleItemSchedulerSoftwareListRemovalReady<Role>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleItemCompletionFaultCause {
    FinishedListDrainAlreadyActive,
    SchedulerIdentityMismatch,
    FinishedListDrainLost,
    RepeatedRoleList,
    FinishedListDrainStillActive,
    ExpectedHardwareHeadStillPublished,
    UnexpectedHardwareHeadChanged,
    PostUnlinkMailboxBusy,
    PostUnlinkMailboxIdentityExhausted,
    PostUnlinkMailboxGenerationExhausted,
    PostUnlinkMailboxCommitMismatch,
    PostUnlinkMailboxAffinityMismatch,
    PrimaryInterruptFault,
    PostUnlinkNoSchedulerWorkRearmMismatch,
    PostUnlinkPendingRearmMismatch,
    PostUnlinkRecheckUnavailable,
    PostUnlinkRecheckRearmMismatch,
}

pub struct SingleItemCompletionFault<Owner> {
    pub(crate) cause: SingleItemCompletionFaultCause,
    pub(crate) _owner: Owner,
}

impl<Owner> SingleItemCompletionFault<Owner> {
    /// Retain `owner` after the shared completion protocol faulted.
    pub const fn new(cause: SingleItemCompletionFaultCause, owner: Owner) -> Self {
        Self {
            cause,
            _owner: owner,
        }
    }

    /// Why the shared completion protocol retained the role owner.
    pub const fn cause(&self) -> SingleItemCompletionFaultCause {
        self.cause
    }

    /// Inspect the retained role owner in validation.
    #[cfg(any(test, feature = "test-support"))]
    pub fn owner_mut(&mut self) -> &mut Owner {
        &mut self._owner
    }
}

pub enum SingleItemRunningProgress<Role>
where
    Role: SingleItemCompletionRole,
{
    Running(SchedulerFinishedListDrainState<Role::Running>),
    CompletionObserved(SchedulerFinishedListDrainState<Role::CompletionObserved>),
    UnrelatedList {
        drain: SchedulerFinishedListDrainState<Role::Running>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

pub struct SingleItemCompletedDrainProgress<Role>
where
    Role: SingleItemCompletionRole,
{
    pub drain: SchedulerFinishedListDrainState<Role::CompletionObserved>,
    pub observed: BluetoothSchedulerFinishedHardwareListObserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleItemPostUnlinkDisposition {
    Continue,
    Waiting,
}

pub enum SingleItemPostUnlinkProgress<Role>
where
    Role: SingleItemCompletionRole,
{
    Pending {
        awaiting: Role::PostUnlinkAwaiting,
        disposition: SingleItemPostUnlinkDisposition,
    },
    Ready(Role::RemovalReady),
}

pub trait SingleItemCompletionBackend<Role>
where
    Role: SingleItemCompletionRole,
{
    type FaultOwner;

    fn take_scheduler_wake(&mut self) -> Option<Role::Wake>;

    fn observe_completion(
        &mut self,
        running: Role::Running,
        wake: Role::Wake,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>;

    fn continue_running_drain(
        &mut self,
        pending: SchedulerFinishedListDrainPending<Role::Running>,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>;

    fn continue_completed_drain(
        &mut self,
        pending: SchedulerFinishedListDrainPending<Role::CompletionObserved>,
    ) -> Result<SingleItemCompletedDrainProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>;

    fn observe_hardware_head_retirement(
        &mut self,
        completed: Role::CompletionObserved,
    ) -> Result<Role::HardwareHeadEmpty, SingleItemCompletionFault<Self::FaultOwner>>;

    fn unlink_and_arm(
        &mut self,
        observed: Role::HardwareHeadEmpty,
    ) -> Result<Role::PostUnlinkAwaiting, SingleItemCompletionFault<Self::FaultOwner>>;

    fn consume_post_unlink(
        &mut self,
        awaiting: Role::PostUnlinkAwaiting,
    ) -> Result<SingleItemPostUnlinkProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>;
}

enum SingleItemCompletionPhase<Role>
where
    Role: SingleItemCompletionRole,
{
    RunningAwaitingWake(Role::Running),
    RunningReady {
        running: Role::Running,
        wake: Role::Wake,
    },
    RunningDrain(SchedulerFinishedListDrainPending<Role::Running>),
    CompletionDrain(SchedulerFinishedListDrainPending<Role::CompletionObserved>),
    CompletionObserved(Role::CompletionObserved),
    HardwareHeadEmpty(Role::HardwareHeadEmpty),
    PostUnlinkAwaiting(Role::PostUnlinkAwaiting),
}

/// Executor-neutral owner of one single-item scheduler completion lifecycle.
pub struct SingleItemCompletion<Role>
where
    Role: SingleItemCompletionRole,
{
    phase: SingleItemCompletionPhase<Role>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleItemCompletionWaitKind {
    Scheduler,
    PostUnlink,
}

pub enum SingleItemCompletionStep<Role, FaultOwner>
where
    Role: SingleItemCompletionRole,
{
    Continue(SingleItemCompletion<Role>),
    Waiting(SingleItemCompletion<Role>),
    UnrelatedList {
        completion: SingleItemCompletion<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RemovalReady(Role::RemovalReady),
    Fault(SingleItemCompletionFault<FaultOwner>),
}

impl<Role> SingleItemCompletion<Role>
where
    Role: SingleItemCompletionRole,
{
    pub const fn new(running: Role::Running) -> Self {
        Self {
            phase: SingleItemCompletionPhase::RunningAwaitingWake(running),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn into_scheduler_wait_running(self) -> Result<Role::Running, Self> {
        match self.phase {
            SingleItemCompletionPhase::RunningAwaitingWake(running) => Ok(running),
            phase => Err(Self { phase }),
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub const fn from_hardware_head_empty(observed: Role::HardwareHeadEmpty) -> Self {
        Self {
            phase: SingleItemCompletionPhase::HardwareHeadEmpty(observed),
        }
    }

    pub const fn wait_kind(&self) -> Option<SingleItemCompletionWaitKind> {
        match &self.phase {
            SingleItemCompletionPhase::RunningAwaitingWake(_) => {
                Some(SingleItemCompletionWaitKind::Scheduler)
            }
            SingleItemCompletionPhase::PostUnlinkAwaiting(_) => {
                Some(SingleItemCompletionWaitKind::PostUnlink)
            }
            _ => None,
        }
    }

    pub fn step<Backend>(
        self,
        backend: &mut Backend,
    ) -> SingleItemCompletionStep<Role, Backend::FaultOwner>
    where
        Backend: SingleItemCompletionBackend<Role>,
    {
        match self.phase {
            SingleItemCompletionPhase::RunningAwaitingWake(running) => {
                let Some(wake) = backend.take_scheduler_wake() else {
                    return SingleItemCompletionStep::Waiting(Self::new(running));
                };
                SingleItemCompletionStep::Continue(Self {
                    phase: SingleItemCompletionPhase::RunningReady { running, wake },
                })
            }
            SingleItemCompletionPhase::RunningReady { running, wake } => {
                match backend.observe_completion(running, wake) {
                    Ok(progress) => Self::from_running_progress(progress),
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
            SingleItemCompletionPhase::RunningDrain(pending) => {
                match backend.continue_running_drain(pending) {
                    Ok(progress) => Self::from_running_progress(progress),
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
            SingleItemCompletionPhase::CompletionDrain(pending) => {
                match backend.continue_completed_drain(pending) {
                    Ok(progress) => {
                        let completion = Self::from_completed_drain(progress.drain);
                        SingleItemCompletionStep::UnrelatedList {
                            completion,
                            observed: progress.observed,
                        }
                    }
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
            SingleItemCompletionPhase::CompletionObserved(completed) => {
                match backend.observe_hardware_head_retirement(completed) {
                    Ok(observed) => SingleItemCompletionStep::Continue(Self {
                        phase: SingleItemCompletionPhase::HardwareHeadEmpty(observed),
                    }),
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
            SingleItemCompletionPhase::HardwareHeadEmpty(observed) => {
                match backend.unlink_and_arm(observed) {
                    Ok(awaiting) => SingleItemCompletionStep::Continue(Self {
                        phase: SingleItemCompletionPhase::PostUnlinkAwaiting(awaiting),
                    }),
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
            SingleItemCompletionPhase::PostUnlinkAwaiting(awaiting) => {
                match backend.consume_post_unlink(awaiting) {
                    Ok(SingleItemPostUnlinkProgress::Pending {
                        awaiting,
                        disposition,
                    }) => {
                        let completion = Self {
                            phase: SingleItemCompletionPhase::PostUnlinkAwaiting(awaiting),
                        };
                        match disposition {
                            SingleItemPostUnlinkDisposition::Continue => {
                                SingleItemCompletionStep::Continue(completion)
                            }
                            SingleItemPostUnlinkDisposition::Waiting => {
                                SingleItemCompletionStep::Waiting(completion)
                            }
                        }
                    }
                    Ok(SingleItemPostUnlinkProgress::Ready(ready)) => {
                        SingleItemCompletionStep::RemovalReady(ready)
                    }
                    Err(fault) => SingleItemCompletionStep::Fault(fault),
                }
            }
        }
    }

    fn from_running_progress<FaultOwner>(
        progress: SingleItemRunningProgress<Role>,
    ) -> SingleItemCompletionStep<Role, FaultOwner> {
        match progress {
            SingleItemRunningProgress::Running(drain) => match drain {
                SchedulerFinishedListDrainState::Drained(running) => {
                    SingleItemCompletionStep::Waiting(Self::new(running))
                }
                SchedulerFinishedListDrainState::Pending(pending) => {
                    SingleItemCompletionStep::Continue(Self {
                        phase: SingleItemCompletionPhase::RunningDrain(pending),
                    })
                }
            },
            SingleItemRunningProgress::CompletionObserved(drain) => {
                SingleItemCompletionStep::Continue(Self::from_completed_drain(drain))
            }
            SingleItemRunningProgress::UnrelatedList { drain, observed } => {
                let completion = match drain {
                    SchedulerFinishedListDrainState::Drained(running) => Self::new(running),
                    SchedulerFinishedListDrainState::Pending(pending) => Self {
                        phase: SingleItemCompletionPhase::RunningDrain(pending),
                    },
                };
                SingleItemCompletionStep::UnrelatedList {
                    completion,
                    observed,
                }
            }
        }
    }

    fn from_completed_drain(
        drain: SchedulerFinishedListDrainState<Role::CompletionObserved>,
    ) -> Self {
        match drain {
            SchedulerFinishedListDrainState::Drained(completed) => Self {
                phase: SingleItemCompletionPhase::CompletionObserved(completed),
            },
            SchedulerFinishedListDrainState::Pending(pending) => Self {
                phase: SingleItemCompletionPhase::CompletionDrain(pending),
            },
        }
    }
}

#[cfg(test)]
mod tests;
