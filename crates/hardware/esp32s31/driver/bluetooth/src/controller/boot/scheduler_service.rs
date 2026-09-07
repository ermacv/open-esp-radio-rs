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
    BluetoothSchedulerRunInterruptsPrepared, ControllerPublishedTaskService,
    ControllerSchedulerCurrentError, ControllerTimePendingOrphanStep,
    LegacyAdvertisingRecurringCandidateFailure, LegacyAdvertisingSchedulerStartFailure,
    PassiveScanSchedulerStartFailure, SchedulerRunInterruptStorage, drain_controller_time_orphan,
};

#[cfg(target_arch = "riscv32")]
#[expect(
    clippy::result_large_err,
    reason = "scheduler service transitions return exact role graphs, reservations, and HCI continuation owners on failure"
)]
impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn restore_legacy_connectable_advertising_disabled(
        &mut self,
        configured: oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertiserConfigured<
            'static,
        >,
    ) -> Result<
        (),
        crate::le::advertising::connectable::LegacyConnectableAdvertisingDisabledRestoreFailure,
    > {
        self.legacy_connectable_advertising_resources
            .restore_disabled_advertiser(configured)
    }

    /// Durable general scheduler handoff for this powered epoch.
    pub const fn scheduler_wake(&self) -> &crate::interrupt::SchedulerWakeCell {
        self.runtime.scheduler_wake()
    }

    /// Durable scheduler lock/modify handoff for this powered epoch.
    pub const fn scheduler_lock_modify_events(
        &self,
    ) -> &crate::scheduler::SchedulerLockModifyEventCell {
        self.runtime.scheduler_lock_modify_events()
    }

    /// Sole bounded finished-list worker for task-side draining.
    pub fn scheduler_finished_lists(
        &mut self,
    ) -> &mut crate::scheduler::SchedulerFinishedListWorker {
        self.runtime.scheduler_finished_lists()
    }

    /// Durable ready notification for the Controller-owned post-unlink mailbox.
    ///
    /// Executor integrations register their waker before rechecking this cell.
    /// The mailbox itself closes the epoch only when it consumes the ready
    /// event, so cancellation of a waiter cannot discard notification state.
    pub const fn post_unlink_wake(&self) -> &crate::le::dtm::DtmPostUnlinkWakeCell {
        self.mailbox.wake()
    }

    /// Borrow the retained epoch as phase-only recurring timing authority.
    pub(crate) fn legacy_connectable_advertising_recurring_timing(
        &self,
    ) -> Option<crate::LegacyAdvertisingRecurringTimingObservation> {
        (*self.scheduler_epoch).map(crate::LegacyAdvertisingRecurringTimingObservation::new)
    }

    /// Software timing policy paired with the retained scheduler epoch.
    pub(crate) const fn legacy_connectable_advertising_scheduler_config(
        &self,
    ) -> crate::scheduler::SchedulerSoftwareConfig {
        self.runtime.scheduler_config()
    }

    /// Check out both static role graphs for one portable scheduled successor.
    pub(crate) fn begin_legacy_connectable_advertising_scheduled_event(
        &mut self,
        definition: crate::le::advertising::connectable::LegacyConnectableAdvertisingSetPrepared,
        event: oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingEvent<
            'static,
        >,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingPrepared,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingRuntimeBeginFailure,
    > {
        self.legacy_connectable_advertising_resources
            .begin_scheduled_event(definition, event, self.peripheral_connection_resources)
    }

    /// Reserve one exact phase-locked connectable successor.
    pub(crate) fn admit_legacy_connectable_advertising_recurring_event(
        &mut self,
        candidate: crate::le::advertising::connectable::LegacyConnectableAdvertisingEventCandidate,
    ) -> Result<
        crate::scheduler::core::LegacyConnectableAdvertisingPreSequence,
        crate::scheduler::core::LegacyConnectableAdvertisingEventPreparationFailure,
    > {
        self.runtime
            .admit_legacy_connectable_advertising_recurring_event(candidate)
    }

    /// Release a phase-locked recurrence before sequence authorization.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_pre_sequence(
        &mut self,
        admitted: crate::scheduler::core::LegacyConnectableAdvertisingPreSequence,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingCancelled,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_pre_sequence(admitted)
    }

    /// Apply the sole fresh recurring sequence sample.
    pub(crate) fn prepare_legacy_connectable_advertising_recurring_event(
        &mut self,
        admitted: crate::scheduler::core::LegacyConnectableAdvertisingPreSequence,
        sample: crate::ControllerTimeSample,
    ) -> Result<
        crate::scheduler::core::LegacyConnectableAdvertisingEventPrepared,
        crate::scheduler::core::LegacyConnectableAdvertisingEventPreparationFailure,
    > {
        self.runtime.prepare_legacy_connectable_advertising_event(
            admitted,
            crate::scheduler::core::LegacyConnectableAdvertisingSequenceObservation { sample },
        )
    }

    /// Retry only the empty-list join after sequence authorization.
    pub(crate) fn merge_legacy_connectable_advertising_recurring_event(
        &mut self,
        prepared: crate::scheduler::core::LegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        crate::scheduler::core::LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        crate::scheduler::core::LegacyConnectableAdvertisingEmptySchedulerMergeFailure,
    > {
        self.runtime
            .prepare_legacy_connectable_advertising_empty_list_merge(prepared)
    }

    /// Release one sequence-ready recurrence before list publication.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_event(
        &mut self,
        prepared: crate::scheduler::core::LegacyConnectableAdvertisingEventPrepared,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingCancelled,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingCancellationInvariant,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_event(prepared)
    }

    /// Undo one unpublished recurring empty-list merge.
    pub(crate) fn cancel_legacy_connectable_advertising_recurring_merge(
        &mut self,
        merged: crate::scheduler::core::LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingCancelled,
        crate::scheduler::core::LegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    > {
        self.runtime
            .cancel_legacy_connectable_advertising_empty_list_merge(merged)
    }

    /// Rebuild one successor from the completed event's nominal phase.
    pub(crate) fn prepare_legacy_advertising_recurring_candidate(
        &self,
        scheduled: crate::le::advertising::LegacyAdvertisingNextEventScheduled<'static>,
    ) -> Result<
        crate::le::advertising::LegacyAdvertisingRecurringEventCandidate<'static>,
        LegacyAdvertisingRecurringCandidateFailure,
    > {
        let Some(epoch) = *self.scheduler_epoch else {
            return Err(
                LegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(scheduled),
            );
        };
        scheduled
            .prepare_candidate(
                self.legacy_advertising_resources.default_tx_power_dbm(),
                crate::LegacyAdvertisingRecurringTimingObservation::new(epoch),
                self.runtime.scheduler_config(),
            )
            .map_err(LegacyAdvertisingRecurringCandidateFailure::Preparation)
    }

    /// Retry only the finite packet/reset/timing projection of a successor.
    pub(crate) fn retry_legacy_advertising_recurring_candidate(
        &self,
        failure: crate::le::advertising::LegacyAdvertisingRecurringPreparationFailure<'static>,
    ) -> Result<
        crate::le::advertising::LegacyAdvertisingRecurringEventCandidate<'static>,
        LegacyAdvertisingRecurringCandidateFailure,
    > {
        let Some(epoch) = *self.scheduler_epoch else {
            return Err(LegacyAdvertisingRecurringCandidateFailure::Preparation(
                failure,
            ));
        };
        failure
            .retry(
                self.legacy_advertising_resources.default_tx_power_dbm(),
                crate::LegacyAdvertisingRecurringTimingObservation::new(epoch),
                self.runtime.scheduler_config(),
            )
            .map_err(LegacyAdvertisingRecurringCandidateFailure::Preparation)
    }

    /// Reserve one recurring advertising window in the retained timeline.
    pub(crate) fn admit_legacy_advertising_recurring_candidate(
        &mut self,
        candidate: crate::le::advertising::LegacyAdvertisingRecurringEventCandidate<'static>,
    ) -> Result<
        crate::scheduler::LegacyAdvertisingRecurringPreSequence<'static>,
        crate::scheduler::LegacyAdvertisingRecurringEventPreparationFailure<'static>,
    > {
        self.runtime
            .admit_legacy_advertising_recurring_event(candidate)
    }

    /// Release a recurring timeline reservation before sequence authorization.
    pub(crate) fn cancel_legacy_advertising_recurring_pre_sequence(
        &mut self,
        admitted: crate::scheduler::LegacyAdvertisingRecurringPreSequence<'static>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'static> {
        self.runtime
            .cancel_legacy_advertising_recurring_pre_sequence(admitted)
            .into_parts()
            .0
    }

    /// Release a sequence-ready recurring descriptor before scheduler-list publication.
    pub(crate) fn cancel_legacy_advertising_recurring_prepared(
        &mut self,
        prepared: crate::scheduler::LegacyAdvertisingEventPrepared<'static>,
    ) -> crate::le::advertising::LegacyAdvertisingCancelled<'static> {
        self.runtime.cancel_legacy_advertising_first_event(prepared)
    }

    /// Undo one unpublished recurring empty-list merge.
    pub(crate) fn cancel_legacy_advertising_recurring_merge(
        &mut self,
        merged: crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    ) -> Result<
        crate::le::advertising::LegacyAdvertisingCancelled<'static>,
        crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    > {
        self.runtime
            .cancel_legacy_advertising_empty_list_merge(merged)
            .map(|prepared| self.runtime.cancel_legacy_advertising_first_event(prepared))
    }

    /// Return an unpublished disabled successor to this exact runtime.
    pub(crate) fn restore_legacy_advertising_cancelled_disabled(
        &mut self,
        cancelled: crate::le::advertising::LegacyAdvertisingCancelled<'static>,
    ) -> crate::LegacyAdvertisingCancelledRestoreOutcome<'static> {
        self.legacy_advertising_resources
            .restore_cancelled(cancelled)
    }

    /// Observe one abandoned Controller-time request before publishing stop success.
    pub(crate) fn drain_abandoned_recurring_controller_time(
        &mut self,
    ) -> Result<crate::controller::ControllerTimeOrphanDrainStep, ControllerSchedulerCurrentError>
    {
        match drain_controller_time_orphan(self) {
            Ok(ControllerTimePendingOrphanStep::Idle) => {
                Ok(crate::controller::ControllerTimeOrphanDrainStep::Idle)
            }
            Ok(ControllerTimePendingOrphanStep::Waiting) => {
                Ok(crate::controller::ControllerTimeOrphanDrainStep::Waiting)
            }
            Ok(ControllerTimePendingOrphanStep::Drained) => {
                Ok(crate::controller::ControllerTimeOrphanDrainStep::Drained)
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Retry the empty-list join without rebuilding or reauthorizing the event.
    pub(crate) fn merge_legacy_advertising_recurring_event(
        &mut self,
        prepared: crate::scheduler::LegacyAdvertisingEventPrepared<'static>,
    ) -> Result<
        crate::scheduler::LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
        crate::scheduler::LegacyAdvertisingEmptySchedulerMergeFailure<'static>,
    > {
        self.runtime
            .prepare_legacy_advertising_empty_list_merge(prepared)
    }

    /// Advance one finite scheduler lock/modify transaction.
    pub fn step_scheduler_lock_modify(
        &mut self,
        event: crate::scheduler::SchedulerLockModifyEvent,
    ) -> crate::scheduler::SchedulerLockModifyWorkerStep {
        self.runtime.step_scheduler_lock_modify(event)
    }

    /// Admit one published advertising graph through the common RUN suffix.
    pub(crate) fn start_legacy_advertising_scheduler<'a>(
        &mut self,
        head: crate::scheduler::LegacyAdvertisingSchedulerHeadPublished<'a>,
    ) -> Result<
        crate::scheduler::core::SingleItemSchedulerRunning<
            crate::le::advertising::legacy::completion::LegacyAdvertisingCompletionRole<'a>,
        >,
        LegacyAdvertisingSchedulerStartFailure<'a, S::Error>,
    >
    where
        S: SchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(LegacyAdvertisingSchedulerStartFailure { error, head });
            }
        };
        let address = head.scheduler_item_address();
        let (item, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        let item = item.into_running(&run);
        Ok(crate::scheduler::core::SingleItemSchedulerRunning::new(
            item,
            run,
            reservation,
        ))
    }

    /// Admit one published passive-scanner graph through the common RUN suffix.
    pub(crate) fn start_passive_scan_scheduler(
        &mut self,
        head: crate::scheduler::PassiveScanSchedulerHeadPublished,
    ) -> Result<
        crate::scheduler::core::SingleItemSchedulerRunning<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
        PassiveScanSchedulerStartFailure<S::Error>,
    >
    where
        S: SchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => return Err(PassiveScanSchedulerStartFailure { error, head }),
        };
        let address = head.scheduler_item_address();
        let (graph, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        let graph = graph.into_running(&run);
        Ok(crate::scheduler::core::SingleItemSchedulerRunning::new(
            graph,
            run,
            reservation,
        ))
    }

    fn publish_scheduler_run_suffix(
        &mut self,
        address: oer_esp32s31_hal::types::BluetoothControllerSramAddress,
        publication: oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareListHeadPublished,
        interrupts: BluetoothSchedulerRunInterruptsPrepared,
    ) -> oer_esp32s31_hal::bluetooth::BluetoothSchedulerHardwareRunCommandPublished {
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
        ready: crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::legacy::completion::LegacyAdvertisingCompletionRole<'a>,
        >,
    ) -> crate::scheduler::core::LegacyAdvertisingSchedulerRecycleStep<'a> {
        self.runtime.recycle_legacy_advertising_completed(ready)
    }

    /// Extract completed PDUs and return scanner SRAM to CPU ownership.
    pub(crate) fn recycle_passive_scan_completed(
        &mut self,
        ready: crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
    ) -> crate::scheduler::core::PassiveScanSchedulerRecycleStep {
        self.runtime.recycle_passive_scan_completed(ready)
    }

    pub(crate) fn restore_passive_scan_recycled(
        &mut self,
        recycled: crate::scheduler::core::PassiveScanSchedulerRecycled,
    ) -> Result<
        (
            oer_esp32s31_bluetooth_memory::LeReceivedBatch,
            oer_esp32s31_bluetooth_memory::PassiveScanSchedulerItemCompletionStatus,
        ),
        crate::le::scanning::passive::PassiveScanRuntimeRestoreFailure,
    > {
        self.passive_scan_resources.restore_recycled(recycled)
    }

    /// Reclaim one completed response-capable advertising event after generic removal.
    pub(crate) fn recycle_legacy_connectable_advertising_completed(
        &mut self,
        ready: crate::scheduler::core::SingleItemSchedulerSoftwareListRemovalReady<
            crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingCompletionRole,
        >,
    ) -> crate::le::advertising::connectable::completion::LegacyConnectableAdvertisingRecycleStep
    {
        self.runtime
            .recycle_legacy_connectable_advertising_completed(ready)
    }

    /// Restore both reusable runtime slots after an event accepted no connection.
    pub(crate) fn restore_legacy_connectable_advertising_no_connection(
        &mut self,
        outcome: crate::le::advertising::connectable::LegacyConnectableAdvertisingNoConnection,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingNoConnectionRestored,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingNoConnection,
    > {
        self.legacy_connectable_advertising_resources
            .restore_no_connection(outcome, self.peripheral_connection_resources)
    }

    /// Restore advertising SRAM while retaining the accepted peripheral allocation.
    pub(crate) fn restore_legacy_connectable_advertising_connection(
        &mut self,
        outcome: crate::le::advertising::connectable::LegacyConnectableAdvertisingConnectionAccepted,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingConnectionTransfer,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingConnectionAccepted,
    > {
        self.legacy_connectable_advertising_resources
            .restore_connection_accepted(outcome)
    }

    /// Cancel an accepted connection only at the explicit pre-publication Reset boundary.
    pub(crate) fn cancel_legacy_connectable_advertising_connection_for_reset(
        &mut self,
        transfer: crate::le::advertising::connectable::LegacyConnectableAdvertisingConnectionTransfer,
    ) -> Result<
        crate::le::advertising::connectable::LegacyConnectableAdvertisingPeripheralResetEvidence,
        crate::le::advertising::connectable::LegacyConnectableAdvertisingPeripheralResetCancellationFailure,
    >{
        transfer.cancel_peripheral_for_reset(self.peripheral_connection_resources)
    }
}
