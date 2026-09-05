//! Peripheral-connection scheduler service operations.

use core::ops::ControlFlow;

use super::super::{
    BluetoothControllerPublishedTaskService, BluetoothPeripheralConnectionSchedulerStartFailure,
    BluetoothSchedulerRunInterruptStorage,
};
use crate::le::peripheral::completion::{
    BluetoothPeripheralConnectionCompletionRole, BluetoothPeripheralConnectionRecycleOutcome,
};
use crate::scheduler::core::{
    BluetoothSingleItemSchedulerRunning, BluetoothSingleItemSchedulerSoftwareListRemovalReady,
};

pub(crate) type BluetoothPeripheralConnectionRecurringSchedulerStartOutcome<E> = ControlFlow<
    crate::scheduler::core::BluetoothPeripheralConnectionRecurringSchedulerValidationFailure,
    ControlFlow<
        (
            E,
            crate::scheduler::core::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
        ),
        ControlFlow<
            crate::scheduler::core::BluetoothPeripheralConnectionRecurringSchedulerPublicationFailStop,
            BluetoothSingleItemSchedulerRunning<BluetoothPeripheralConnectionCompletionRole>,
        >,
    >,
>;

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Validate and run one recurring event through a single commit/publication edge.
    ///
    /// Common-list/head validation and stable interrupt preparation precede
    /// the LL successor and phase commit. An RX proof mismatch after that
    /// commit is a sealed fail-stop; only a validated RX join continues
    /// through head/event/RUN publication.
    ///
    /// The active peripheral actor currently stops at the first-event path;
    /// it does not yet drive completed events through this recurrence entry.
    #[expect(
        dead_code,
        reason = "the active peripheral actor does not yet compose recurring scheduler publication"
    )]
    pub(crate) fn start_peripheral_connection_recurring_scheduler(
        &mut self,
        merged: crate::scheduler::core::BluetoothPeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> BluetoothPeripheralConnectionRecurringSchedulerStartOutcome<S::Error>
    where
        S: BluetoothSchedulerRunInterruptStorage,
    {
        let validated = match self
            .runtime
            .validate_peripheral_connection_recurring_scheduler(merged)
        {
            ControlFlow::Continue(validated) => validated,
            ControlFlow::Break(failure) => return ControlFlow::Break(failure),
        };
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return ControlFlow::Continue(ControlFlow::Break((error, validated.into_merged())));
            }
        };
        let committed = validated.commit(interrupts);
        let (head, interrupts) = match self
            .runtime
            .publish_peripheral_connection_recurring_scheduler_head(committed)
        {
            ControlFlow::Continue(published) => published,
            ControlFlow::Break(failure) => {
                return ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure)));
            }
        };
        let address = head.scheduler_item_address();
        let (event, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(
            BluetoothSingleItemSchedulerRunning::new(event.into_running(&run), run, reservation),
        )))
    }

    /// Admit one RX/head-published connection event through the common RUN suffix.
    #[allow(
        clippy::result_large_err,
        reason = "stable-storage rejection returns the complete published connection graph"
    )]
    pub(crate) fn start_peripheral_connection_scheduler(
        &mut self,
        head: crate::BluetoothPeripheralConnectionSchedulerHeadPublished,
    ) -> Result<
        BluetoothSingleItemSchedulerRunning<BluetoothPeripheralConnectionCompletionRole>,
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
        Ok(BluetoothSingleItemSchedulerRunning::new(
            event.into_running(&run),
            run,
            reservation,
        ))
    }

    /// Reclaim event-local connection SRAM after the common removal-ready boundary.
    pub(crate) fn recycle_peripheral_connection_completed(
        &mut self,
        ready: BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            BluetoothPeripheralConnectionCompletionRole,
        >,
    ) -> BluetoothPeripheralConnectionRecycleOutcome {
        self.runtime.recycle_peripheral_connection_completed(ready)
    }
}
