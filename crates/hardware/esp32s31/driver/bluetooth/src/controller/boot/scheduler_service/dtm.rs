//! Direct Test Mode scheduler service operations.

use super::super::{
    ControllerPublishedTaskService, DtmPostUnlinkArmError, DtmPostUnlinkArmStep,
    DtmSchedulerStartFailure, DtmSoftwareListRemovalPublishedStep, PostUnlinkRearm, PostUnlinkTake,
    PrimaryPublishedInterruptStep, SchedulerRunInterruptStorage,
};

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Admit one published DTM graph through the complete scheduler-run suffix.
    ///
    /// The exact order is dynamic interrupt preparation, synchronous BTMAC
    /// scheduler-event publication and the final RUN command. The returned
    /// state retains the graph and grants no CPU-side completion access.
    #[expect(
        clippy::result_large_err,
        reason = "a start rejection must return the complete affine published graph"
    )]
    pub(crate) fn start_dtm_scheduler<Role>(
        &mut self,
        head: crate::scheduler::DtmSchedulerHeadPublished<Role>,
    ) -> Result<crate::scheduler::DtmSchedulerRunning<Role>, DtmSchedulerStartFailure<Role, S::Error>>
    where
        S: SchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(DtmSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (item, publication) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::scheduler::DtmSchedulerRunning::new(item, run))
    }

    pub(crate) fn step_dtm_stop<Role>(
        &mut self,
        running: crate::scheduler::DtmSchedulerRunning<Role>,
        stop: oer_esp32s31_hal::bluetooth::BluetoothSchedulerStop,
    ) -> crate::scheduler::core::DtmSchedulerStopStep<Role>
    where
        S: SchedulerRunInterruptStorage,
    {
        critical_section::with(|_| self.runtime.step_dtm_stop(self.storage, running, stop))
    }

    /// Perform one fresh fenced completion-list transfer and immediately join
    /// its affine result to this exact running DTM graph.
    ///
    /// A non-sentinel item status advances only to completion-observed. The
    /// descriptor, packet and scheduler reservation remain hardware-owned
    /// until the later unlink and recycle transaction is complete.
    pub fn observe_dtm_completion<Role>(
        &mut self,
        running: crate::scheduler::DtmSchedulerRunning<Role>,
        wake: crate::interrupt::SchedulerWakeBatch,
    ) -> crate::scheduler::DtmSchedulerCompletionStep<Role> {
        self.runtime.observe_dtm_completion(running, wake)
    }

    /// Continue an already captured finished-list drain while the DTM graph
    /// remains running.
    ///
    /// The opaque input proves that the same capture retains another list. No
    /// new hardware transfer occurs, and one retained list is returned per
    /// call together with the unchanged running or newly completed graph.
    pub fn continue_dtm_running_finished_list_drain<Role>(
        &mut self,
        pending: crate::scheduler::SchedulerFinishedListDrainPending<
            crate::scheduler::DtmSchedulerRunning<Role>,
        >,
    ) -> crate::scheduler::DtmSchedulerRunningDrainStep<Role> {
        self.runtime
            .continue_dtm_running_finished_list_drain(pending)
    }

    /// Continue the captured finished-list drain after DTM completion was
    /// observed while unrelated list tokens remained.
    ///
    /// The opaque input proves affinity to that exact capture. This consumes no
    /// new hardware observation and returns every unrelated affine list token
    /// losslessly.
    pub fn continue_dtm_completed_finished_list_drain<Role>(
        &mut self,
        pending: crate::scheduler::SchedulerFinishedListDrainPending<
            crate::scheduler::DtmSchedulerCompletionObserved<Role>,
        >,
    ) -> crate::scheduler::DtmSchedulerCompletionObservedDrainStep<Role> {
        self.runtime
            .continue_dtm_completed_finished_list_drain(pending)
    }

    /// Observe the post-picker hardware-list retirement barrier.
    ///
    /// This operation never clears or republishes the head. It performs one
    /// fresh typed read with a trailing device fence. Any nonempty result is a
    /// fail-stop invariant violation; the affine owner is retained only for
    /// diagnostic shutdown handling, never for a polling retry.
    pub fn observe_dtm_hardware_head_retirement<Role>(
        &mut self,
        completed: crate::scheduler::DtmSchedulerCompletionObserved<Role>,
    ) -> crate::scheduler::DtmSchedulerHardwareHeadRetirementStep<Role> {
        self.runtime.observe_dtm_hardware_head_retirement(completed)
    }

    /// Remove the sole empty-head DTM item and arm its post-unlink mailbox in
    /// one serialization boundary.
    ///
    /// A primary service cannot run between the ownership-only unlink and the
    /// mailbox arm. A busy or exhausted mailbox rejects before unlinking.
    pub fn unlink_and_arm_dtm_software_list_removal<Role>(
        &mut self,
        observed: crate::scheduler::DtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> DtmPostUnlinkArmStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(DtmPostUnlinkArmError::Busy) => {
                    return DtmPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(DtmPostUnlinkArmError::IdentityExhausted) => {
                    return DtmPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                }
                Err(DtmPostUnlinkArmError::GenerationExhausted) => {
                    return DtmPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_dtm_software_list(observed) {
                crate::scheduler::core::DtmSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                    observed,
                ) => DtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed),
                crate::scheduler::core::DtmSchedulerSoftwareListUnlinkStep::Unlinked(
                    unlinked,
                ) => {
                    if mailbox.commit_arm(critical_section, key) {
                        DtmPostUnlinkArmStep::Armed(
                            crate::le::dtm::BluetoothPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        DtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    /// Consume the exact primary event stored for one armed post-unlink owner.
    ///
    /// Mailbox take, finite command-status reads and any pending re-arm remain
    /// inside the same serialization boundary used by primary service.
    pub fn consume_published_dtm_software_list_removal<Role>(
        &mut self,
        awaiting: crate::le::dtm::BluetoothPostUnlinkAwaiting<
            crate::scheduler::DtmSchedulerSoftwareListUnlinked<Role>,
        >,
    ) -> DtmSoftwareListRemovalPublishedStep<Role>
    where
        S: SchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                PostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime.recheck_dtm_software_list_removal(storage, unlinked) {
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                            unlinked,
                        ) => DtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                            unlinked,
                        },
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            PostUnlinkRearm::Armed(awaiting) => {
                                DtmSoftwareListRemovalPublishedStep::RecheckUnavailable {
                                    awaiting,
                                }
                            }
                            PostUnlinkRearm::AffinityMismatch(unlinked) => {
                                DtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalRecheck::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            PostUnlinkRearm::Armed(awaiting) => {
                                DtmSoftwareListRemovalPublishedStep::DirectPending {
                                    awaiting,
                                }
                            }
                            PostUnlinkRearm::AffinityMismatch(unlinked) => {
                                DtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalRecheck::Ready(
                            ready,
                        ) => DtmSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                PostUnlinkTake::AffinityMismatch(awaiting) => {
                    return DtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting);
                }
                PostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                PrimaryPublishedInterruptStep::Fault(fault) => {
                    DtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                }
                PrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        PostUnlinkRearm::Armed(awaiting) => {
                            DtmSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, epoch }
                        }
                        PostUnlinkRearm::AffinityMismatch(unlinked) => {
                            DtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                                unlinked,
                                epoch,
                            }
                        }
                    }
                }
                PrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_dtm_software_list_removal(unlinked, event) {
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        } => DtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        },
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalJoin::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            PostUnlinkRearm::Armed(awaiting) => {
                                DtmSoftwareListRemovalPublishedStep::PublishedPending {
                                    awaiting,
                                }
                            }
                            PostUnlinkRearm::AffinityMismatch(unlinked) => {
                                DtmSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::core::DtmSchedulerSoftwareListRemovalJoin::Ready(
                            ready,
                        ) => DtmSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }

    /// Return TX or RX-non-success completion ownership to source-owned CPU
    /// state after the exact removal-ready transition.
    ///
    /// RX success is rejected into its separate drain/account/re-arm method.
    pub fn recycle_dtm_completed<Role>(
        &mut self,
        ready: crate::scheduler::DtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> crate::scheduler::DtmSchedulerRecycleStep<Role> {
        self.runtime.recycle_dtm_completed(ready)
    }

    /// Drain, account and re-arm one successful removal-ready receiver event.
    ///
    /// The returned chain is validated before mutation. Every rejection keeps
    /// the exact graph/session owner; success releases memory, timeline and
    /// source-list ownership before exposing the re-armed session.
    pub fn recycle_dtm_receiver_success(
        &mut self,
        ready: crate::scheduler::DtmSchedulerSoftwareListRemovalReady<
            crate::le::dtm::DtmReceiverEvent,
        >,
    ) -> crate::scheduler::DtmSchedulerRxSuccessRecycleStep {
        self.runtime.recycle_dtm_receiver_success(ready)
    }
}
