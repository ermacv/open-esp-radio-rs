//! Task-side scheduler execution for one published Controller epoch.
//!
//! Startup, stable interrupt publication and controller-time acquisition stay
//! in the parent module. This module owns the operational scheduler surface:
//! list publication, RUN, completion draining, post-unlink gating and recycle.

mod connection;

use super::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentError,
    BluetoothControllerTimePendingOrphanStep, BluetoothDtmPostUnlinkArmError,
    BluetoothDtmPostUnlinkArmStep, BluetoothDtmPostUnlinkRearm, BluetoothDtmPostUnlinkTake,
    BluetoothDtmSchedulerStartFailure, BluetoothDtmSoftwareListRemovalPublishedStep,
    BluetoothLegacyAdvertisingPostUnlinkArmStep, BluetoothLegacyAdvertisingPostUnlinkRearm,
    BluetoothLegacyAdvertisingPostUnlinkTake, BluetoothLegacyAdvertisingRecurringCandidateFailure,
    BluetoothLegacyAdvertisingSchedulerStartFailure,
    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep,
    BluetoothPassiveScanPostUnlinkArmStep, BluetoothPassiveScanPostUnlinkRearm,
    BluetoothPassiveScanPostUnlinkTake, BluetoothPassiveScanSchedulerStartFailure,
    BluetoothPassiveScanSoftwareListRemovalPublishedStep, BluetoothPrimaryPublishedInterruptStep,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerRunInterruptsPrepared,
    drain_controller_time_orphan,
};

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Durable general scheduler handoff for this powered epoch.
    pub const fn scheduler_wake(&self) -> &crate::BluetoothSchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// Durable scheduler lock/modify handoff for this powered epoch.
    pub const fn scheduler_lock_modify_events(
        &self,
    ) -> &crate::BluetoothSchedulerLockModifyEventCell {
        self.runtime.scheduler_lock_modify_events()
    }

    /// Sole bounded finished-list worker for task-side draining.
    pub fn scheduler_finished_lists(&mut self) -> &mut crate::BluetoothSchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }

    /// Durable ready notification for the Controller-owned post-unlink mailbox.
    ///
    /// Executor integrations register their waker before rechecking this cell.
    /// The mailbox itself closes the epoch only when it consumes the ready
    /// event, so cancellation of a waiter cannot discard notification state.
    pub const fn post_unlink_wake(&self) -> &crate::BluetoothDtmPostUnlinkWakeCell {
        self.mailbox.wake()
    }

    /// Rebuild one successor from the completed event's nominal phase.
    pub(crate) fn prepare_legacy_advertising_recurring_candidate(
        &self,
        scheduled: crate::BluetoothLegacyAdvertisingNextEventScheduled<'static>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingRecurringEventCandidate<'static>,
        BluetoothLegacyAdvertisingRecurringCandidateFailure,
    > {
        let Some(epoch) = *self.scheduler_epoch else {
            return Err(
                BluetoothLegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                    scheduled,
                ),
            );
        };
        scheduled
            .prepare_candidate(
                self.legacy_advertising_resources.default_tx_power_dbm(),
                crate::BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
                self.runtime.scheduler_config(),
            )
            .map_err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation)
    }

    /// Retry only the finite packet/reset/timing projection of a successor.
    pub(crate) fn retry_legacy_advertising_recurring_candidate(
        &self,
        failure: crate::BluetoothLegacyAdvertisingRecurringPreparationFailure<'static>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingRecurringEventCandidate<'static>,
        BluetoothLegacyAdvertisingRecurringCandidateFailure,
    > {
        let Some(epoch) = *self.scheduler_epoch else {
            return Err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation(failure));
        };
        failure
            .retry(
                self.legacy_advertising_resources.default_tx_power_dbm(),
                crate::BluetoothLegacyAdvertisingRecurringTimingObservation::new(epoch),
                self.runtime.scheduler_config(),
            )
            .map_err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation)
    }

    /// Reserve one recurring advertising window in the retained timeline.
    pub(crate) fn admit_legacy_advertising_recurring_candidate(
        &mut self,
        candidate: crate::BluetoothLegacyAdvertisingRecurringEventCandidate<'static>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
        crate::BluetoothLegacyAdvertisingRecurringEventPreparationFailure<'static>,
    > {
        self.runtime
            .admit_legacy_advertising_recurring_event(candidate)
    }

    /// Release a recurring timeline reservation before sequence authorization.
    pub(crate) fn cancel_legacy_advertising_recurring_pre_sequence(
        &mut self,
        admitted: crate::BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    ) -> crate::BluetoothLegacyAdvertisingCancelled<'static> {
        self.runtime
            .cancel_legacy_advertising_recurring_pre_sequence(admitted)
            .into_parts()
            .0
    }

    /// Release a sequence-ready recurring descriptor before scheduler-list publication.
    pub(crate) fn cancel_legacy_advertising_recurring_prepared(
        &mut self,
        prepared: crate::BluetoothLegacyAdvertisingEventPrepared<'static>,
    ) -> crate::BluetoothLegacyAdvertisingCancelled<'static> {
        self.runtime.cancel_legacy_advertising_first_event(prepared)
    }

    /// Undo one unpublished recurring empty-list merge.
    pub(crate) fn cancel_legacy_advertising_recurring_merge(
        &mut self,
        merged: crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingCancelled<'static>,
        crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    > {
        self.runtime
            .cancel_legacy_advertising_empty_list_merge(merged)
            .map(|prepared| self.runtime.cancel_legacy_advertising_first_event(prepared))
    }

    /// Return an unpublished disabled successor to this exact runtime.
    pub(crate) fn restore_legacy_advertising_cancelled_disabled(
        &mut self,
        cancelled: crate::BluetoothLegacyAdvertisingCancelled<'static>,
    ) -> Result<(), crate::BluetoothLegacyAdvertisingCancelled<'static>> {
        self.legacy_advertising_resources
            .restore_cancelled(cancelled)
    }

    /// Observe one abandoned Controller-time request before publishing stop success.
    pub(crate) fn drain_abandoned_recurring_controller_time(
        &mut self,
    ) -> Result<
        crate::BluetoothControllerTimeOrphanDrainStep,
        BluetoothControllerSchedulerCurrentError,
    > {
        match drain_controller_time_orphan(self) {
            Ok(BluetoothControllerTimePendingOrphanStep::Idle) => {
                Ok(crate::BluetoothControllerTimeOrphanDrainStep::Idle)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Waiting) => {
                Ok(crate::BluetoothControllerTimeOrphanDrainStep::Waiting)
            }
            Ok(BluetoothControllerTimePendingOrphanStep::Drained) => {
                Ok(crate::BluetoothControllerTimeOrphanDrainStep::Drained)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Retry the empty-list join without rebuilding or reauthorizing the event.
    pub(crate) fn merge_legacy_advertising_recurring_event(
        &mut self,
        prepared: crate::BluetoothLegacyAdvertisingEventPrepared<'static>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
        crate::BluetoothLegacyAdvertisingEmptySchedulerMergeFailure<'static>,
    > {
        self.runtime
            .prepare_legacy_advertising_empty_list_merge(prepared)
    }

    /// Advance one finite scheduler lock/modify transaction.
    pub fn step_scheduler_lock_modify(
        &mut self,
        event: crate::BluetoothSchedulerLockModifyEvent,
    ) -> crate::BluetoothSchedulerLockModifyWorkerStep {
        self.runtime.step_scheduler_lock_modify(event)
    }

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

    /// Admit one published advertising graph through the common RUN suffix.
    #[expect(
        clippy::result_large_err,
        reason = "a start rejection returns the complete published advertising graph"
    )]
    pub fn start_legacy_advertising_scheduler<'a>(
        &mut self,
        head: crate::BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
    ) -> Result<
        crate::BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        BluetoothLegacyAdvertisingSchedulerStartFailure<'a, S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(BluetoothLegacyAdvertisingSchedulerStartFailure { error, head });
            }
        };
        let address = head.scheduler_item_address();
        let (item, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::BluetoothLegacyAdvertisingSchedulerRunning::new(
            item,
            run,
            reservation,
        ))
    }

    /// Admit one published passive-scanner graph through the common RUN suffix.
    pub(crate) fn start_passive_scan_scheduler(
        &mut self,
        head: crate::BluetoothPassiveScanSchedulerHeadPublished,
    ) -> Result<
        crate::BluetoothPassiveScanSchedulerRunning,
        BluetoothPassiveScanSchedulerStartFailure<S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(BluetoothPassiveScanSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (graph, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::BluetoothPassiveScanSchedulerRunning::new(
            graph,
            run,
            reservation,
        ))
    }

    fn publish_scheduler_run_suffix(
        &mut self,
        address: open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress,
        publication: open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> open_esp_radio_esp32s31_hal::BluetoothSchedulerHardwareRunCommandPublished {
        let event = self
            .runtime
            .publish_scheduler_run_event(publication, interrupts);
        let run = self.runtime.publish_scheduler_hardware_run_command(event);
        self.runtime.retain_running_first_item(address);
        run
    }

    /// Perform one fresh fenced completion-list transfer for advertising.
    pub fn observe_legacy_advertising_completion<'a>(
        &mut self,
        running: crate::BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        wake: crate::BluetoothSchedulerWakeBatch,
    ) -> crate::BluetoothLegacyAdvertisingSchedulerCompletionStep<'a> {
        self.runtime
            .observe_legacy_advertising_completion(running, wake)
    }

    /// Continue one retained finished-list capture while advertising runs.
    pub fn continue_legacy_advertising_running_finished_list_drain<'a>(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothLegacyAdvertisingSchedulerRunning<'a>,
        >,
    ) -> crate::BluetoothLegacyAdvertisingSchedulerRunningDrainStep<'a> {
        self.runtime
            .continue_legacy_advertising_running_finished_list_drain(pending)
    }

    /// Continue one retained capture after advertising completion was observed.
    pub fn continue_legacy_advertising_completed_finished_list_drain<'a>(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
        >,
    ) -> crate::BluetoothLegacyAdvertisingSchedulerCompletionObservedDrainStep<'a> {
        self.runtime
            .continue_legacy_advertising_completed_finished_list_drain(pending)
    }

    /// Observe the post-picker hardware-head retirement barrier for advertising.
    pub fn observe_legacy_advertising_hardware_head_retirement<'a>(
        &mut self,
        completed: crate::BluetoothLegacyAdvertisingSchedulerCompletionObserved<'a>,
    ) -> crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadRetirementStep<'a> {
        self.runtime
            .observe_legacy_advertising_hardware_head_retirement(completed)
    }

    /// Atomically unlink advertising and arm the shared post-unlink mailbox.
    pub fn unlink_and_arm_legacy_advertising_software_list_removal<'a>(
        &mut self,
        observed: crate::BluetoothLegacyAdvertisingSchedulerHardwareHeadEmptyObserved<'a>,
    ) -> BluetoothLegacyAdvertisingPostUnlinkArmStep<'a> {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxIdentityExhausted(
                        observed,
                    );
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothLegacyAdvertisingPostUnlinkArmStep::GenerationExhausted(
                        observed,
                    );
                }
            };
            match runtime.unlink_legacy_advertising_software_list(observed) {
                crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(
                    observed,
                ) => BluetoothLegacyAdvertisingPostUnlinkArmStep::SchedulerIdentityMismatch(
                    observed,
                ),
                crate::BluetoothLegacyAdvertisingSchedulerSoftwareListUnlinkStep::Unlinked(
                    unlinked,
                ) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothLegacyAdvertisingPostUnlinkArmStep::Armed(
                            crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothLegacyAdvertisingPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    /// Consume or directly recheck one armed advertising removal gate.
    pub fn consume_published_legacy_advertising_software_list_removal<'a>(
        &mut self,
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
    ) -> BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep<'a>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take_legacy_advertising(critical_section, awaiting) {
                BluetoothLegacyAdvertisingPostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime
                        .recheck_legacy_advertising_software_list_removal(storage, unlinked)
                    {
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked) => {
                            BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { unlinked }
                        }
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::StorageUnavailable(unlinked) => {
                            match mailbox.rearm_legacy_advertising(critical_section, key, unlinked) {
                                BluetoothLegacyAdvertisingPostUnlinkRearm::Armed(awaiting) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting },
                                BluetoothLegacyAdvertisingPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::Pending(unlinked) => {
                            match mailbox.rearm_legacy_advertising(critical_section, key, unlinked) {
                                BluetoothLegacyAdvertisingPostUnlinkRearm::Armed(awaiting) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::DirectPending { awaiting },
                                BluetoothLegacyAdvertisingPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalRecheck::Ready(ready) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                BluetoothLegacyAdvertisingPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting);
                }
                BluetoothLegacyAdvertisingPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Fault {
                        unlinked,
                        fault,
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm_legacy_advertising(critical_section, key, unlinked) {
                        BluetoothLegacyAdvertisingPostUnlinkRearm::Armed(awaiting) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, epoch },
                        BluetoothLegacyAdvertisingPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch },
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_legacy_advertising_software_list_removal(unlinked, event) {
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch { unlinked, event } => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { unlinked, event },
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::Pending(unlinked) => {
                            match mailbox.rearm_legacy_advertising(critical_section, key, unlinked) {
                                BluetoothLegacyAdvertisingPostUnlinkRearm::Armed(awaiting) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::PublishedPending { awaiting },
                                BluetoothLegacyAdvertisingPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalJoin::Ready(ready) => BluetoothLegacyAdvertisingSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }

    /// Cancel an advertising post-unlink wait without discarding a ready event.
    pub fn cancel_legacy_advertising_software_list_removal<'a>(
        &mut self,
        awaiting: crate::BluetoothLegacyAdvertisingPostUnlinkAwaiting<'a>,
    ) -> crate::BluetoothLegacyAdvertisingPostUnlinkCancelStep<'a> {
        critical_section::with(|critical_section| {
            self.mailbox
                .cancel_legacy_advertising(critical_section, awaiting)
        })
    }

    /// Return released advertising SRAM while retaining unresolved TX status.
    pub fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: crate::BluetoothLegacyAdvertisingSchedulerSoftwareListRemovalReady<'a>,
    ) -> crate::BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
        self.runtime.recycle_legacy_advertising_completed(ready)
    }

    /// Perform one fresh fenced completion-list transfer for passive scanning.
    pub fn observe_passive_scan_completion(
        &mut self,
        running: crate::BluetoothPassiveScanSchedulerRunning,
        wake: crate::BluetoothSchedulerWakeBatch,
    ) -> crate::BluetoothPassiveScanSchedulerCompletionStep {
        self.runtime.observe_passive_scan_completion(running, wake)
    }

    /// Continue one retained finished-list capture while the scanner runs.
    pub fn continue_passive_scan_running_finished_list_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothPassiveScanSchedulerRunning,
        >,
    ) -> crate::BluetoothPassiveScanSchedulerRunningDrainStep {
        self.runtime
            .continue_passive_scan_running_finished_list_drain(pending)
    }

    /// Continue one retained capture after scanner completion was observed.
    pub fn continue_passive_scan_completed_finished_list_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothPassiveScanSchedulerCompletionObserved,
        >,
    ) -> crate::BluetoothPassiveScanSchedulerCompletionObservedDrainStep {
        self.runtime
            .continue_passive_scan_completed_finished_list_drain(pending)
    }

    /// Observe the post-picker hardware-head retirement barrier for scanning.
    pub fn observe_passive_scan_hardware_head_retirement(
        &mut self,
        completed: crate::BluetoothPassiveScanSchedulerCompletionObserved,
    ) -> crate::BluetoothPassiveScanSchedulerHardwareHeadRetirementStep {
        self.runtime
            .observe_passive_scan_hardware_head_retirement(completed)
    }

    /// Atomically unlink the scanner item and arm the shared return mailbox.
    pub fn unlink_and_arm_passive_scan_software_list_removal(
        &mut self,
        observed: crate::BluetoothPassiveScanSchedulerHardwareHeadEmptyObserved,
    ) -> BluetoothPassiveScanPostUnlinkArmStep {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothPassiveScanPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothPassiveScanPostUnlinkArmStep::MailboxIdentityExhausted(
                        observed,
                    );
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothPassiveScanPostUnlinkArmStep::GenerationExhausted(observed);
                }
            };
            match runtime.unlink_passive_scan_software_list(observed) {
                crate::BluetoothPassiveScanSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed) => {
                    BluetoothPassiveScanPostUnlinkArmStep::SchedulerIdentityMismatch(observed)
                }
                crate::BluetoothPassiveScanSchedulerSoftwareListUnlinkStep::Unlinked(unlinked) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothPassiveScanPostUnlinkArmStep::Armed(
                            crate::BluetoothPassiveScanPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothPassiveScanPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    /// Consume or directly recheck one armed scanner removal gate.
    pub fn consume_published_passive_scan_software_list_removal(
        &mut self,
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
    ) -> BluetoothPassiveScanSoftwareListRemovalPublishedStep
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take_passive_scan(critical_section, awaiting) {
                BluetoothPassiveScanPostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime
                        .recheck_passive_scan_software_list_removal(storage, unlinked)
                    {
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked) => {
                            BluetoothPassiveScanSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { unlinked }
                        }
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::StorageUnavailable(unlinked) => {
                            match mailbox.rearm_passive_scan(critical_section, key, unlinked) {
                                BluetoothPassiveScanPostUnlinkRearm::Armed(awaiting) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting },
                                BluetoothPassiveScanPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::Pending(unlinked) => {
                            match mailbox.rearm_passive_scan(critical_section, key, unlinked) {
                                BluetoothPassiveScanPostUnlinkRearm::Armed(awaiting) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::DirectPending { awaiting },
                                BluetoothPassiveScanPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalRecheck::Ready(ready) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::Ready { ready },
                    };
                }
                BluetoothPassiveScanPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothPassiveScanSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting);
                }
                BluetoothPassiveScanPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothPassiveScanSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm_passive_scan(critical_section, key, unlinked) {
                        BluetoothPassiveScanPostUnlinkRearm::Armed(awaiting) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, epoch },
                        BluetoothPassiveScanPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch },
                    }
                }
                BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                    match runtime.join_passive_scan_software_list_removal(unlinked, event) {
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch { unlinked, event } => BluetoothPassiveScanSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { unlinked, event },
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::Pending(unlinked) => {
                            match mailbox.rearm_passive_scan(critical_section, key, unlinked) {
                                BluetoothPassiveScanPostUnlinkRearm::Armed(awaiting) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::PublishedPending { awaiting },
                                BluetoothPassiveScanPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked },
                            }
                        }
                        crate::BluetoothPassiveScanSchedulerSoftwareListRemovalJoin::Ready(ready) => BluetoothPassiveScanSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
            }
        })
    }

    /// Cancel a scanner post-unlink wait without discarding a ready event.
    pub fn cancel_passive_scan_software_list_removal(
        &mut self,
        awaiting: crate::BluetoothPassiveScanPostUnlinkAwaiting,
    ) -> crate::BluetoothPassiveScanPostUnlinkCancelStep {
        critical_section::with(|critical_section| {
            self.mailbox.cancel_passive_scan(critical_section, awaiting)
        })
    }

    /// Extract completed PDUs and return scanner SRAM to CPU ownership.
    pub fn recycle_passive_scan_completed(
        &mut self,
        ready: crate::BluetoothPassiveScanSchedulerSoftwareListRemovalReady,
    ) -> crate::BluetoothPassiveScanSchedulerRecycleStep {
        self.runtime.recycle_passive_scan_completed(ready)
    }

    pub(crate) fn restore_passive_scan_recycled(
        &mut self,
        recycled: crate::BluetoothPassiveScanSchedulerRecycled,
    ) -> Result<
        (
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch,
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanSchedulerItemCompletionStatus,
        ),
        crate::passive_scanning::BluetoothPassiveScanRuntimeRestoreFailure,
    >{
        self.passive_scan_resources.restore_recycled(recycled)
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
                            crate::BluetoothDtmPostUnlinkAwaiting::new(unlinked, key),
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
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> BluetoothDtmSoftwareListRemovalPublishedStep<Role>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let storage = self.storage;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let (key, pending) = match mailbox.take(critical_section, awaiting) {
                BluetoothDtmPostUnlinkTake::Recheck { key, unlinked } => {
                    return match runtime.recheck_dtm_software_list_removal(storage, unlinked) {
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(
                            unlinked,
                        ) => BluetoothDtmSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch {
                            unlinked,
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::StorageUnavailable(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckUnavailable {
                                    awaiting,
                                }
                            }
                            BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::RecheckRearmMismatch {
                                    unlinked,
                                }
                            }
                        },
                        crate::scheduler::BluetoothDtmSchedulerSoftwareListRemovalRecheck::Pending(
                            unlinked,
                        ) => match mailbox.rearm(critical_section, key, unlinked) {
                            BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::DirectPending {
                                    awaiting,
                                }
                            }
                            BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
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
                BluetoothDtmPostUnlinkTake::AffinityMismatch(awaiting) => {
                    return BluetoothDtmSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(
                        awaiting,
                    );
                }
                BluetoothDtmPostUnlinkTake::Ready { key, event } => (key, event),
            };
            let (unlinked, published) = pending.into_parts();
            match published {
                BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                    BluetoothDtmSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                }
                BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                    match mailbox.rearm(critical_section, key, unlinked) {
                        BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                            BluetoothDtmSoftwareListRemovalPublishedStep::NoSchedulerWork {
                                awaiting,
                                epoch,
                            }
                        }
                        BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
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
                            BluetoothDtmPostUnlinkRearm::Armed(awaiting) => {
                                BluetoothDtmSoftwareListRemovalPublishedStep::PublishedPending {
                                    awaiting,
                                }
                            }
                            BluetoothDtmPostUnlinkRearm::AffinityMismatch(unlinked) => {
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

    /// Cancel an armed post-unlink wait without discarding a stored event.
    pub fn cancel_dtm_software_list_removal<Role>(
        &mut self,
        awaiting: crate::BluetoothDtmPostUnlinkAwaiting<Role>,
    ) -> crate::BluetoothDtmPostUnlinkCancelStep<Role> {
        critical_section::with(|critical_section| self.mailbox.cancel(critical_section, awaiting))
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
