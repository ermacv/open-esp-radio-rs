//! Thin Embassy drive for recurring connectable legacy advertising.
//!
//! The chip crate owns HCI ordering, scheduler admission, cancellation and
//! publication. This module only collapses immediately-ready transitions and
//! retains a phase across a caller-selected executor wait.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    LeControllerCommandEndpoint, LeControllerCommandReady, LeControllerEndpointMismatch,
    LeControllerResponsePending,
};

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
        LegacyAdvertisingDisableResponsePending, LegacyAdvertisingResetCompletionReady,
        LegacyConnectableAdvertisingActiveResponsePending,
        LegacyConnectableAdvertisingHciActiveSession,
        LegacyConnectableAdvertisingNoConnectionReady,
        LegacyConnectableAdvertisingNoConnectionResponsePending,
        LegacyConnectableAdvertisingNoConnectionStopping,
        LegacyConnectableAdvertisingRecurrenceCandidate,
        LegacyConnectableAdvertisingRecurrenceGraphPrepared,
        LegacyConnectableAdvertisingRecurrenceMerged,
        LegacyConnectableAdvertisingRecurrencePrepared,
        LegacyConnectableAdvertisingRecurrenceScheduled,
        LegacyConnectableAdvertisingRecurrenceSequencePending,
        LegacyConnectableAdvertisingRecurrenceSequenceReady,
        LegacyConnectableAdvertisingRecurringCommandReady,
        LegacyConnectableAdvertisingRecurringHci,
        LegacyConnectableAdvertisingRecurringHciCancellationPending,
        LegacyConnectableAdvertisingRecurringHciFailStop,
        LegacyConnectableAdvertisingRecurringHciRetry,
        LegacyConnectableAdvertisingRecurringResponsePending,
        LegacyConnectableAdvertisingRecurringStopping, LegacyConnectableAdvertisingStopOrder,
    },
};

/// Terminal continuations for one finite recurring-radio drive.
///
/// The handler is borrowed so each mutually-exclusive lower branch can return
/// through the same actor policy without moving policy state into several
/// closures. Every affine radio/HCI owner is passed to exactly one method.
pub trait LegacyConnectableAdvertisingRecurringDriveHandler<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    Running,
> where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    type Output;

    fn wait_controller_time(
        &self,
        wait: LegacyConnectableAdvertisingRecurringControllerTimeWait<'runtime, S, CAPACITY, Order>,
    ) -> Self::Output;

    fn retry_graph_prepared(
        &self,
        retry: LegacyConnectableAdvertisingRecurringHciRetry<
            LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_candidate(
        &self,
        retry: LegacyConnectableAdvertisingRecurringHciRetry<
            LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_prepared(
        &self,
        retry: LegacyConnectableAdvertisingRecurringHciRetry<
            LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_merged(
        &self,
        retry: LegacyConnectableAdvertisingRecurringHciRetry<
            LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn running(&self, running: Running) -> Self::Output;

    fn fail_stop(
        &self,
        failure: LegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
    ) -> Self::Output;
}

mod order_sealed {
    pub trait Sealed {}
}

impl order_sealed::Sealed for LeControllerCommandReady<'_, ()> {}
impl order_sealed::Sealed for LeControllerResponsePending<'_, ()> {}

/// HCI order axes that can publish one prepared recurrence.
pub trait AdvertisingForwardOrder<'runtime, S, const CAPACITY: usize>:
    BluetoothLegacyConnectableAdvertisingRecurringForwardOrder + order_sealed::Sealed + Sized
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Running;

    fn start_with<H>(
        state: LegacyConnectableAdvertisingRecurringHci<
            LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
            Self,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                Self,
                Self::Running,
            >;
}

impl<'runtime, S, const CAPACITY: usize> AdvertisingForwardOrder<'runtime, S, CAPACITY>
    for LeControllerCommandReady<'runtime, ()>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Running = LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>;

    fn start_with<H>(
        state: LegacyConnectableAdvertisingRecurringCommandReady<
            'runtime,
            LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                Self,
                Self::Running,
            >,
    {
        state.start_with(
            |running| handler.running(running),
            |retry| handler.retry_merged(retry),
            |failure| handler.fail_stop(failure),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize> AdvertisingForwardOrder<'runtime, S, CAPACITY>
    for LeControllerResponsePending<'runtime, ()>
where
    S: SchedulerRunInterruptStorage + 'runtime,
{
    type Running = LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>;

    fn start_with<H>(
        state: LegacyConnectableAdvertisingRecurringResponsePending<
            'runtime,
            LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                Self,
                Self::Running,
            >,
    {
        state.start_with(
            |running| handler.running(running),
            |retry| handler.retry_merged(retry),
            |failure| handler.fail_stop(failure),
        )
    }
}

/// A sequence-lock request parked outside the caller's awaited future.
#[must_use = "wait for a durable recheck and resume the exact recurrence phase"]
pub struct LegacyConnectableAdvertisingRecurringControllerTimeWait<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
> where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >,
}

/// Affine evidence that the caller-selected controller-time source completed.
#[must_use = "resume the retained recurrence wait exactly once"]
pub struct LegacyConnectableAdvertisingRecurringControllerTimeReady {
    _private: (),
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringControllerTimeWait<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
{
    /// Wait without moving the hardware/controller owner into the future.
    pub async fn wait_for_recheck<R>(
        &self,
        recheck: R,
    ) -> LegacyConnectableAdvertisingRecurringControllerTimeReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        LegacyConnectableAdvertisingRecurringControllerTimeReady { _private: () }
    }

    /// Perform one bounded observation and collapse any newly-ready edges.
    pub fn resume_with<H>(
        self,
        _ready: LegacyConnectableAdvertisingRecurringControllerTimeReady,
        handler: &H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                Order,
                Order::Running,
            >,
    {
        self.state.recheck_with(
            |waiting| handler.wait_controller_time(Self { state: waiting }),
            |ready| drive_sequence_ready_with(ready, handler),
            |failure| handler.fail_stop(failure),
        )
    }
}

/// Begin recurrence with the caller-provided fresh advertising delay.
pub fn begin_legacy_connectable_advertising_recurring_command_ready_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    completed: LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            LeControllerCommandReady<'runtime, ()>,
            LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        >,
{
    drive_legacy_connectable_advertising_recurring_scheduled_with(
        completed.begin_recurring(delay),
        handler,
    )
}

/// Begin recurrence without pausing an earlier backpressured response.
pub fn begin_legacy_connectable_advertising_recurring_response_pending_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    completed: LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            LeControllerResponsePending<'runtime, ()>,
            LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        >,
{
    drive_legacy_connectable_advertising_recurring_scheduled_with(
        completed.begin_recurring(delay),
        handler,
    )
}

/// Resume a retained timing retry without generating another delay.
pub fn drive_legacy_connectable_advertising_recurring_graph_prepared_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.retry_timing_with(
        |candidate| drive_candidate_with(candidate, handler),
        |retry| handler.retry_graph_prepared(retry),
        |failure| handler.fail_stop(failure),
    )
}

/// Resume a retained admission retry without generating another delay.
pub fn drive_legacy_connectable_advertising_recurring_candidate_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    drive_candidate_with(state, handler)
}

/// Resume a retained scheduler-merge retry without generating another delay.
pub fn drive_legacy_connectable_advertising_recurring_prepared_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    drive_prepared_with(state, handler)
}

/// Resume a retained atomic-start retry without generating another delay.
pub fn drive_legacy_connectable_advertising_recurring_merged_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    Order::start_with(state, handler)
}

/// Reopen a retry envelope for HCI progress without retrying the radio edge.
///
/// The returned value is still the same aggregate recurrence phase and HCI
/// order. Calling this function performs no scheduler or MMIO operation. An
/// actor can therefore service a pending response or accept Disable/Reset
/// before separately passing the phase to the matching `drive_*_with` entry.
pub fn retain_legacy_connectable_advertising_recurring_retry_for_hci<Phase, E, Order>(
    retry: LegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order>,
) -> LegacyConnectableAdvertisingRecurringHci<Phase, Order> {
    retry.retry()
}

fn drive_legacy_connectable_advertising_recurring_scheduled_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.prepare_with(
        |candidate| drive_candidate_with(candidate, handler),
        |retry| handler.retry_graph_prepared(retry),
        |failure| handler.fail_stop(failure),
    )
}

fn drive_candidate_with<'runtime, S, const CAPACITY: usize, Order, H>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.begin_sequence_with(
        |pending| {
            handler.wait_controller_time(LegacyConnectableAdvertisingRecurringControllerTimeWait {
                state: pending,
            })
        },
        |retry| handler.retry_candidate(retry),
        |failure| handler.fail_stop(failure),
    )
}

fn drive_sequence_ready_with<'runtime, S, const CAPACITY: usize, Order, H>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.prepare_with(
        |prepared| drive_prepared_with(prepared, handler),
        |retry| handler.retry_candidate(retry),
    )
}

fn drive_prepared_with<'runtime, S, const CAPACITY: usize, Order, H>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    Order: AdvertisingForwardOrder<'runtime, S, CAPACITY>,
    H: LegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.merge_with(
        |merged| Order::start_with(merged, handler),
        |retry| handler.retry_prepared(retry),
    )
}

/// Re-park a valid aggregate after one non-blocking HCI operation.
pub fn retain_legacy_connectable_advertising_recurring_controller_time<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
>(
    state: LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >,
) -> LegacyConnectableAdvertisingRecurringControllerTimeWait<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    LegacyConnectableAdvertisingRecurringControllerTimeWait { state }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringControllerTimeWait<'runtime, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    /// Extract the still-aggregated chip owner for one HCI intake/publication.
    pub fn into_state(
        self,
    ) -> LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    > {
        self.state
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        LeControllerCommandReady<'runtime, ()>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        self.state.wait_command_available(controller).await
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        LeControllerResponsePending<'runtime, ()>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        self.state.wait_response_capacity(controller).await
    }
}

/// Stop completion and cancellation-drain continuations.
pub trait LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    type Output;

    fn disable_ready(
        &self,
        ready: LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
    ) -> Self::Output;

    fn reset_ready(
        &self,
        ready: LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
    ) -> Self::Output;

    fn wait_cancellation(
        &self,
        wait: LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>,
    ) -> Self::Output;

    fn fail_stop(
        &self,
        failure: LegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            LegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ) -> Self::Output;
}

/// An orphaned sequence request parked outside an awaited future.
#[must_use = "wait for a durable recheck and drain the exact stop order"]
pub struct LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pending: LegacyConnectableAdvertisingRecurringHciCancellationPending<'runtime, S, CAPACITY>,
}

/// Affine evidence that a caller-selected cancellation recheck completed.
#[must_use = "resume the retained cancellation wait exactly once"]
pub struct LegacyConnectableAdvertisingRecurringCancellationReady {
    _private: (),
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_for_recheck<R>(
        &self,
        recheck: R,
    ) -> LegacyConnectableAdvertisingRecurringCancellationReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        LegacyConnectableAdvertisingRecurringCancellationReady { _private: () }
    }

    pub fn resume_with<H>(
        self,
        _ready: LegacyConnectableAdvertisingRecurringCancellationReady,
        handler: &H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
    {
        self.pending.recheck_with(
            |pending| handler.wait_cancellation(Self { pending }),
            |ready| handler.disable_ready(ready),
            |ready| handler.reset_ready(ready),
            |failure| handler.fail_stop(failure),
        )
    }
}

/// Finish a stop whose advertising graph was already restored before recurrence.
pub fn finish_legacy_connectable_advertising_no_connection_stopping_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    stopped: LegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    H: LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
{
    stopped.finish_with(
        |ready| handler.disable_ready(ready),
        |ready| handler.reset_ready(ready),
        |failure| handler.fail_stop(failure),
    )
}

/// Cancel before any scheduler graph was prepared.
pub fn cancel_legacy_connectable_advertising_recurring_scheduled_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    stopping: LegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    H: LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
{
    stopping.cancel_with(
        |ready| handler.disable_ready(ready),
        |ready| handler.reset_ready(ready),
        |failure| handler.fail_stop(failure),
    )
}

macro_rules! define_immediate_cancel {
    ($name:ident, $phase:ident) => {
        pub fn $name<'runtime, S, const CAPACITY: usize, H>(
            stopping: LegacyConnectableAdvertisingRecurringStopping<
                'runtime,
                $phase<'runtime, S, CAPACITY>,
            >,
            handler: &H,
        ) -> H::Output
        where
            S: SchedulerRunInterruptStorage,
            H: LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
        {
            stopping.cancel_with(
                |ready| handler.disable_ready(ready),
                |ready| handler.reset_ready(ready),
                |failure| handler.fail_stop(failure),
            )
        }
    };
}

define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_graph_prepared_with,
    LegacyConnectableAdvertisingRecurrenceGraphPrepared
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    LegacyConnectableAdvertisingRecurrenceCandidate
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_sequence_ready_with,
    LegacyConnectableAdvertisingRecurrenceSequenceReady
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    LegacyConnectableAdvertisingRecurrencePrepared
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_merged_with,
    LegacyConnectableAdvertisingRecurrenceMerged
);

/// Begin cancellation of an in-flight Controller-time sequence request.
pub fn cancel_legacy_connectable_advertising_recurring_sequence_pending_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    stopping: LegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
    >,
    handler: &H,
) -> H::Output
where
    S: SchedulerRunInterruptStorage,
    H: LegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
{
    stopping.cancel_with(
        |pending| {
            handler.wait_cancellation(LegacyConnectableAdvertisingRecurringCancellationWait {
                pending,
            })
        },
        |failure| handler.fail_stop(failure),
    )
}
