//! Shared lifecycle for one scheduler item from `RUN` to removal readiness.
//!
//! The spine owns only protocol order. A role backend retains the concrete
//! scheduler graph and classifies completion; packet extraction, status
//! interpretation and memory reclamation remain role-specific after
//! [`BluetoothSingleItemCompletionStep::RemovalReady`].

#![forbid(unsafe_code)]

#[cfg(target_arch = "riscv32")]
use crate::BluetoothPostUnlinkAwaiting;
use crate::BluetoothSchedulerFinishedHardwareListObserved;
use crate::scheduler::core::{
    BluetoothSchedulerFinishedListDrainPending, BluetoothSchedulerFinishedListDrainState,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::core::{
    BluetoothSingleItemSchedulerCompletionObserved,
    BluetoothSingleItemSchedulerHardwareHeadEmptyObserved, BluetoothSingleItemSchedulerRole,
    BluetoothSingleItemSchedulerRunning, BluetoothSingleItemSchedulerSoftwareListRemovalReady,
    BluetoothSingleItemSchedulerSoftwareListUnlinked,
};

pub(crate) trait BluetoothSingleItemCompletionRole {
    type Wake;
    type Running;
    type CompletionObserved;
    type HardwareHeadEmpty;
    type PostUnlinkAwaiting;
    type RemovalReady;
}

#[cfg(target_arch = "riscv32")]
impl<Role> BluetoothSingleItemCompletionRole for Role
where
    Role: BluetoothSingleItemSchedulerRole,
{
    type Wake = crate::BluetoothSchedulerWakeBatch;
    type Running = BluetoothSingleItemSchedulerRunning<Role>;
    type CompletionObserved = BluetoothSingleItemSchedulerCompletionObserved<Role>;
    type HardwareHeadEmpty = BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>;
    type PostUnlinkAwaiting =
        BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>;
    type RemovalReady = BluetoothSingleItemSchedulerSoftwareListRemovalReady<Role>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        dead_code,
        reason = "hardware completion backends construct the full fault taxonomy on ESP32-S31; host tests exercise the shared owner transitions"
    )
)]
pub(crate) enum BluetoothSingleItemCompletionFaultCause {
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

pub(crate) struct BluetoothSingleItemCompletionFault<Owner> {
    pub(crate) cause: BluetoothSingleItemCompletionFaultCause,
    pub(crate) _owner: Owner,
}

pub(crate) enum BluetoothSingleItemRunningProgress<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    Running(BluetoothSchedulerFinishedListDrainState<Role::Running>),
    CompletionObserved(BluetoothSchedulerFinishedListDrainState<Role::CompletionObserved>),
    UnrelatedList {
        drain: BluetoothSchedulerFinishedListDrainState<Role::Running>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
}

pub(crate) struct BluetoothSingleItemCompletedDrainProgress<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    pub(crate) drain: BluetoothSchedulerFinishedListDrainState<Role::CompletionObserved>,
    pub(crate) observed: BluetoothSchedulerFinishedHardwareListObserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothSingleItemPostUnlinkDisposition {
    Continue,
    Waiting,
}

pub(crate) enum BluetoothSingleItemPostUnlinkProgress<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    Pending {
        awaiting: Role::PostUnlinkAwaiting,
        disposition: BluetoothSingleItemPostUnlinkDisposition,
    },
    Ready(Role::RemovalReady),
}

pub(crate) trait BluetoothSingleItemCompletionBackend<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    type FaultOwner;

    fn take_scheduler_wake(&mut self) -> Option<Role::Wake>;

    fn observe_completion(
        &mut self,
        running: Role::Running,
        wake: Role::Wake,
    ) -> Result<
        BluetoothSingleItemRunningProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    >;

    fn continue_running_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<Role::Running>,
    ) -> Result<
        BluetoothSingleItemRunningProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    >;

    fn continue_completed_drain(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<Role::CompletionObserved>,
    ) -> Result<
        BluetoothSingleItemCompletedDrainProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    >;

    fn observe_hardware_head_retirement(
        &mut self,
        completed: Role::CompletionObserved,
    ) -> Result<Role::HardwareHeadEmpty, BluetoothSingleItemCompletionFault<Self::FaultOwner>>;

    fn unlink_and_arm(
        &mut self,
        observed: Role::HardwareHeadEmpty,
    ) -> Result<Role::PostUnlinkAwaiting, BluetoothSingleItemCompletionFault<Self::FaultOwner>>;

    fn consume_post_unlink(
        &mut self,
        awaiting: Role::PostUnlinkAwaiting,
    ) -> Result<
        BluetoothSingleItemPostUnlinkProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    >;
}

enum BluetoothSingleItemCompletionPhase<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    RunningAwaitingWake(Role::Running),
    RunningReady {
        running: Role::Running,
        wake: Role::Wake,
    },
    RunningDrain(BluetoothSchedulerFinishedListDrainPending<Role::Running>),
    CompletionDrain(BluetoothSchedulerFinishedListDrainPending<Role::CompletionObserved>),
    CompletionObserved(Role::CompletionObserved),
    HardwareHeadEmpty(Role::HardwareHeadEmpty),
    PostUnlinkAwaiting(Role::PostUnlinkAwaiting),
}

/// Executor-neutral owner of one single-item scheduler completion lifecycle.
pub(crate) struct BluetoothSingleItemCompletion<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    phase: BluetoothSingleItemCompletionPhase<Role>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothSingleItemCompletionWaitKind {
    Scheduler,
    PostUnlink,
}

pub(crate) enum BluetoothSingleItemCompletionStep<Role, FaultOwner>
where
    Role: BluetoothSingleItemCompletionRole,
{
    Continue(BluetoothSingleItemCompletion<Role>),
    Waiting(BluetoothSingleItemCompletion<Role>),
    UnrelatedList {
        completion: BluetoothSingleItemCompletion<Role>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    RemovalReady(Role::RemovalReady),
    Fault(BluetoothSingleItemCompletionFault<FaultOwner>),
}

impl<Role> BluetoothSingleItemCompletion<Role>
where
    Role: BluetoothSingleItemCompletionRole,
{
    pub(crate) const fn new(running: Role::Running) -> Self {
        Self {
            phase: BluetoothSingleItemCompletionPhase::RunningAwaitingWake(running),
        }
    }

    pub(crate) const fn wait_kind(&self) -> Option<BluetoothSingleItemCompletionWaitKind> {
        match &self.phase {
            BluetoothSingleItemCompletionPhase::RunningAwaitingWake(_) => {
                Some(BluetoothSingleItemCompletionWaitKind::Scheduler)
            }
            BluetoothSingleItemCompletionPhase::PostUnlinkAwaiting(_) => {
                Some(BluetoothSingleItemCompletionWaitKind::PostUnlink)
            }
            _ => None,
        }
    }

    pub(crate) fn step<Backend>(
        self,
        backend: &mut Backend,
    ) -> BluetoothSingleItemCompletionStep<Role, Backend::FaultOwner>
    where
        Backend: BluetoothSingleItemCompletionBackend<Role>,
    {
        match self.phase {
            BluetoothSingleItemCompletionPhase::RunningAwaitingWake(running) => {
                let Some(wake) = backend.take_scheduler_wake() else {
                    return BluetoothSingleItemCompletionStep::Waiting(Self::new(running));
                };
                BluetoothSingleItemCompletionStep::Continue(Self {
                    phase: BluetoothSingleItemCompletionPhase::RunningReady { running, wake },
                })
            }
            BluetoothSingleItemCompletionPhase::RunningReady { running, wake } => {
                match backend.observe_completion(running, wake) {
                    Ok(progress) => Self::from_running_progress(progress),
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
            BluetoothSingleItemCompletionPhase::RunningDrain(pending) => {
                match backend.continue_running_drain(pending) {
                    Ok(progress) => Self::from_running_progress(progress),
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
            BluetoothSingleItemCompletionPhase::CompletionDrain(pending) => {
                match backend.continue_completed_drain(pending) {
                    Ok(progress) => {
                        let completion = Self::from_completed_drain(progress.drain);
                        BluetoothSingleItemCompletionStep::UnrelatedList {
                            completion,
                            observed: progress.observed,
                        }
                    }
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
            BluetoothSingleItemCompletionPhase::CompletionObserved(completed) => {
                match backend.observe_hardware_head_retirement(completed) {
                    Ok(observed) => BluetoothSingleItemCompletionStep::Continue(Self {
                        phase: BluetoothSingleItemCompletionPhase::HardwareHeadEmpty(observed),
                    }),
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
            BluetoothSingleItemCompletionPhase::HardwareHeadEmpty(observed) => {
                match backend.unlink_and_arm(observed) {
                    Ok(awaiting) => BluetoothSingleItemCompletionStep::Continue(Self {
                        phase: BluetoothSingleItemCompletionPhase::PostUnlinkAwaiting(awaiting),
                    }),
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
            BluetoothSingleItemCompletionPhase::PostUnlinkAwaiting(awaiting) => {
                match backend.consume_post_unlink(awaiting) {
                    Ok(BluetoothSingleItemPostUnlinkProgress::Pending {
                        awaiting,
                        disposition,
                    }) => {
                        let completion = Self {
                            phase: BluetoothSingleItemCompletionPhase::PostUnlinkAwaiting(awaiting),
                        };
                        match disposition {
                            BluetoothSingleItemPostUnlinkDisposition::Continue => {
                                BluetoothSingleItemCompletionStep::Continue(completion)
                            }
                            BluetoothSingleItemPostUnlinkDisposition::Waiting => {
                                BluetoothSingleItemCompletionStep::Waiting(completion)
                            }
                        }
                    }
                    Ok(BluetoothSingleItemPostUnlinkProgress::Ready(ready)) => {
                        BluetoothSingleItemCompletionStep::RemovalReady(ready)
                    }
                    Err(fault) => BluetoothSingleItemCompletionStep::Fault(fault),
                }
            }
        }
    }

    fn from_running_progress<FaultOwner>(
        progress: BluetoothSingleItemRunningProgress<Role>,
    ) -> BluetoothSingleItemCompletionStep<Role, FaultOwner> {
        match progress {
            BluetoothSingleItemRunningProgress::Running(drain) => match drain {
                BluetoothSchedulerFinishedListDrainState::Drained(running) => {
                    BluetoothSingleItemCompletionStep::Waiting(Self::new(running))
                }
                BluetoothSchedulerFinishedListDrainState::Pending(pending) => {
                    BluetoothSingleItemCompletionStep::Continue(Self {
                        phase: BluetoothSingleItemCompletionPhase::RunningDrain(pending),
                    })
                }
            },
            BluetoothSingleItemRunningProgress::CompletionObserved(drain) => {
                BluetoothSingleItemCompletionStep::Continue(Self::from_completed_drain(drain))
            }
            BluetoothSingleItemRunningProgress::UnrelatedList { drain, observed } => {
                let completion = match drain {
                    BluetoothSchedulerFinishedListDrainState::Drained(running) => {
                        Self::new(running)
                    }
                    BluetoothSchedulerFinishedListDrainState::Pending(pending) => Self {
                        phase: BluetoothSingleItemCompletionPhase::RunningDrain(pending),
                    },
                };
                BluetoothSingleItemCompletionStep::UnrelatedList {
                    completion,
                    observed,
                }
            }
        }
    }

    fn from_completed_drain(
        drain: BluetoothSchedulerFinishedListDrainState<Role::CompletionObserved>,
    ) -> Self {
        match drain {
            BluetoothSchedulerFinishedListDrainState::Drained(completed) => Self {
                phase: BluetoothSingleItemCompletionPhase::CompletionObserved(completed),
            },
            BluetoothSchedulerFinishedListDrainState::Pending(pending) => Self {
                phase: BluetoothSingleItemCompletionPhase::CompletionDrain(pending),
            },
        }
    }
}

#[cfg(test)]
mod tests;
