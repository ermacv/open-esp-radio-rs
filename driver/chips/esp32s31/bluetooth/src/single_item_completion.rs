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
use crate::scheduler::{
    BluetoothSchedulerFinishedListDrainPending, BluetoothSchedulerFinishedListDrainState,
};
#[cfg(target_arch = "riscv32")]
use crate::scheduler::{
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
mod tests {
    use super::*;
    use std::rc::Rc;

    struct Role;

    impl BluetoothSingleItemCompletionRole for Role {
        type Wake = ();
        type Running = u8;
        type CompletionObserved = u16;
        type HardwareHeadEmpty = u32;
        type PostUnlinkAwaiting = u64;
        type RemovalReady = usize;
    }

    #[derive(Default)]
    struct Backend {
        wake: bool,
        still_running_once: bool,
        post_unlink_pending: bool,
    }

    impl BluetoothSingleItemCompletionBackend<Role> for Backend {
        type FaultOwner = ();

        fn take_scheduler_wake(&mut self) -> Option<()> {
            core::mem::replace(&mut self.wake, false).then_some(())
        }

        fn observe_completion(
            &mut self,
            running: u8,
            (): (),
        ) -> Result<BluetoothSingleItemRunningProgress<Role>, BluetoothSingleItemCompletionFault<()>>
        {
            if self.still_running_once {
                self.still_running_once = false;
                return Ok(BluetoothSingleItemRunningProgress::Running(
                    BluetoothSchedulerFinishedListDrainState::Drained(running),
                ));
            }
            Ok(BluetoothSingleItemRunningProgress::CompletionObserved(
                BluetoothSchedulerFinishedListDrainState::Drained(u16::from(running)),
            ))
        }

        fn continue_running_drain(
            &mut self,
            _pending: BluetoothSchedulerFinishedListDrainPending<u8>,
        ) -> Result<BluetoothSingleItemRunningProgress<Role>, BluetoothSingleItemCompletionFault<()>>
        {
            unreachable!("the model completes in its first captured list")
        }

        fn continue_completed_drain(
            &mut self,
            _pending: BluetoothSchedulerFinishedListDrainPending<u16>,
        ) -> Result<
            BluetoothSingleItemCompletedDrainProgress<Role>,
            BluetoothSingleItemCompletionFault<()>,
        > {
            unreachable!("the model completes with an exhausted capture")
        }

        fn observe_hardware_head_retirement(
            &mut self,
            completed: u16,
        ) -> Result<u32, BluetoothSingleItemCompletionFault<()>> {
            Ok(u32::from(completed))
        }

        fn unlink_and_arm(
            &mut self,
            observed: u32,
        ) -> Result<u64, BluetoothSingleItemCompletionFault<()>> {
            Ok(u64::from(observed))
        }

        fn consume_post_unlink(
            &mut self,
            awaiting: u64,
        ) -> Result<
            BluetoothSingleItemPostUnlinkProgress<Role>,
            BluetoothSingleItemCompletionFault<()>,
        > {
            if self.post_unlink_pending {
                self.post_unlink_pending = false;
                return Ok(BluetoothSingleItemPostUnlinkProgress::Pending {
                    awaiting,
                    disposition: BluetoothSingleItemPostUnlinkDisposition::Waiting,
                });
            }
            Ok(BluetoothSingleItemPostUnlinkProgress::Ready(
                awaiting as usize,
            ))
        }
    }

    #[test]
    fn common_spine_waits_then_advances_each_owner_to_removal_ready() {
        let mut backend = Backend {
            post_unlink_pending: true,
            ..Backend::default()
        };
        let completion = BluetoothSingleItemCompletion::<Role>::new(9);
        assert_eq!(
            completion.wait_kind(),
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler)
        );
        let BluetoothSingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend)
        else {
            panic!("an absent wake must preserve the running owner");
        };

        backend.wake = true;
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the scheduler wake must remain paired with the running owner");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the completion hook must retain the observed owner");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("head retirement must retain the empty-head owner");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("unlink must retain the armed post-unlink owner");
        };
        assert_eq!(
            completion.wait_kind(),
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink)
        );
        let BluetoothSingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend)
        else {
            panic!("post-unlink backpressure must retain the exact awaiting owner");
        };
        assert_eq!(
            completion.wait_kind(),
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink)
        );
        let BluetoothSingleItemCompletionStep::RemovalReady(ready) = completion.step(&mut backend)
        else {
            panic!("the matching post-unlink publication must expose removal readiness");
        };
        assert_eq!(ready, 9);
    }

    #[test]
    fn role_hook_can_keep_the_item_running_without_losing_its_owner() {
        let mut backend = Backend {
            wake: true,
            still_running_once: true,
            ..Backend::default()
        };
        let completion = BluetoothSingleItemCompletion::<Role>::new(13);
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the first wake must remain paired with the running owner");
        };
        let BluetoothSingleItemCompletionStep::Waiting(completion) = completion.step(&mut backend)
        else {
            panic!("a role-level still-running observation must await another wake");
        };
        assert_eq!(
            completion.wait_kind(),
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler)
        );

        backend.wake = true;
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the next wake must retain the same role owner");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the second observation must complete the scripted role item");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the completed owner must advance to empty-head observation");
        };
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the empty-head owner must advance to the post-unlink gate");
        };
        let BluetoothSingleItemCompletionStep::RemovalReady(ready) = completion.step(&mut backend)
        else {
            panic!("the post-unlink gate must return the exact role owner");
        };
        assert_eq!(ready, 13);
    }

    struct IdentityMismatchBackend {
        wake: bool,
        owner: Option<Rc<()>>,
    }

    impl BluetoothSingleItemCompletionBackend<Role> for IdentityMismatchBackend {
        type FaultOwner = Rc<()>;

        fn take_scheduler_wake(&mut self) -> Option<()> {
            core::mem::replace(&mut self.wake, false).then_some(())
        }

        fn observe_completion(
            &mut self,
            _running: u8,
            (): (),
        ) -> Result<
            BluetoothSingleItemRunningProgress<Role>,
            BluetoothSingleItemCompletionFault<Self::FaultOwner>,
        > {
            let Some(owner) = self.owner.take() else {
                panic!("the scripted mismatch owns exactly one affine token");
            };
            Err(BluetoothSingleItemCompletionFault {
                cause: BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                _owner: owner,
            })
        }

        fn continue_running_drain(
            &mut self,
            _pending: BluetoothSchedulerFinishedListDrainPending<u8>,
        ) -> Result<
            BluetoothSingleItemRunningProgress<Role>,
            BluetoothSingleItemCompletionFault<Self::FaultOwner>,
        > {
            unreachable!("the scripted mismatch occurs during initial classification")
        }

        fn continue_completed_drain(
            &mut self,
            _pending: BluetoothSchedulerFinishedListDrainPending<u16>,
        ) -> Result<
            BluetoothSingleItemCompletedDrainProgress<Role>,
            BluetoothSingleItemCompletionFault<Self::FaultOwner>,
        > {
            unreachable!("the scripted mismatch occurs before completion drain")
        }

        fn observe_hardware_head_retirement(
            &mut self,
            _completed: u16,
        ) -> Result<u32, BluetoothSingleItemCompletionFault<Self::FaultOwner>> {
            unreachable!("the scripted mismatch occurs before head retirement")
        }

        fn unlink_and_arm(
            &mut self,
            _observed: u32,
        ) -> Result<u64, BluetoothSingleItemCompletionFault<Self::FaultOwner>> {
            unreachable!("the scripted mismatch occurs before unlink")
        }

        fn consume_post_unlink(
            &mut self,
            _awaiting: u64,
        ) -> Result<
            BluetoothSingleItemPostUnlinkProgress<Role>,
            BluetoothSingleItemCompletionFault<Self::FaultOwner>,
        > {
            unreachable!("the scripted mismatch occurs before post-unlink")
        }
    }

    #[test]
    fn identity_mismatch_preserves_the_exact_backend_owner() {
        let identity = Rc::new(());
        let mut backend = IdentityMismatchBackend {
            wake: true,
            owner: Some(Rc::clone(&identity)),
        };
        let completion = BluetoothSingleItemCompletion::<Role>::new(21);
        let BluetoothSingleItemCompletionStep::Continue(completion) = completion.step(&mut backend)
        else {
            panic!("the wake must remain paired with the running owner");
        };
        let BluetoothSingleItemCompletionStep::Fault(fault) = completion.step(&mut backend) else {
            panic!("the scripted scheduler identity mismatch must fail closed");
        };
        assert_eq!(
            fault.cause,
            BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch
        );
        assert!(Rc::ptr_eq(&fault._owner, &identity));
    }
}
