//! Peripheral-connection scheduler service operations.

use super::super::{
    BluetoothControllerPublishedTaskService, BluetoothPeripheralConnectionPostUnlinkArmStep,
    BluetoothPeripheralConnectionPostUnlinkRearm, BluetoothPeripheralConnectionPostUnlinkTake,
    BluetoothPeripheralConnectionRecurringSchedulerStartFailure,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin,
    BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck,
    BluetoothPeripheralConnectionSchedulerStartFailure,
    BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep,
    BluetoothSchedulerRunInterruptStorage,
};
use crate::BluetoothPrimaryPublishedInterruptStep;
use crate::dtm_post_unlink::BluetoothDtmPostUnlinkArmError;

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Validate and run one recurring event through a single commit/publication edge.
    ///
    /// Common-list/head validation and stable interrupt preparation are the
    /// only fallible operations. The LL successor and proposed phase commit
    /// together only after both succeed; RX/head/event/RUN then publish as one
    /// infallible suffix.
    #[allow(
        clippy::result_large_err,
        reason = "every pre-commit rejection returns the complete recurring merge"
    )]
    pub fn start_peripheral_connection_recurring_scheduler(
        &mut self,
        merged: crate::scheduler::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> Result<
        crate::BluetoothPeripheralConnectionSchedulerRunning,
        BluetoothPeripheralConnectionRecurringSchedulerStartFailure<S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let validated = match self
            .runtime
            .validate_peripheral_connection_recurring_scheduler(merged)
        {
            Ok(validated) => validated,
            Err(failure) => {
                return Err(
                    BluetoothPeripheralConnectionRecurringSchedulerStartFailure::Validation {
                        error: failure.error(),
                        merged: failure.into_merged(),
                    },
                );
            }
        };
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(
                    BluetoothPeripheralConnectionRecurringSchedulerStartFailure::Interrupts {
                        error,
                        merged: validated.into_merged(),
                    },
                );
            }
        };
        let committed = validated.commit(interrupts);
        let (head, interrupts) = self
            .runtime
            .publish_peripheral_connection_recurring_scheduler_head(committed);
        let address = head.scheduler_item_address();
        let (event, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::BluetoothPeripheralConnectionSchedulerRunning::new(
            event,
            run,
            reservation,
        ))
    }

    /// Admit one RX/head-published connection event through the common RUN suffix.
    #[allow(
        clippy::result_large_err,
        reason = "stable-storage rejection returns the complete published connection graph"
    )]
    pub fn start_peripheral_connection_scheduler(
        &mut self,
        head: crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
    ) -> Result<
        crate::BluetoothPeripheralConnectionSchedulerRunning,
        BluetoothPeripheralConnectionSchedulerStartFailure<S::Error>,
    >
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(BluetoothPeripheralConnectionSchedulerStartFailure { error, head });
            }
        };
        let address = head.scheduler_item_address();
        let (event, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(crate::BluetoothPeripheralConnectionSchedulerRunning::new(
            event,
            run,
            reservation,
        ))
    }

    /// Perform one fresh fenced completion-list transfer for a connection event.
    pub fn observe_peripheral_connection_completion(
        &mut self,
        running: crate::BluetoothPeripheralConnectionSchedulerRunning,
        wake: crate::BluetoothSchedulerWakeBatch,
    ) -> crate::BluetoothPeripheralConnectionSchedulerCompletionStep {
        self.runtime
            .observe_peripheral_connection_completion(running, wake)
    }

    /// Continue one retained finished-list capture while the connection runs.
    pub fn continue_peripheral_connection_running_finished_list_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothPeripheralConnectionSchedulerRunning,
        >,
    ) -> crate::BluetoothPeripheralConnectionSchedulerRunningDrainStep {
        self.runtime
            .continue_peripheral_connection_running_finished_list_drain(pending)
    }

    /// Continue one retained capture after connection completion was observed.
    pub fn continue_peripheral_connection_completed_finished_list_drain(
        &mut self,
        pending: crate::BluetoothSchedulerFinishedListDrainPending<
            crate::BluetoothPeripheralConnectionSchedulerCompletionObserved,
        >,
    ) -> crate::BluetoothPeripheralConnectionSchedulerCompletionObservedDrainStep {
        self.runtime
            .continue_peripheral_connection_completed_finished_list_drain(pending)
    }

    /// Observe the post-picker hardware-head retirement barrier for a connection.
    pub fn observe_peripheral_connection_hardware_head_retirement(
        &mut self,
        completed: crate::BluetoothPeripheralConnectionSchedulerCompletionObserved,
    ) -> crate::BluetoothPeripheralConnectionSchedulerHardwareHeadRetirementStep {
        self.runtime
            .observe_peripheral_connection_hardware_head_retirement(completed)
    }

    /// Atomically unlink the connection item and arm its post-unlink mailbox.
    pub fn unlink_and_arm_peripheral_connection_software_list_removal(
        &mut self,
        observed: crate::BluetoothPeripheralConnectionSchedulerHardwareHeadEmptyObserved,
    ) -> BluetoothPeripheralConnectionPostUnlinkArmStep {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            let key = match mailbox.prepare_arm(critical_section) {
                Ok(key) => key,
                Err(BluetoothDtmPostUnlinkArmError::Busy) => {
                    return BluetoothPeripheralConnectionPostUnlinkArmStep::MailboxBusy(observed);
                }
                Err(BluetoothDtmPostUnlinkArmError::IdentityExhausted) => {
                    return BluetoothPeripheralConnectionPostUnlinkArmStep::MailboxIdentityExhausted(
                        observed,
                    );
                }
                Err(BluetoothDtmPostUnlinkArmError::GenerationExhausted) => {
                    return BluetoothPeripheralConnectionPostUnlinkArmStep::GenerationExhausted(
                        observed,
                    );
                }
            };
            match runtime.unlink_peripheral_connection_software_list(observed) {
                crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::SchedulerIdentityMismatch(observed) => {
                    BluetoothPeripheralConnectionPostUnlinkArmStep::SchedulerIdentityMismatch(observed)
                }
                crate::BluetoothPeripheralConnectionSchedulerSoftwareListUnlinkStep::Unlinked(unlinked) => {
                    if mailbox.commit_arm(critical_section, key) {
                        BluetoothPeripheralConnectionPostUnlinkArmStep::Armed(
                            crate::BluetoothPeripheralConnectionPostUnlinkAwaiting::new(unlinked, key),
                        )
                    } else {
                        BluetoothPeripheralConnectionPostUnlinkArmStep::MailboxCommitMismatch(unlinked)
                    }
                }
            }
        })
    }

    /// Consume or directly recheck one armed connection removal gate.
    pub fn consume_peripheral_connection_software_list_removal(
        &mut self,
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
    ) -> BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let runtime = &mut self.runtime;
        let mailbox = self.mailbox;
        critical_section::with(|critical_section| {
            match mailbox.take_peripheral_connection(critical_section, awaiting) {
                BluetoothPeripheralConnectionPostUnlinkTake::Recheck { key, unlinked } => {
                    match runtime.recheck_peripheral_connection_software_list_removal(
                        self.storage,
                        unlinked,
                    ) {
                        BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::SchedulerIdentityMismatch(unlinked) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::DirectSchedulerIdentityMismatch { unlinked },
                        BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::StorageUnavailable(unlinked) => {
                            match mailbox.rearm_peripheral_connection(critical_section, key, unlinked) {
                                BluetoothPeripheralConnectionPostUnlinkRearm::Armed(awaiting) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::RecheckUnavailable { awaiting },
                                BluetoothPeripheralConnectionPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Pending(unlinked) => {
                            match mailbox.rearm_peripheral_connection(critical_section, key, unlinked) {
                                BluetoothPeripheralConnectionPostUnlinkRearm::Armed(awaiting) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::DirectPending { awaiting },
                                BluetoothPeripheralConnectionPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::RecheckRearmMismatch { unlinked },
                            }
                        }
                        BluetoothPeripheralConnectionSchedulerSoftwareListRemovalRecheck::Ready(ready) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::Ready { ready },
                    }
                }
                BluetoothPeripheralConnectionPostUnlinkTake::AffinityMismatch(awaiting) => {
                    BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::MailboxAffinityMismatch(awaiting)
                }
                BluetoothPeripheralConnectionPostUnlinkTake::Ready { key, event } => {
                    let (unlinked, published) = event.into_parts();
                    match published {
                        BluetoothPrimaryPublishedInterruptStep::Fault(fault) => {
                            BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::Fault { unlinked, fault }
                        }
                        BluetoothPrimaryPublishedInterruptStep::NoSchedulerWork(epoch) => {
                            match mailbox.rearm_peripheral_connection(critical_section, key, unlinked) {
                                BluetoothPeripheralConnectionPostUnlinkRearm::Armed(awaiting) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::NoSchedulerWork { awaiting, epoch },
                                BluetoothPeripheralConnectionPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::NoSchedulerWorkRearmMismatch { unlinked, epoch },
                            }
                        }
                        BluetoothPrimaryPublishedInterruptStep::Scheduler { event, .. } => {
                            match runtime.join_peripheral_connection_software_list_removal(unlinked, event) {
                                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::SchedulerIdentityMismatch { unlinked, event } => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::SchedulerIdentityMismatch { unlinked, event },
                                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Pending(unlinked) => {
                                    match mailbox.rearm_peripheral_connection(critical_section, key, unlinked) {
                                        BluetoothPeripheralConnectionPostUnlinkRearm::Armed(awaiting) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::PublishedPending { awaiting },
                                        BluetoothPeripheralConnectionPostUnlinkRearm::AffinityMismatch(unlinked) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::PendingRearmMismatch { unlinked },
                                    }
                                }
                                BluetoothPeripheralConnectionSchedulerSoftwareListRemovalJoin::Ready(ready) => BluetoothPeripheralConnectionSoftwareListRemovalPublishedStep::Ready { ready },
                            }
                        }
                    }
                }
            }
        })
    }

    /// Reclaim event-local connection SRAM and the common scheduler owners.
    pub fn recycle_peripheral_connection_completed(
        &mut self,
        ready: crate::BluetoothPeripheralConnectionSchedulerSoftwareListRemovalReady,
    ) -> crate::BluetoothPeripheralConnectionSchedulerRecycleStep {
        self.runtime.recycle_peripheral_connection_completed(ready)
    }

    /// Cancel a connection post-unlink wait without discarding a ready event.
    pub fn cancel_peripheral_connection_software_list_removal(
        &mut self,
        awaiting: crate::BluetoothPeripheralConnectionPostUnlinkAwaiting,
    ) -> crate::BluetoothPeripheralConnectionPostUnlinkCancelStep {
        critical_section::with(|critical_section| {
            self.mailbox
                .cancel_peripheral_connection(critical_section, awaiting)
        })
    }
}
