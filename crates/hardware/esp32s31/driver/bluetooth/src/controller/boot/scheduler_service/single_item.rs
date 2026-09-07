//! Controller composition for the role-neutral single-item completion owner.

#![forbid(unsafe_code)]

use crate::{
    interrupt::{
        PrimaryControllerFault, PrimaryNoSchedulerWork, PrimarySchedulerEvent, SchedulerWakeBatch,
    },
    le::dtm::BluetoothPostUnlinkAwaiting,
    scheduler::{
        SchedulerFinishedListDrainPending,
        completion::{
            SingleItemCompletedDrainProgress, SingleItemCompletionBackend,
            SingleItemCompletionFault, SingleItemCompletionFaultCause,
            SingleItemPostUnlinkDisposition, SingleItemPostUnlinkProgress,
            SingleItemRunningProgress,
        },
        core::{
            SingleItemSchedulerCompletionObserved, SingleItemSchedulerCompletionObservedDrainStep,
            SingleItemSchedulerCompletionStep, SingleItemSchedulerHardwareHeadEmptyObserved,
            SingleItemSchedulerHardwareHeadRetirementStep,
            SingleItemSchedulerRemovalTransitionMismatch, SingleItemSchedulerRole,
            SingleItemSchedulerRunning, SingleItemSchedulerRunningDrainStep,
            SingleItemSchedulerSoftwareListRemovalJoin,
            SingleItemSchedulerSoftwareListRemovalReady,
            SingleItemSchedulerSoftwareListRemovalRecheck,
            SingleItemSchedulerSoftwareListUnlinkStep, SingleItemSchedulerSoftwareListUnlinked,
        },
    },
};

use super::super::{
    ControllerPublishedTaskService, DtmPostUnlinkArmError, PostUnlinkRearm, PostUnlinkTake,
    PrimaryPublishedInterruptStep, SchedulerRunInterruptStorage,
};

pub(crate) enum SingleItemPostUnlinkArmStep<Role: SingleItemSchedulerRole> {
    MailboxBusy(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxIdentityExhausted(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    GenerationExhausted(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    SchedulerIdentityMismatch(SingleItemSchedulerHardwareHeadEmptyObserved<Role>),
    MailboxCommitMismatch(SingleItemSchedulerSoftwareListUnlinked<Role>),
    Armed(BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>),
}

pub(crate) enum SingleItemSoftwareListRemovalPublishedStep<Role: SingleItemSchedulerRole> {
    MailboxAffinityMismatch(
        BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    ),
    Fault {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
        _fault: PrimaryControllerFault,
    },
    NoSchedulerWork {
        awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
        _epoch: PrimaryNoSchedulerWork,
    },
    PublishedPending {
        awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    DirectPending {
        awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    RecheckUnavailable {
        _awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    },
    NoSchedulerWorkRearmMismatch {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
        _epoch: PrimaryNoSchedulerWork,
    },
    PendingRearmMismatch {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    RecheckRearmMismatch {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    SchedulerIdentityMismatch {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
        _event: PrimarySchedulerEvent,
    },
    DirectSchedulerIdentityMismatch {
        _unlinked: SingleItemSchedulerSoftwareListUnlinked<Role>,
    },
    SchedulerStateMismatch(SingleItemSchedulerRemovalTransitionMismatch<Role>),
    Ready {
        ready: SingleItemSchedulerSoftwareListRemovalReady<Role>,
    },
}

impl<'runtime, S, const CAPACITY: usize> ControllerPublishedTaskService<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn observe_single_item_completion<Role: SingleItemSchedulerRole>(
        &mut self,
        running: SingleItemSchedulerRunning<Role>,
        wake: SchedulerWakeBatch,
    ) -> SingleItemSchedulerCompletionStep<Role> {
        self.runtime.observe_single_item_completion(running, wake)
    }

    pub(crate) fn continue_single_item_running_finished_list_drain<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        pending: SchedulerFinishedListDrainPending<SingleItemSchedulerRunning<Role>>,
    ) -> SingleItemSchedulerRunningDrainStep<Role> {
        self.runtime
            .continue_single_item_running_finished_list_drain(pending)
    }

    pub(crate) fn continue_single_item_completed_finished_list_drain<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        pending: SchedulerFinishedListDrainPending<SingleItemSchedulerCompletionObserved<Role>>,
    ) -> SingleItemSchedulerCompletionObservedDrainStep<Role> {
        self.runtime
            .continue_single_item_completed_finished_list_drain(pending)
    }

    pub(crate) fn observe_single_item_hardware_head_retirement<Role: SingleItemSchedulerRole>(
        &mut self,
        completed: SingleItemSchedulerCompletionObserved<Role>,
    ) -> SingleItemSchedulerHardwareHeadRetirementStep<Role> {
        self.runtime
            .observe_single_item_hardware_head_retirement(completed)
    }

    pub(crate) fn unlink_and_arm_single_item_software_list_removal<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        observed: SingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> SingleItemPostUnlinkArmStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(DtmPostUnlinkArmError::Busy) => {
                    return SingleItemPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(DtmPostUnlinkArmError::IdentityExhausted) => {
                    return SingleItemPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                }
                Err(DtmPostUnlinkArmError::GenerationExhausted) => {
                    return SingleItemPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_single_item_software_list(observed) {
                SingleItemSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed) => {
                    SingleItemPostUnlinkArmStep::SchedulerIdentityMismatch(observed)
                }
                SingleItemSchedulerSoftwareListUnlinkStep::Unlinked(unlinked) => {
                    if mailbox.commit_arm(critical_section, key) {
                        SingleItemPostUnlinkArmStep::Armed(BluetoothPostUnlinkAwaiting::new(
                            unlinked, key,
                        ))
                    } else {
                        SingleItemPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    pub(crate) fn consume_published_single_item_software_list_removal<
        Role: SingleItemSchedulerRole,
    >(
        &mut self,
        awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    ) -> SingleItemSoftwareListRemovalPublishedStep<Role> {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                PostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime
                        .recheck_single_item_software_list_removal(storage, unlinked)
                    {
                        SingleItemSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked) => {
                            SingleItemSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { _unlinked: unlinked }
                        }
                        SingleItemSchedulerSoftwareListRemovalRecheck::StorageUnavailable(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                PostUnlinkRearm::Armed(awaiting) => SingleItemSoftwareListRemovalPublishedStep::RecheckUnavailable { _awaiting: awaiting },
                                PostUnlinkRearm::AffinityMismatch(unlinked) => SingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        SingleItemSchedulerSoftwareListRemovalRecheck::SchedulerStateMismatch(owner) => SingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(owner),
                        SingleItemSchedulerSoftwareListRemovalRecheck::Pending(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                PostUnlinkRearm::Armed(awaiting) => SingleItemSoftwareListRemovalPublishedStep::DirectPending { awaiting },
                                PostUnlinkRearm::AffinityMismatch(unlinked) => SingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        SingleItemSchedulerSoftwareListRemovalRecheck::Ready(ready) => SingleItemSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                PostUnlinkTake::AffinityMismatch(awaiting) => {
                    return SingleItemSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                        awaiting,
                    );
                }
                PostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                PrimaryPublishedInterruptStep::Fault(fault) => {
                    SingleItemSoftwareListRemovalPublishedStep::Fault {
                        _unlinked: unlinked,
                        _fault: fault,
                    }
                }
                PrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        PostUnlinkRearm::Armed(awaiting) => SingleItemSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, _epoch: epoch },
                        PostUnlinkRearm::AffinityMismatch(unlinked) => SingleItemSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { _unlinked: unlinked, _epoch: epoch },
                    }
                }
                PrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_single_item_software_list_removal(unlinked, event) {
                        SingleItemSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch { unlinked, event } => SingleItemSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { _unlinked: unlinked, _event: event },
                        SingleItemSchedulerSoftwareListRemovalJoin::SchedulerStateMismatch(owner) => SingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(owner),
                        SingleItemSchedulerSoftwareListRemovalJoin::Pending(unlinked) => {
                            match mailbox.rearm(critical_section, key, unlinked) {
                                PostUnlinkRearm::Armed(awaiting) => SingleItemSoftwareListRemovalPublishedStep::PublishedPending { awaiting },
                                PostUnlinkRearm::AffinityMismatch(unlinked) => SingleItemSoftwareListRemovalPublishedStep::PendingRearmMismatch { _unlinked: unlinked },
                            }
                        }
                        SingleItemSchedulerSoftwareListRemovalJoin::Ready(ready) => SingleItemSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }
}

pub(crate) enum SingleItemSchedulerCompletionFaultOwner<Role: SingleItemSchedulerRole> {
    Completion(SingleItemSchedulerCompletionStep<Role>),
    RunningDrain(SingleItemSchedulerRunningDrainStep<Role>),
    CompletionDrain(SingleItemSchedulerCompletionObservedDrainStep<Role>),
    HardwareHeadRetirement(SingleItemSchedulerHardwareHeadRetirementStep<Role>),
    PostUnlinkArm(SingleItemPostUnlinkArmStep<Role>),
    PostUnlinkPublished(SingleItemSoftwareListRemovalPublishedStep<Role>),
}

fn completion_fault<Role: SingleItemSchedulerRole>(
    cause: SingleItemCompletionFaultCause,
    owner: SingleItemSchedulerCompletionFaultOwner<Role>,
) -> SingleItemCompletionFault<SingleItemSchedulerCompletionFaultOwner<Role>> {
    SingleItemCompletionFault {
        cause,
        _owner: owner,
    }
}

impl<Role, S, const CAPACITY: usize> SingleItemCompletionBackend<Role>
    for ControllerPublishedTaskService<'_, S, CAPACITY>
where
    Role: SingleItemSchedulerRole,
    S: SchedulerRunInterruptStorage,
{
    type FaultOwner = SingleItemSchedulerCompletionFaultOwner<Role>;

    fn take_scheduler_wake(&mut self) -> Option<SchedulerWakeBatch> {
        self.scheduler_wake().take()
    }

    fn observe_completion(
        &mut self,
        running: SingleItemSchedulerRunning<Role>,
        wake: SchedulerWakeBatch,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>> {
        match self.observe_single_item_completion(running, wake) {
            SingleItemSchedulerCompletionStep::NoFinishedList(running) => {
                Ok(SingleItemRunningProgress::Running(
                    crate::scheduler::SchedulerFinishedListDrainState::Drained(running),
                ))
            }
            SingleItemSchedulerCompletionStep::UnrelatedList { drain, observed } => {
                Ok(SingleItemRunningProgress::UnrelatedList { drain, observed })
            }
            SingleItemSchedulerCompletionStep::StillInFlight(drain) => {
                Ok(SingleItemRunningProgress::Running(drain))
            }
            SingleItemSchedulerCompletionStep::CompletionObserved(drain) => {
                Ok(SingleItemRunningProgress::CompletionObserved(drain))
            }
            step @ SingleItemSchedulerCompletionStep::DrainAlreadyActive(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::FinishedListDrainAlreadyActive,
                    SingleItemSchedulerCompletionFaultOwner::Completion(step),
                ))
            }
            step @ SingleItemSchedulerCompletionStep::SchedulerIdentityMismatch(_)
            | step @ SingleItemSchedulerCompletionStep::RoleItemIdentityMismatch(_)
            | step @ SingleItemSchedulerCompletionStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::Completion(step),
                ))
            }
        }
    }

    fn continue_running_drain(
        &mut self,
        pending: crate::scheduler::SchedulerFinishedListDrainPending<
            SingleItemSchedulerRunning<Role>,
        >,
    ) -> Result<SingleItemRunningProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>> {
        match self.continue_single_item_running_finished_list_drain(pending) {
            SingleItemSchedulerRunningDrainStep::UnrelatedList { drain, observed } => {
                Ok(SingleItemRunningProgress::UnrelatedList { drain, observed })
            }
            SingleItemSchedulerRunningDrainStep::StillInFlight(drain) => {
                Ok(SingleItemRunningProgress::Running(drain))
            }
            SingleItemSchedulerRunningDrainStep::CompletionObserved(drain) => {
                Ok(SingleItemRunningProgress::CompletionObserved(drain))
            }
            step @ SingleItemSchedulerRunningDrainStep::SchedulerIdentityMismatch(_)
            | step @ SingleItemSchedulerRunningDrainStep::RoleItemIdentityMismatch(_)
            | step @ SingleItemSchedulerRunningDrainStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::RunningDrain(step),
                ))
            }
            step @ SingleItemSchedulerRunningDrainStep::DrainLost(_) => Err(completion_fault(
                SingleItemCompletionFaultCause::FinishedListDrainLost,
                SingleItemSchedulerCompletionFaultOwner::RunningDrain(step),
            )),
        }
    }

    fn continue_completed_drain(
        &mut self,
        pending: crate::scheduler::SchedulerFinishedListDrainPending<
            SingleItemSchedulerCompletionObserved<Role>,
        >,
    ) -> Result<SingleItemCompletedDrainProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>
    {
        match self.continue_single_item_completed_finished_list_drain(pending) {
            SingleItemSchedulerCompletionObservedDrainStep::UnrelatedList { drain, observed } => {
                Ok(SingleItemCompletedDrainProgress { drain, observed })
            }
            step @ SingleItemSchedulerCompletionObservedDrainStep::SchedulerIdentityMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
                ))
            }
            step @ SingleItemSchedulerCompletionObservedDrainStep::DrainLost(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::FinishedListDrainLost,
                    SingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
                ))
            }
            step @ SingleItemSchedulerCompletionObservedDrainStep::RepeatedRoleList { .. } => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::RepeatedRoleList,
                    SingleItemSchedulerCompletionFaultOwner::CompletionDrain(step),
                ))
            }
        }
    }

    fn observe_hardware_head_retirement(
        &mut self,
        completed: SingleItemSchedulerCompletionObserved<Role>,
    ) -> Result<
        SingleItemSchedulerHardwareHeadEmptyObserved<Role>,
        SingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.observe_single_item_hardware_head_retirement(completed) {
            SingleItemSchedulerHardwareHeadRetirementStep::EmptyObserved(observed) => Ok(observed),
            step @ SingleItemSchedulerHardwareHeadRetirementStep::SchedulerIdentityMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
                ))
            }
            step @ SingleItemSchedulerHardwareHeadRetirementStep::FinishedListDrainStillActive(
                _,
            ) => Err(completion_fault(
                SingleItemCompletionFaultCause::FinishedListDrainStillActive,
                SingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ SingleItemSchedulerHardwareHeadRetirementStep::ExpectedHeadStillPublished {
                ..
            } => Err(completion_fault(
                SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished,
                SingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ SingleItemSchedulerHardwareHeadRetirementStep::UnexpectedHeadChanged {
                ..
            } => Err(completion_fault(
                SingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged,
                SingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
            )),
            step @ SingleItemSchedulerHardwareHeadRetirementStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::HardwareHeadRetirement(step),
                ))
            }
        }
    }

    fn unlink_and_arm(
        &mut self,
        observed: SingleItemSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> Result<
        BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
        SingleItemCompletionFault<Self::FaultOwner>,
    > {
        match self.unlink_and_arm_single_item_software_list_removal(observed) {
            SingleItemPostUnlinkArmStep::Armed(awaiting) => Ok(awaiting),
            step @ SingleItemPostUnlinkArmStep::MailboxBusy(_) => Err(completion_fault(
                SingleItemCompletionFaultCause::PostUnlinkMailboxBusy,
                SingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
            )),
            step @ SingleItemPostUnlinkArmStep::MailboxIdentityExhausted(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
            step @ SingleItemPostUnlinkArmStep::GenerationExhausted(_) => Err(completion_fault(
                SingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted,
                SingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
            )),
            step @ SingleItemPostUnlinkArmStep::SchedulerIdentityMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
                ))
            }
            step @ SingleItemPostUnlinkArmStep::MailboxCommitMismatch(_) => Err(completion_fault(
                SingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch,
                SingleItemSchedulerCompletionFaultOwner::PostUnlinkArm(step),
            )),
        }
    }

    fn consume_post_unlink(
        &mut self,
        awaiting: BluetoothPostUnlinkAwaiting<SingleItemSchedulerSoftwareListUnlinked<Role>>,
    ) -> Result<SingleItemPostUnlinkProgress<Role>, SingleItemCompletionFault<Self::FaultOwner>>
    {
        match self.consume_published_single_item_software_list_removal(awaiting) {
            SingleItemSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, .. }
            | SingleItemSoftwareListRemovalPublishedStep::PublishedPending { awaiting } => {
                Ok(SingleItemPostUnlinkProgress::Pending {
                    awaiting,
                    disposition: SingleItemPostUnlinkDisposition::Continue,
                })
            }
            SingleItemSoftwareListRemovalPublishedStep::DirectPending { awaiting } => {
                Ok(SingleItemPostUnlinkProgress::Pending {
                    awaiting,
                    disposition: SingleItemPostUnlinkDisposition::Waiting,
                })
            }
            SingleItemSoftwareListRemovalPublishedStep::Ready { ready } => {
                Ok(SingleItemPostUnlinkProgress::Ready(ready))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::Fault { .. } => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PrimaryInterruptFault,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                ..
            } => Err(completion_fault(
                SingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch,
                SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
            )),
            step @ SingleItemSoftwareListRemovalPublishedStep::PendingRearmMismatch { .. } => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::RecheckUnavailable { .. } => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::RecheckRearmMismatch { .. } => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
            step @ SingleItemSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                ..
            }
            | step
            @ SingleItemSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                ..
            }
            | step @ SingleItemSoftwareListRemovalPublishedStep::SchedulerStateMismatch(_) => {
                Err(completion_fault(
                    SingleItemCompletionFaultCause::SchedulerIdentityMismatch,
                    SingleItemSchedulerCompletionFaultOwner::PostUnlinkPublished(step),
                ))
            }
        }
    }
}
