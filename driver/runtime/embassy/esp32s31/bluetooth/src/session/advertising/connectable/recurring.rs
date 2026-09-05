//! Thin Embassy drive for recurring connectable legacy advertising.
//!
//! The chip crate owns HCI ordering, scheduler admission, cancellation and
//! publication. This module only collapses immediately-ready transitions and
//! retains a phase across a caller-selected executor wait.

#![forbid(unsafe_code)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    LeControllerCommandEndpoint, LeControllerCommandReady, LeControllerEndpointMismatch,
    LeControllerResponsePending,
};
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingResetCompletionReady,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingNoConnectionReady,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingNoConnectionStopping,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady,
    BluetoothLegacyConnectableAdvertisingRecurringCommandReady,
    BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
    BluetoothLegacyConnectableAdvertisingRecurringHci,
    BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending,
    BluetoothLegacyConnectableAdvertisingRecurringHciFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringHciRetry,
    BluetoothLegacyConnectableAdvertisingRecurringResponsePending,
    BluetoothLegacyConnectableAdvertisingRecurringStopping,
    BluetoothLegacyConnectableAdvertisingStopOrder, BluetoothSchedulerRunInterruptStorage,
};

/// Terminal continuations for one finite recurring-radio drive.
///
/// The handler is borrowed so each mutually-exclusive lower branch can return
/// through the same actor policy without moving policy state into several
/// closures. Every affine radio/HCI owner is passed to exactly one method.
pub trait EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    Running,
> where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    type Output;

    fn wait_controller_time(
        &self,
        wait: EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
            'runtime,
            S,
            CAPACITY,
            Order,
        >,
    ) -> Self::Output;

    fn retry_graph_prepared(
        &self,
        retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
            BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_candidate(
        &self,
        retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
            BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_prepared(
        &self,
        retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
            BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn retry_merged(
        &self,
        retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
            BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
            S::Error,
            Order,
        >,
    ) -> Self::Output;

    fn running(&self, running: Running) -> Self::Output;

    fn fail_stop(
        &self,
        failure: BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            Order,
        >,
    ) -> Self::Output;
}

mod order_sealed {
    pub trait Sealed {}
}

impl order_sealed::Sealed for LeControllerCommandReady<'_, ()> {}
impl order_sealed::Sealed for LeControllerResponsePending<'_, ()> {}

/// HCI order axes that can publish one prepared recurrence.
pub trait EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<
    'runtime,
    S,
    const CAPACITY: usize,
>:
    BluetoothLegacyConnectableAdvertisingRecurringForwardOrder + order_sealed::Sealed + Sized where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Running;

    fn start_with<H>(
        state: BluetoothLegacyConnectableAdvertisingRecurringHci<
            BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
            Self,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
                'runtime,
                S,
                CAPACITY,
                Self,
                Self::Running,
            >;
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>
    for LeControllerCommandReady<'runtime, ()>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Running = BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>;

    fn start_with<H>(
        state: BluetoothLegacyConnectableAdvertisingRecurringCommandReady<
            'runtime,
            BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>
    for LeControllerResponsePending<'runtime, ()>
where
    S: BluetoothSchedulerRunInterruptStorage + 'runtime,
{
    type Running =
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>;

    fn start_with<H>(
        state: BluetoothLegacyConnectableAdvertisingRecurringResponsePending<
            'runtime,
            BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        >,
        handler: &H,
    ) -> H::Output
    where
        H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
pub struct EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
> where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >,
}

/// Affine evidence that the caller-selected controller-time source completed.
#[must_use = "resume the retained recurrence wait exactly once"]
pub struct EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady {
    _private: (),
}

impl<'runtime, S, const CAPACITY: usize, Order>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
{
    /// Wait without moving the hardware/controller owner into the future.
    pub async fn wait_for_recheck<R>(
        &self,
        recheck: R,
    ) -> EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady { _private: () }
    }

    /// Perform one bounded observation and collapse any newly-ready edges.
    pub fn resume_with<H>(
        self,
        _ready: EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeReady,
        handler: &H,
    ) -> H::Output
    where
        H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    completed: BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>,
    delay: AdvertisingDelay,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            LeControllerCommandReady<'runtime, ()>,
            BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
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
    completed: BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<
        'runtime,
        S,
        CAPACITY,
    >,
    delay: AdvertisingDelay,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            LeControllerResponsePending<'runtime, ()>,
            BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    retry: BluetoothLegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order>,
) -> BluetoothLegacyConnectableAdvertisingRecurringHci<Phase, Order> {
    retry.retry()
}

fn drive_legacy_connectable_advertising_recurring_scheduled_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
    H,
>(
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
            'runtime,
            S,
            CAPACITY,
            Order,
            Order::Running,
        >,
{
    state.begin_sequence_with(
        |pending| {
            handler.wait_controller_time(
                EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait {
                    state: pending,
                },
            )
        },
        |retry| handler.retry_candidate(retry),
        |failure| handler.fail_stop(failure),
    )
}

fn drive_sequence_ready_with<'runtime, S, const CAPACITY: usize, Order, H>(
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: EmbassyBluetoothLegacyConnectableAdvertisingRecurringForwardOrder<'runtime, S, CAPACITY>,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringDriveHandler<
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
    state: BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >,
) -> EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
    'runtime,
    S,
    CAPACITY,
    Order,
>
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait { state }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    /// Extract the still-aggregated chip owner for one HCI intake/publication.
    pub fn into_state(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    > {
        self.state
    }
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        LeControllerCommandReady<'runtime, ()>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringControllerTimeWait<
        'runtime,
        S,
        CAPACITY,
        LeControllerResponsePending<'runtime, ()>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
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
pub trait EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    type Output;

    fn disable_ready(
        &self,
        ready: BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
    ) -> Self::Output;

    fn reset_ready(
        &self,
        ready: BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
    ) -> Self::Output;

    fn wait_cancellation(
        &self,
        wait: EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> Self::Output;

    fn fail_stop(
        &self,
        failure: BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
            'runtime,
            S,
            CAPACITY,
            BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
        >,
    ) -> Self::Output;
}

/// An orphaned sequence request parked outside an awaited future.
#[must_use = "wait for a durable recheck and drain the exact stop order"]
pub struct EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending:
        BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending<'runtime, S, CAPACITY>,
}

/// Affine evidence that a caller-selected cancellation recheck completed.
#[must_use = "resume the retained cancellation wait exactly once"]
pub struct EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady {
    _private: (),
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub async fn wait_for_recheck<R>(
        &self,
        recheck: R,
    ) -> EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady { _private: () }
    }

    pub fn resume_with<H>(
        self,
        _ready: EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationReady,
        handler: &H,
    ) -> H::Output
    where
        H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
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
    stopped: BluetoothLegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
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
    stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
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
            stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<
                'runtime,
                $phase<'runtime, S, CAPACITY>,
            >,
            handler: &H,
        ) -> H::Output
        where
            S: BluetoothSchedulerRunInterruptStorage,
            H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<
                    'runtime,
                    S,
                    CAPACITY,
                >,
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
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_candidate_with,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_sequence_ready_with,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_prepared_with,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared
);
define_immediate_cancel!(
    cancel_legacy_connectable_advertising_recurring_merged_with,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged
);

/// Begin cancellation of an in-flight Controller-time sequence request.
pub fn cancel_legacy_connectable_advertising_recurring_sequence_pending_with<
    'runtime,
    S,
    const CAPACITY: usize,
    H,
>(
    stopping: BluetoothLegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
    >,
    handler: &H,
) -> H::Output
where
    S: BluetoothSchedulerRunInterruptStorage,
    H: EmbassyBluetoothLegacyConnectableAdvertisingRecurringStopHandler<'runtime, S, CAPACITY>,
{
    stopping.cancel_with(
        |pending| {
            handler.wait_cancellation(
                EmbassyBluetoothLegacyConnectableAdvertisingRecurringCancellationWait { pending },
            )
        },
        |failure| handler.fail_stop(failure),
    )
}
