//! Direct Test Mode scheduler service operations.

use super::super::{
    BluetoothControllerPublishedTaskService, BluetoothDtmPostUnlinkArmError,
    BluetoothDtmPostUnlinkArmStep, BluetoothDtmSchedulerStartFailure,
    BluetoothDtmSoftwareListRemovalPublishedStep, BluetoothPostUnlinkRearm,
    BluetoothPostUnlinkTake, BluetoothPrimaryPublishedInterruptStep,
    BluetoothSchedulerRunInterruptStorage,
};

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
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
        head: crate::BluetoothDtmSchedulerHeadPublished<Role>,
    ) -> Result<
        crate::BluetoothDtmSchedulerRunning<Role>,
        BluetoothDtmSchedulerStartFailure<Role, S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(BluetoothDtmSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (item, publication) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::BluetoothDtmSchedulerRunning::new(item, run))
    }

    /// Perform one fresh fenced completion-list transfer and immediately join
    /// its affine result to this exact running DTM graph.
    ///
    /// A non-sentinel item status advances only to completion-observed. The
    /// descriptor, packet and scheduler reservation remain hardware-owned
    /// until the later unlink and recycle transaction is complete.
    pub fn observe_dtm_completion<Role>(
        &mut self,
        running: crate::BluetoothDtmSchedulerRunning<Role>,
        wake: crate::BluetoothSchedulerWakeBatch,
    ) -> crate::BluetoothDtmSchedulerCompletionStep<Role> {
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
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothDtmSchedulerRunning<Role>,
        >,
    ) -> crate::BluetoothDtmSchedulerRunningDrainStep<Role> {
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
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothDtmSchedulerCompletionObserved<Role>,
        >,
    ) -> crate::BluetoothDtmSchedulerCompletionObservedDrainStep<Role> {
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
        completed: crate::BluetoothDtmSchedulerCompletionObserved<Role>,
    ) -> crate::BluetoothDtmSchedulerHardwareHeadRetirementStep<Role> {
        self.runtime.observe_dtm_hardware_head_retirement(completed)
    }

    /// Remove the sole empty-head DTM item and arm its post-unlink mailbox in
    /// one serialization boundary.
    ///
    /// A primary service cannot run between the ownership-only unlink and the
    /// mailbox arm. A busy or exhausted mailbox rejects before unlinking.
    pub fn unlink_and_arm_dtm_software_list_removal<Role>(
        &mut self,
        observed: crate::BluetoothDtmSchedulerHardwareHeadEmptyObserved<Role>,
    ) -> BluetoothDtmPostUnlinkArmStep<Role> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothDtmPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothDtmPostUnlinkArmStep::MailboxIdentityExhausted(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothDtmPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_dtm_software_list(observed) {
                crate::scheduler::BluetoothDtmSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                    observed,
                ) => BluetoothDtmPostUnlinkArmStep::SchedulerIdentityMismatch(observed),
                crate::scheduler::BluetoothDtmSchedulerSoftwareListUnlinkStep::Unlinked(
                    unlinked,
                ) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothDtmPostUnlinkArmStep::Armed(
                            crate::BluetoothPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothDtmPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
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
        awaiting: crate::BluetoothPostUnlinkAwaiting<
            crate::BluetoothDtmSchedulerSoftwareListUnlinked<Role>,
        >,
    ) -> BluetoothDtmSoftwareListRemovalPublishedStep<Role>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                BluetoothPostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime.recheck_dtm_software_list_removal(storage, unlinked) {
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                            unlinked,
                        ) => BluetoothDtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                            unlinked,
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckUnavailable {
                                    awaiting,
                                }
                            }
                            BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::DirectPending {
                                    awaiting,
                                }
                            }
                            BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::Ready(
                            ready,
                        ) => BluetoothDtmSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                BluetoothPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothDtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                        awaiting,
                    );
                }
                BluetoothPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothDtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        BluetoothPostUnlinkRearm::Armed(awaiting) => {
                            BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWork {
                                awaiting,
                                epoch,
                            }
                        }
                        BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => {
                            BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch {
                                unlinked,
                                epoch,
                            }
                        }
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_dtm_software_list_removal(unlinked, event) {
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        } => BluetoothDtmSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch {
                            unlinked,
                            event,
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::PublishedPending {
                                    awaiting,
                                }
                            }
                            BluetoothPostUnlinkRearm::AffinityMismatch(unlinked) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::PendingRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalJoin::Ready(
                            ready,
                        ) => BluetoothDtmSoftwareListRemovalPublishedStep::Ready { ready },
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
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<Role>,
    ) -> crate::BluetoothDtmSchedulerRecycleStep<Role> {
        self.runtime.recycle_dtm_completed(ready)
    }

    /// Drain, account and re-arm one successful removal-ready receiver event.
    ///
    /// The returned chain is validated before mutation. Every rejection keeps
    /// the exact graph/session owner; success releases memory, timeline and
    /// source-list ownership before exposing the re-armed session.
    pub fn recycle_dtm_receiver_success(
        &mut self,
        ready: crate::BluetoothDtmSchedulerSoftwareListRemovalReady<
            crate::BluetoothDtmReceiverEvent,
        >,
    ) -> crate::BluetoothDtmSchedulerRxSuccessRecycleStep {
        self.runtime.recycle_dtm_receiver_success(ready)
    }
}
