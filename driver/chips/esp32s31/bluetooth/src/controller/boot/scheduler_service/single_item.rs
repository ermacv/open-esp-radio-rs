//! Controller composition for the role-neutral single-item completion owner.

#![forbid(unsafe_code)]

use super::super::{
    BluetoothControllerPublishedTaskService, BluetoothDtmPostUnlinkArmError,
    BluetoothPostUnlinkRearm, BluetoothPostUnlinkTake, BluetoothPrimaryPublishedInterruptStep,
    BluetoothSchedulerRunInterruptStorage,
};
use crate::scheduler::completion::{
    BluetoothSingleItemCompletedDrainProgress, BluetoothSingleItemCompletionBackend,
    BluetoothSingleItemCompletionFault, BluetoothSingleItemCompletionFaultCause,
    BluetoothSingleItemPostUnlinkDisposition, BluetoothSingleItemPostUnlinkProgress,
    BluetoothSingleItemRunningProgress,
};
use crate::scheduler::core::{
    BluetoothSingleItemSchedulerCompletionObserved,
    BluetoothSingleItemSchedulerCompletionObservedDrainStep,
    BluetoothSingleItemSchedulerCompletionStep,
    BluetoothSingleItemSchedulerHardwareHeadEmptyObserved,
    BluetoothSingleItemSchedulerHardwareHeadRetirementStep,
    BluetoothSingleItemSchedulerRemovalTransitionMismatch, BluetoothSingleItemSchedulerRole,
    BluetoothSingleItemSchedulerRunning, BluetoothSingleItemSchedulerRunningDrainStep,
    BluetoothSingleItemSchedulerSoftwareListRemovalJoin,
    BluetoothSingleItemSchedulerSoftwareListRemovalReady,
    BluetoothSingleItemSchedulerSoftwareListRemovalRecheck,
    BluetoothSingleItemSchedulerSoftwareListUnlinkStep,
    BluetoothSingleItemSchedulerSoftwareListUnlinked,
};
use crate::{
    BluetoothPostUnlinkAwaiting, BluetoothPrimaryControllerFault, BluetoothPrimaryNoSchedulerWork,
    BluetoothPrimarySchedulerEvent, BluetoothSchedulerFinishedListDrainPending,
    BluetoothSchedulerWakeBatch,
};

pub(crate) enum BluetoothSingleItemPostUnlinkArmStep<Role: BluetoothSingleItemSchedulerRole> {
    MailboxBusy(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxIdentityExhausted(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    GenerationExhausted(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    SchedulerIdentityMismatch(BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxCommitMismatch(BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>),
    Armed(BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>),
}

pub(crate) enum BluetoothSingleItemSoftwareListRemovalPublishedStep<
    Role: BluetoothSingleItemSchedulerRole,
> {
    MailboxAffinityMismatch(
        BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
    ),
    Fault {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        _fault: BluetoothPrimaryControllerFault,
    },
    NoSchedulerWork {
        awaiting:
            BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
        _epoch: BluetoothPrimaryNoSchedulerWork,
    },
    PublishedPending {
        awaiting:
            BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    DirectPending {
        awaiting:
            BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    RecheckUnavailable {
        _awaiting:
            BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    NoSchedulerWorkRearmMismatch {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        _epoch: BluetoothPrimaryNoSchedulerWork,
    },
    PendingRearmMismatch {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    RecheckRearmMismatch {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    SchedulerIdentityMismatch {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        _event: BluetoothPrimarySchedulerEvent,
    },
    DirectSchedulerIdentityMismatch {
        _unlinked: BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    SchedulerStateMismatch(BluetoothSingleItemSchedulerRemovalTransitionMismatch<Role>),
    Ready {
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<Role>,
    },
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn observe_single_item_completion<Role: BluetoothSingleItemSchedulerRole>(
        &mut self,
        running: BluetoothSingleItemSchedulerRunning<Role>,
        wake: BluetoothSchedulerWakeBatch,
    ) -> BluetoothSingleItemSchedulerCompletionStep<Role> {
        self.runtime.observe_single_item_completion(running, wake)
    }

    pub(crate) fn continue_single_item_running_finished_list_drain<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerRunning<Role>,
        >,
    ) -> BluetoothSingleItemSchedulerRunningDrainStep<Role> {
        self.runtime
            .continue_single_item_running_finished_list_drain(pending)
    }

    pub(crate) fn continue_single_item_completed_finished_list_drain<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        pending: BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ) -> BluetoothSingleItemSchedulerCompletionObservedDrainStep<Role> {
        self.runtime
            .continue_single_item_completed_finished_list_drain(pending)
    }

    pub(crate) fn observe_single_item_hardware_head_retirement<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        completed: BluetoothSingleItemSchedulerCompletionObserved<Role>,
    ) -> BluetoothSingleItemSchedulerHardwareHeadRetirementStep<Role> {
        self.runtime
            .observe_single_item_hardware_head_retirement(completed)
    }

    pub(crate) fn unlink_and_arm_single_item_software_list_removal<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        observed: BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> BluetoothSingleItemPostUnlinkArmStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothSingleItemPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothSingleItemPostUnlinkArmStep::MailboxIdentityExhausted(
                        observed,
                    );
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothSingleItemPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_single_item_software_list(observed) {
                BluetoothSingleItemSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                    observed,
                ) => BluetoothSingleItemPostUnlinkArmStep::SchedulerIdentityMismatch(observed),
                BluetoothSingleItemSchedulerSoftwareListUnlinkStep::Unlinked(unlinked) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothSingleItemPostUnlinkArmStep::Armed(
                            BluetoothPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothSingleItemPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    pub(crate) fn consume_published_single_item_software_list_removal<
        Role: BluetoothSingleItemSchedulerRole,
    >(
        &mut self,
        awaiting: BluetoothPostUnlinkAwaiting<
            BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        >,
    ) -> BluetoothSingleItemSoftwareListRemovalPublishedStep<Role> {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                BluetoothPostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime
                        .recheck_single_item_software_list_removal(storage, unlinked)
                    {
                        BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked) => {
                            BluetoothSingleItemSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { _unlinked: unlinked }
                        }
                        BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::StorageUnavailable(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                BluetoothPostUnlinkRearm::Armed(awaiting) => BluetoothSingleItemSoftwareListRemovalPublishedStep::RecheckUnavailable { _awaiting: awaiting },
                                BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothSingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::SchedulerStateMismatch(owner) => BluetoothSingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(owner),
                        BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::Pending(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                BluetoothPostUnlinkRearm::Armed(awaiting) => BluetoothSingleItemSoftwareListRemovalPublishedStep::DirectPending { awaiting },
                                BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothSingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        BluetoothSingleItemSchedulerSoftwareListRemovalRecheck::Ready(ready) => BluetoothSingleItemSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                BluetoothPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothSingleItemSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting);
                }
                BluetoothPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothSingleItemSoftwareListRemovalPublishedStep::Fault {
                        _unlinked: unlinked,
                        _fault: fault,
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        BluetoothPostUnlinkRearm::Armed(awaiting) => BluetoothSingleItemSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, _epoch: epoch },
                        BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothSingleItemSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { _unlinked: unlinked, _epoch: epoch },
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_single_item_software_list_removal(unlinked, event) {
                        BluetoothSingleItemSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch { unlinked, event } => BluetoothSingleItemSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { _unlinked: unlinked, _event: event },
                        BluetoothSingleItemSchedulerSoftwareListRemovalJoin::SchedulerStateMismatch(owner) => BluetoothSingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(owner),
                        BluetoothSingleItemSchedulerSoftwareListRemovalJoin::Pending(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                BluetoothPostUnlinkRearm::Armed(awaiting) => BluetoothSingleItemSoftwareListRemovalPublishedStep::PublishedPending { awaiting },
                                BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothSingleItemSoftwareListRemovalPublishedStep::PendingRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        BluetoothSingleItemSchedulerSoftwareListRemovalJoin::Ready(ready) => BluetoothSingleItemSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }
}

pub(crate) enum BluetoothSingleItemSchedulerCompletionFaultOwner<
    Role: BluetoothSingleItemSchedulerRole,
> {
    Completion(BluetoothSingleItemSchedulerCompletionStep<Role>),
    RunningDrain(BluetoothSingleItemSchedulerRunningDrainStep<Role>),
    CompletionDrain(BluetoothSingleItemSchedulerCompletionObservedDrainStep<Role>),
    HardwareHeadRetirement(BluetoothSingleItemSchedulerHardwareHeadRetirementStep<Role>),
    PostUnlinkArm(BluetoothSingleItemPostUnlinkArmStep<Role>),
    PostUnlinkPublished(BluetoothSingleItemSoftwareListRemovalPublishedStep<Role>),
}

fn completion_fault<Role: BluetoothSingleItemSchedulerRole>(
    cause: BluetoothSingleItemCompletionFaultCause,
    owner: BluetoothSingleItemSchedulerCompletionFaultOwner<Role>,
) -> BluetoothSingleItemCompletionFault<BluetoothSingleItemSchedulerCompletionFaultOwner<Role>> {
    BluetoothSingleItemCompletionFault {
        cause,
        _owner: owner,
    }
}

impl<Role, S, const CAPACITY: usize> BluetoothSingleItemCompletionBackend<Role>
    for BluetoothControllerPublishedTaskService<'_, S, CAPACITY>
where
    Role: BluetoothSingleItemSchedulerRole,
    S: BluetoothSchedulerRunInterruptStorage,
{
    type FaultOwner = BluetoothSingleItemSchedulerCompletionFaultOwner<Role>;

    fn take_scheduler_wake(&mut self) -> Option<BluetoothSchedulerWakeBatch> {
        self.scheduler_wake().take()
    }

    fn observe_completion(
        &mut self,
        running: BluetoothSingleItemSchedulerRunning<Role>,
        wake: BluetoothSchedulerWakeBatch,
    ) -> Result<
        BluetoothSingleItemRunningProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.observe_single_item_completion(running, wake) {
            BluetoothSingleItemSchedulerCompletionStep::NoFinishedList(running) => {
                Ok(BluetoothSingleItemRunningProgress::Running(
                    crate::BluetoothSchedulerFinishedListDrainState::Drained(running),
                ))
            }
            BluetoothSingleItemSchedulerCompletionStep::UnrelatedList { drain, observed } => {
                Ok(BluetoothSingleItemRunningProgress::UnrelatedList { drain, observed })
            }
            BluetoothSingleItemSchedulerCompletionStep::StillInFlight(drain) => {
                Ok(BluetoothSingleItemRunningProgress::Running(drain))
            }
            BluetoothSingleItemSchedulerCompletionStep::CompletionObserved(drain) => Ok(
                BluetoothSingleItemRunningProgress::CompletionObserved(drain),
            ),
            step @ BluetoothSingleItemSchedulerCompletionStep::DrainAlreadyActive(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::FinishedListDrainAlreadyActive,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::Completion(step),
                ))
            }
            step @ BluetoothSingleItemSchedulerCompletionStep::SchedulerIdentityMismatch(_)
            | step @ BluetoothSingleItemSchedulerCompletionStep::RoleItemIdentityMismatch(_)
            | step @ BluetoothSingleItemSchedulerCompletionStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::Completion(step),
                ))
            }
        }
    }

    fn continue_running_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerRunning<Role>,
        >,
    ) -> Result<
        BluetoothSingleItemRunningProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.continue_single_item_running_finished_list_drain(pending) {
            BluetoothSingleItemSchedulerRunningDrainStep::UnrelatedList { drain, observed } => {
                Ok(BluetoothSingleItemRunningProgress::UnrelatedList { drain, observed })
            }
            BluetoothSingleItemSchedulerRunningDrainStep::StillInFlight(drain) => {
                Ok(BluetoothSingleItemRunningProgress::Running(drain))
            }
            BluetoothSingleItemSchedulerRunningDrainStep::CompletionObserved(drain) => Ok(
                BluetoothSingleItemRunningProgress::CompletionObserved(drain),
            ),
            step @ BluetoothSingleItemSchedulerRunningDrainStep::SchedulerIdentityMismatch(_)
            | step @ BluetoothSingleItemSchedulerRunningDrainStep::RoleItemIdentityMismatch(_)
            | step @ BluetoothSingleItemSchedulerRunningDrainStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::RunningDrain(step),
                ))
            }
            step @ BluetoothSingleItemSchedulerRunningDrainStep::DrainLost(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::RunningDrain(step),
                ))
            }
        }
    }

    fn continue_completed_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            BluetoothSingleItemSchedulerCompletionObserved<Role>,
        >,
    ) -> Result<
        BluetoothSingleItemCompletedDrainProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.continue_single_item_completed_finished_list_drain(pending) {
            BluetoothSingleItemSchedulerCompletionObservedDrainStep::UnrelatedList {
                drain,
                observed,
            } => Ok(BluetoothSingleItemCompletedDrainProgress { drain, observed }),
            step @ BluetoothSingleItemSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
            )),
            step @ BluetoothSingleItemSchedulerCompletionObservedDrainStep::DrainLost(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
                ))
            }
            step @ BluetoothSingleItemSchedulerCompletionObservedDrainStep::RepeatedRoleList {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::RepeatedRoleList,
                BluetoothSingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
            )),
        }
    }

    fn observe_hardware_head_retirement(
        &mut self,
        completed: BluetoothSingleItemSchedulerCompletionObserved<Role>,
    ) -> Result<
        BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.observe_single_item_hardware_head_retirement(completed) {
            BluetoothSingleItemSchedulerHardwareHeadRetirementStep::EmptyObserved(observed) => {
                Ok(observed)
            }
            step @ BluetoothSingleItemSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ BluetoothSingleItemSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::FinishedListDrainStillActive,
                BluetoothSingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ BluetoothSingleItemSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished,
                BluetoothSingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ BluetoothSingleItemSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged,
                BluetoothSingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ BluetoothSingleItemSchedulerHardwareHeadRetirementStep::SchedulerStateMismatch(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
        }
    }

    fn unlink_and_arm(
        &mut self,
        observed: BluetoothSingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> Result<
        BluetoothPostUnlinkAwaiting<BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.unlink_and_arm_single_item_software_list_removal(observed) {
            BluetoothSingleItemPostUnlinkArmStep::Armed(awaiting) => Ok(awaiting),
            step @ BluetoothSingleItemPostUnlinkArmStep::MailboxBusy(_) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxBusy,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
            )),
            step @ BluetoothSingleItemPostUnlinkArmStep::MailboxIdentityExhausted(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
            step @ BluetoothSingleItemPostUnlinkArmStep::GenerationExhausted(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
            step @ BluetoothSingleItemPostUnlinkArmStep::SchedulerIdentityMismatch(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
            step @ BluetoothSingleItemPostUnlinkArmStep::MailboxCommitMismatch(_) => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
        }
    }

    fn consume_post_unlink(
        &mut self,
        awaiting: BluetoothPostUnlinkAwaiting<
            BluetoothSingleItemSchedulerSoftwareListUnlinked<Role>,
        >,
    ) -> Result<
        BluetoothSingleItemPostUnlinkProgress<Role>,
        BluetoothSingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.consume_published_single_item_software_list_removal(awaiting) {
            BluetoothSingleItemSoftwareListRemovalPublishedStep::NoSchedulerWork {
                awaiting,
                ..
            }
            | BluetoothSingleItemSoftwareListRemovalPublishedStep::PublishedPending {
                awaiting,
            } => Ok(BluetoothSingleItemPostUnlinkProgress::Pending {
                awaiting,
                disposition: BluetoothSingleItemPostUnlinkDisposition::Continue,
            }),
            BluetoothSingleItemSoftwareListRemovalPublishedStep::DirectPending { awaiting } => {
                Ok(BluetoothSingleItemPostUnlinkProgress::Pending {
                    awaiting,
                    disposition: BluetoothSingleItemPostUnlinkDisposition::Waiting,
                })
            }
            BluetoothSingleItemSoftwareListRemovalPublishedStep::Ready { ready } => {
                Ok(BluetoothSingleItemPostUnlinkProgress::Ready(ready))
            }
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::Fault { .. } => {
                Err(completion_fault(
                    BluetoothSingleItemCompletionFaultCause::PrimaryInterruptFault,
                    BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::RecheckUnavailable {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                ..
            } => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                ..
            }
            | step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                ..
            }
            | step @ BluetoothSingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(
                _,
            ) => Err(completion_fault(
                BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                BluetoothSingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
        }
    }
}
