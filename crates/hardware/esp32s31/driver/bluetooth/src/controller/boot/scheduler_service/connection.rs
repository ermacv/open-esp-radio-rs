//! Peripheral-connection scheduler service operations.

use core::ops::ControlFlow;

use crate::{
    le::peripheral::completion::{
        PeripheralConnectionCompletionRole, PeripheralConnectionRecycleOutcome,
    },
    scheduler::core::{SingleItemSchedulerRunning, SingleItemSchedulerSoftwareListRemovalReady},
};

use super::super::{
    ControllerPublishedTaskService, PeripheralConnectionSchedulerStartFailure,
    SchedulerRunInterruptStorage,
};

pub(crate) type PeripheralConnectionRecurringSchedulerStartOutcome<E> = ControlFlow<
    crate::scheduler::core::PeripheralConnectionRecurringSchedulerValidationFailure,
    ControlFlow<
        (
            E,
            crate::scheduler::core::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        ),
        ControlFlow<
            crate::scheduler::core::PeripheralConnectionRecurringSchedulerPublicationFailStop,
            SingleItemSchedulerRunning<PeripheralConnectionCompletionRole>,
        >,
    >,
>;

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
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
        merged: crate::scheduler::core::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
    ) -> PeripheralConnectionRecurringSchedulerStartOutcome<S::Error>
    where
        S: SchedulerRunInterruptStorage,
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
            SingleItemSchedulerRunning::new(event.into_running(&run), run, reservation),
        )))
    }

    /// Admit one RX/head-published connection event through the common RUN suffix.
    #[allow(
        clippy::result_large_err,
        reason = "stable-storage rejection returns the complete published connection graph"
    )]
    pub(crate) fn start_peripheral_connection_scheduler(
        &mut self,
        head: crate::scheduler::PeripheralConnectionSchedulerHeadPublished,
    ) -> Result<
        SingleItemSchedulerRunning<PeripheralConnectionCompletionRole>,
        PeripheralConnectionSchedulerStartFailure<S::Error>,
    >
    where
        S: SchedulerRunInterruptStorage,
    {
        let interrupts = match self.storage.prepare_scheduler_run_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(PeripheralConnectionSchedulerStartFailure { error, head });
            }
        };
        let address = head.scheduler_item_address();
        let (event, publication, reservation) = head.into_parts();
        let run = self.publish_scheduler_run_suffix(address, publication, interrupts);
        Ok(SingleItemSchedulerRunning::new(
            event.into_running(&run),
            run,
            reservation,
        ))
    }

    /// Reclaim event-local connection SRAM after the common removal-ready boundary.
    pub(crate) fn recycle_peripheral_connection_completed(
        &mut self,
        ready: SingleItemSchedulerSoftwareListRemovalReady<PeripheralConnectionCompletionRole>,
    ) -> PeripheralConnectionRecycleOutcome {
        self.runtime.recycle_peripheral_connection_completed(ready)
    }
}
