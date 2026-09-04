//! Task-side scheduler execution for one published Controller epoch.
//!
//! Startup, stable interrupt publication and controller-time acquisition stay
//! in the parent module. This module owns the operational scheduler surface:
//! list publication, RUN, completion draining, post-unlink gating and recycle.

pub(crate) mod connectable_advertising;
mod connection;
mod dtm;
#[cfg(target_arch = "riscv32")]
pub(crate) mod single_item;

use super::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentError,
    BluetoothControllerTimePendingOrphanStep, BluetoothLegacyAdvertisingRecurringCandidateFailure,
    BluetoothLegacyAdvertisingSchedulerStartFailure, BluetoothPassiveScanSchedulerStartFailure,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerRunInterruptsPrepared,
    drain_controller_time_orphan,
};

#[cfg(target_arch = "riscv32")]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn restore_legacy_connectable_advertising_disabled(
        &mut self,
        configured: open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertiserConfigured<
            'static,
        >,
    ) -> Result<
        (),
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure,
    > {
        self.legacy_connectable_advertising_resources
            .restore_disabled_advertiser(configured)
    }

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

    /// Borrow the retained epoch as phase-only recurring timing authority.
    pub(crate) fn legacy_connectable_advertising_recurring_timing(
        &self,
    ) -> Option<crate::BluetoothLegacyAdvertisingRecurringTimingObservation> {
        (*self.scheduler_epoch)
            .map(crate::BluetoothLegacyAdvertisingRecurringTimingObservation::new)
    }

    /// Software timing policy paired with the retained scheduler epoch.
    pub(crate) const fn legacy_connectable_advertising_scheduler_config(
        &self,
    ) -> crate::BluetoothSchedulerSoftwareConfig {
        self.runtime.scheduler_config()
    }

    /// Check out both static role graphs for one portable scheduled successor.
    pub(crate) fn begin_legacy_connectable_advertising_scheduled_event(
        &mut self,
        definition: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingSetPrepared,
        event: open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingEvent<'static>,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingPrepared,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    > {
        self.legacy_connectable_advertising_resources
            .begin_scheduled_event(definition, event, self.peripheral_connection_resources)
    }

    /// Reserve one exact phase-locked connectable successor.
    pub(crate) fn admit_legacy_connectable_advertising_recurring_event(
        &mut self,
        candidate: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingEventCandidate,
    ) -> Result<
        crate::scheduler::BluetoothLegacyConnectableAdvertisingPreSequence,
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    > {
        self.runtime
            .admit_legacy_connectable_advertising_recurring_event(candidate)
    }

    /// Release a phase-locked recurrence before sequence authorization.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_pre_sequence(
        &mut self,
        admitted: crate::scheduler::BluetoothLegacyConnectableAdvertisingPreSequence,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_pre_sequence(admitted)
    }

    /// Apply the sole fresh recurring sequence sample.
    pub(crate) fn prepare_legacy_connectable_advertising_recurring_event(
        &mut self,
        admitted: crate::scheduler::BluetoothLegacyConnectableAdvertisingPreSequence,
        sample: crate::BluetoothControllerTimeSample,
    ) -> Result<
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPrepared,
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPreparationFailure,
    > {
        self.runtime.prepare_legacy_connectable_advertising_event(
            admitted,
            crate::scheduler::BluetoothLegacyConnectableAdvertisingSequenceObservation { sample },
        )
    }

    /// Retry only the empty-list join after sequence authorization.
    pub(crate) fn merge_legacy_connectable_advertising_recurring_event(
        &mut self,
        prepared: crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    > {
        self.runtime
            .prepare_legacy_connectable_advertising_empty_list_merge(prepared)
    }

    /// Release one sequence-ready recurrence before list publication.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_event(
        &mut self,
        prepared: crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_event(prepared)
    }

    /// Undo one unpublished recurring empty-list merge.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_merge(
        &mut self,
        merged: crate::scheduler::BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingCancelled,
        crate::scheduler::BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_empty_list_merge(merged)
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
    ) -> crate::BluetoothLegacyAdvertisingCancelledRestoreOutcome<'static> {
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

    /// Admit one published advertising graph through the common RUN suffix.
    #[expect(
        clippy::result_large_err,
        reason = "a start rejection returns the complete published advertising graph"
    )]
    pub(crate) fn start_legacy_advertising_scheduler<'a>(
        &mut self,
        head: crate::BluetoothLegacyAdvertisingSchedulerHeadPublished<'a>,
    ) -> Result<
        crate::scheduler::BluetoothSingleItemSchedulerRunning<
            crate::legacy_advertising_completion::BluetoothLegacyAdvertisingCompletionRole<'a>,
        >,
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
        let item = item.into_running(&run);
        Ok(crate::scheduler::BluetoothSingleItemSchedulerRunning::new(
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
        crate::scheduler::BluetoothSingleItemSchedulerRunning<
            crate::passive_scanning_active::BluetoothPassiveScanCompletionRole,
        >,
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
        let graph = graph.into_running(&run);
        Ok(crate::scheduler::BluetoothSingleItemSchedulerRunning::new(
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

    /// Return released advertising SRAM while retaining unresolved TX status.
    pub(crate) fn recycle_legacy_advertising_completed<'a>(
        &mut self,
        ready: crate::scheduler::BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::legacy_advertising_completion::BluetoothLegacyAdvertisingCompletionRole<'a>,
        >,
    ) -> crate::scheduler::BluetoothLegacyAdvertisingSchedulerRecycleStep<'a> {
        self.runtime.recycle_legacy_advertising_completed(ready)
    }

    /// Extract completed PDUs and return scanner SRAM to CPU ownership.
    pub(crate) fn recycle_passive_scan_completed(
        &mut self,
        ready: crate::scheduler::BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::passive_scanning_active::BluetoothPassiveScanCompletionRole,
        >,
    ) -> crate::scheduler::BluetoothPassiveScanSchedulerRecycleStep {
        self.runtime.recycle_passive_scan_completed(ready)
    }

    pub(crate) fn restore_passive_scan_recycled(
        &mut self,
        recycled: crate::scheduler::BluetoothPassiveScanSchedulerRecycled,
    ) -> Result<
        (
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothLeReceivedBatch,
            open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanSchedulerItemCompletionStatus,
        ),
        crate::passive_scanning::BluetoothPassiveScanRuntimeRestoreFailure,
    >{
        self.passive_scan_resources.restore_recycled(recycled)
    }

    /// Reclaim one completed response-capable advertising event after generic removal.
    pub(crate) fn recycle_legacy_connectable_advertising_completed(
        &mut self,
        ready: crate::scheduler::BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            crate::legacy_connectable_advertising_completion::BluetoothLegacyConnectableAdvertisingCompletionRole,
        >,
    ) -> crate::legacy_connectable_advertising_completion::BluetoothLegacyConnectableAdvertisingRecycleStep
    {
        self.runtime
            .recycle_legacy_connectable_advertising_completed(ready)
    }

    /// Restore both reusable runtime slots after an event accepted no connection.
    pub(crate) fn restore_legacy_connectable_advertising_no_connection(
        &mut self,
        outcome: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingNoConnection,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingNoConnection,
    > {
        self.legacy_connectable_advertising_resources
            .restore_no_connection(outcome, self.peripheral_connection_resources)
    }

    /// Restore advertising SRAM while retaining the accepted peripheral allocation.
    pub(crate) fn restore_legacy_connectable_advertising_connection(
        &mut self,
        outcome: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingConnectionAccepted,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingConnectionTransfer,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingConnectionAccepted,
    > {
        self.legacy_connectable_advertising_resources
            .restore_connection_accepted(outcome)
    }

    /// Cancel an accepted connection only at the explicit pre-publication Reset boundary.
    pub(crate) fn cancel_legacy_connectable_advertising_connection_for_reset(
        &mut self,
        transfer: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingConnectionTransfer,
    ) -> Result<
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
        crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure,
    >{
        transfer.cancel_peripheral_for_reset(self.peripheral_connection_resources)
    }
}
