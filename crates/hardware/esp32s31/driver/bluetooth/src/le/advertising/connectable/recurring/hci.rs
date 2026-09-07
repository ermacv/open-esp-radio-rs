//! HCI-order composition while preparing a connectable advertising successor.
//!
//! The recurrence phase and HCI order are independent affine axes. Controller
//! responses never pause radio preparation; Disable and Reset instead replace
//! the command axis with a stop order and cancel the exact unpublished phase.

#![forbid(unsafe_code)]

use crate::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        BluetoothLegacyConnectableAdvertisingRecurringFailStop,
        LegacyAdvertisingDisableResponsePending, LegacyAdvertisingResetCompletionReady,
        LegacyConnectableAdvertisingActiveResponsePending,
        LegacyConnectableAdvertisingHciActiveSession,
        LegacyConnectableAdvertisingNoConnectionReady,
        LegacyConnectableAdvertisingNoConnectionResponsePending,
        LegacyConnectableAdvertisingNoConnectionStopping,
        LegacyConnectableAdvertisingRecurrenceCancellationPending,
        LegacyConnectableAdvertisingRecurrenceCancelled,
        LegacyConnectableAdvertisingRecurrenceCandidate,
        LegacyConnectableAdvertisingRecurrenceGraphPrepared,
        LegacyConnectableAdvertisingRecurrenceMerged,
        LegacyConnectableAdvertisingRecurrencePrepared,
        LegacyConnectableAdvertisingRecurrenceScheduled,
        LegacyConnectableAdvertisingRecurrenceSequencePending,
        LegacyConnectableAdvertisingRecurrenceSequenceReady,
        LegacyConnectableAdvertisingRecurringFailStopCause,
        LegacyConnectableAdvertisingRecurringRetry,
        LegacyConnectableAdvertisingRecurringRetryCause, LegacyConnectableAdvertisingStopKind,
        LegacyConnectableAdvertisingStopOrder, LegacyConnectableAdvertisingStopping,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerEndpointMismatch, LeControllerResponsePending,
};

use oer_bluetooth_ll::advertising::AdvertisingDelay;

pub use crate::le::advertising::connectable::recurring::state::LegacyConnectableAdvertisingRecurringHci;

/// A recurrence phase retaining the authority to accept the next HCI command.
pub type LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase> =
    LegacyConnectableAdvertisingRecurringHci<Phase, LeControllerCommandReady<'runtime, ()>>;

/// A recurrence phase retaining one response under Controller-to-Host backpressure.
pub type LegacyConnectableAdvertisingRecurringResponsePending<'runtime, Phase> =
    LegacyConnectableAdvertisingRecurringHci<Phase, LeControllerResponsePending<'runtime, ()>>;

/// An unpublished recurrence phase retaining an ordered Disable or Reset.
pub type LegacyConnectableAdvertisingRecurringStopping<'runtime, Phase> =
    LegacyConnectableAdvertisingRecurringHci<
        Phase,
        LegacyConnectableAdvertisingStopOrder<'runtime>,
    >;

macro_rules! wrap_retry {
    ($order:expr, $retry:expr) => {
        LegacyConnectableAdvertisingRecurringHciRetry {
            retry: $retry,
            order: $order,
        }
    };
}

macro_rules! wrap_fail_stop {
    ($order:expr, $failure:expr) => {
        LegacyConnectableAdvertisingRecurringHciFailStop {
            failure: $failure,
            _order: $order,
        }
    };
}

mod forward_order_sealed {
    pub trait Sealed {}
}

impl forward_order_sealed::Sealed for LeControllerCommandReady<'_, ()> {}
impl forward_order_sealed::Sealed for LeControllerResponsePending<'_, ()> {}

/// HCI axes on which radio preparation may continue toward `RUN`.
///
/// The trait is sealed: accepting Disable or Reset changes the axis to
/// `Stopping`, which deliberately has no forward-preparation implementation.
pub trait BluetoothLegacyConnectableAdvertisingRecurringForwardOrder:
    forward_order_sealed::Sealed
{
}

impl BluetoothLegacyConnectableAdvertisingRecurringForwardOrder
    for LeControllerCommandReady<'_, ()>
{
}
impl BluetoothLegacyConnectableAdvertisingRecurringForwardOrder
    for LeControllerResponsePending<'_, ()>
{
}

/// Exact retryable recurrence phase with its unchanged HCI-order axis.
#[must_use = "inspect the cause, then retry the exact phase"]
pub struct LegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order> {
    retry: LegacyConnectableAdvertisingRecurringRetry<Phase, E>,
    order: Order,
}

impl<Phase, E, Order> LegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order> {
    pub const fn cause(&self) -> &LegacyConnectableAdvertisingRecurringRetryCause<E> {
        self.retry.cause()
    }

    pub fn retry(self) -> LegacyConnectableAdvertisingRecurringHci<Phase, Order> {
        LegacyConnectableAdvertisingRecurringHci::from_parts(self.retry.retry(), self.order)
    }
}

/// Sealed recurrence failure retaining the exact HCI-order axis.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct LegacyConnectableAdvertisingRecurringHciFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
> where
    S: SchedulerRunInterruptStorage,
{
    failure: BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    _order: Order,
}

impl<S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHciFailStop<'_, S, CAPACITY, Order>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectableAdvertisingRecurringFailStopCause {
        self.failure.cause()
    }
}

/// Endpoint mismatch retaining the complete command and recurrence phase.
#[must_use = "retain the command, phase and HCI authority together"]
pub struct LegacyConnectableAdvertisingRecurringCommandMismatch<'runtime, 'command, Phase> {
    _command: LeControllerClassifiedCommand<'runtime, 'command, Phase>,
}

/// Exhaustive continuation for one non-blocking recurring command intake.
///
/// Exactly one method consumes the handler. Every branch therefore retains the
/// complete phase, scratch buffer and HCI authority without a maximum-sized
/// result enum.
pub trait LegacyConnectableAdvertisingRecurringCommandHandler<'runtime, 'command, 'buffer, Phase>:
    Sized
{
    type Output;

    fn response_pending(
        self,
        state: LegacyConnectableAdvertisingRecurringResponsePending<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn stopping(
        self,
        state: LegacyConnectableAdvertisingRecurringStopping<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn command_mismatch(
        self,
        mismatch: LegacyConnectableAdvertisingRecurringCommandMismatch<'runtime, 'command, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn empty(
        self,
        state: LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn endpoint_mismatch(
        self,
        state: LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn channel_fault(
        self,
        state: LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    ) -> Self::Output;

    fn non_command(
        self,
        state: LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    ) -> Self::Output;
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Start the next portable interval without separating HCI authority.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> LegacyConnectableAdvertisingRecurringCommandReady<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    > {
        let ordered = self.into_ordered();
        let (completed, order) = ordered.into_parts();
        LegacyConnectableAdvertisingRecurringHci::from_parts(
            completed.begin_recurring(delay),
            order,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Start the next interval while retaining an unpublished Controller response.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> LegacyConnectableAdvertisingRecurringResponsePending<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    > {
        let transaction = self.into_transaction();
        let (completed, response) = transaction.into_parts();
        LegacyConnectableAdvertisingRecurringHci::from_parts(
            completed.begin_recurring(delay),
            response,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Complete the retained stop at the already-restored CPU boundary.
    pub fn finish_with<R>(
        self,
        disable: impl FnOnce(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>) -> R,
        reset: impl FnOnce(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        let (completed, order) = self.into_parts();
        completed.stop_recurrence_with(
            order,
            |order, cancelled| finish_cancelled_with(cancelled, order, disable, reset),
            |order, failure| {
                fail_stop(LegacyConnectableAdvertisingRecurringHciFailStop {
                    failure,
                    _order: order,
                })
            },
        )
    }
}

impl<'runtime, Phase> LegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase> {
    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_command_available(&self.order).await
    }

    /// Route one command with the portable active-advertising policy.
    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
        H,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
        handler: H,
    ) -> H::Output
    where
        H: LegacyConnectableAdvertisingRecurringCommandHandler<'runtime, 'command, 'buffer, Phase>,
    {
        let ordered = self.order.map_owner(|()| self.phase);
        match controller.try_receive_classified_command_with_buffer(ordered, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                match controller.route_active_legacy_advertising_classified_command(command) {
                    HciCommandRoute::ResponsePending(transaction) => {
                        let (phase, response) = transaction.into_parts();
                        handler.response_pending(
                            LegacyConnectableAdvertisingRecurringHci::from_parts(phase, response),
                            buffer,
                        )
                    }
                    HciCommandRoute::Disable(disable) => {
                        let (phase, disable) = disable.into_parts();
                        handler.stopping(
                            LegacyConnectableAdvertisingRecurringHci::from_parts(
                                phase,
                                LegacyConnectableAdvertisingStopOrder::Disable(disable),
                            ),
                            buffer,
                        )
                    }
                    HciCommandRoute::ResetBarrier(barrier) => {
                        let (phase, barrier) = barrier.into_parts();
                        handler.stopping(
                            LegacyConnectableAdvertisingRecurringHci::from_parts(
                                phase,
                                LegacyConnectableAdvertisingStopOrder::Reset(barrier),
                            ),
                            buffer,
                        )
                    }
                    HciCommandRoute::EndpointMismatch(command) => handler.command_mismatch(
                        LegacyConnectableAdvertisingRecurringCommandMismatch { _command: command },
                        buffer,
                    ),
                }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (phase, order) = ready.into_parts();
                handler.empty(
                    LegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    buffer,
                )
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (phase, order) = ready.into_parts();
                handler.endpoint_mismatch(
                    LegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    buffer,
                )
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => {
                let (phase, order) = ready.into_parts();
                handler.channel_fault(
                    LegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    buffer,
                    error,
                )
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (phase, order) = ready.into_parts();
                handler.non_command(
                    LegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    frame,
                )
            }
        }
    }
}

impl<Phase> LegacyConnectableAdvertisingRecurringStopping<'_, Phase> {
    pub const fn stop_kind(&self) -> LegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn prepare_with<R>(
        self,
        ready: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.prepare_with(
            self.order,
            |order, phase| {
                ready(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn retry_timing_with<R>(
        self,
        ready: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                SelfPhase<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.retry_timing_with(
            self.order,
            |order, phase| {
                ready(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

type SelfPhase<'runtime, S, const CAPACITY: usize> =
    LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>;

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn begin_sequence_with<R>(
        self,
        waiting: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.begin_sequence_with(
            self.order,
            |order, phase| {
                waiting(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn recheck_with<R>(
        self,
        waiting: impl FnOnce(Self) -> R,
        ready: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.recheck_with(
            self.order,
            |order, phase| {
                waiting(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, phase| {
                ready(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn prepare_with<R>(
        self,
        ready: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
    ) -> R {
        self.phase.prepare_with(
            self.order,
            |order, phase| {
                ready(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    LegacyConnectableAdvertisingRecurringHci<
        LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: SchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn merge_with<R>(
        self,
        ready: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHci<
                LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
    ) -> R {
        self.phase.merge_with(
            self.order,
            |order, phase| {
                ready(LegacyConnectableAdvertisingRecurringHci::from_parts(
                    phase, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringCommandReady<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                LeControllerCommandReady<'runtime, ()>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LeControllerCommandReady<'runtime, ()>,
            >,
        ) -> R,
    ) -> R {
        self.phase.start_with(
            self.order,
            |order, active| {
                running(LegacyConnectableAdvertisingHciActiveSession::from_ordered(
                    order.map_owner(|()| active),
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringResponsePending<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(
            LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                LeControllerResponsePending<'runtime, ()>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LeControllerResponsePending<'runtime, ()>,
            >,
        ) -> R,
    ) -> R {
        self.phase.start_with(
            self.order,
            |response, active| {
                running(LegacyConnectableAdvertisingActiveResponsePending::new(
                    response.map_owner(|()| active),
                ))
            },
            |response, retry_owner| retry(wrap_retry!(response, retry_owner)),
            |response, failure| fail_stop(wrap_fail_stop!(response, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>) -> R,
        retry: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciRetry<
                LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.phase.start_with(
            self.order,
            |order, active| {
                running(LegacyConnectableAdvertisingStopping::from_parts(
                    active, order,
                ))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

/// Stop order waiting for an abandoned Controller-time request to drain.
#[must_use = "recheck until the cancellation reaches an exact CPU boundary"]
pub struct LegacyConnectableAdvertisingRecurringHciCancellationPending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    pending: LegacyConnectableAdvertisingRecurrenceCancellationPending<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringHciCancellationPending<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> LegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

fn finish_cancelled_with<'runtime, S, const CAPACITY: usize, R>(
    cancelled: LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
    disable: impl FnOnce(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>) -> R,
    reset: impl FnOnce(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
) -> R
where
    S: SchedulerRunInterruptStorage,
{
    let (task, _set, _phase, _scheduler_status, _rejected_packets) = cancelled.into_parts();
    match order {
        LegacyConnectableAdvertisingStopOrder::Disable(deferred) => disable(
            LegacyAdvertisingDisableResponsePending::from_cancelled(task, deferred),
        ),
        LegacyConnectableAdvertisingStopOrder::Reset(barrier) => reset(
            LegacyAdvertisingResetCompletionReady::from_cancelled(task, barrier),
        ),
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<R>(
        self,
        disable: impl FnOnce(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>) -> R,
        reset: impl FnOnce(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.phase.cancel_with(
            self.order,
            |order, cancelled| finish_cancelled_with(cancelled, order, disable, reset),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

macro_rules! impl_immediate_stop_cancellation {
    ($phase:ident) => {
        impl<'runtime, S, const CAPACITY: usize>
            LegacyConnectableAdvertisingRecurringStopping<
                'runtime,
                $phase<'runtime, S, CAPACITY>,
            >
        where
            S: SchedulerRunInterruptStorage,
        {
            pub fn cancel_with<R>(
                self,
                disable: impl FnOnce(
                    LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
                ) -> R,
                reset: impl FnOnce(
                    LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
                ) -> R,
                fail_stop: impl FnOnce(
                    LegacyConnectableAdvertisingRecurringHciFailStop<
                        'runtime,
                        S,
                        CAPACITY,
                        LegacyConnectableAdvertisingStopOrder<'runtime>,
                    >,
                ) -> R,
            ) -> R {
                self.phase.cancel_with(
                    self.order,
                    |order, cancelled| finish_cancelled_with(cancelled, order, disable, reset),
                    |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
                )
            }
        }
    };
}

impl_immediate_stop_cancellation!(LegacyConnectableAdvertisingRecurrenceGraphPrepared);
impl_immediate_stop_cancellation!(LegacyConnectableAdvertisingRecurrenceCandidate);
impl_immediate_stop_cancellation!(LegacyConnectableAdvertisingRecurrenceSequenceReady);
impl_immediate_stop_cancellation!(LegacyConnectableAdvertisingRecurrencePrepared);
impl_immediate_stop_cancellation!(LegacyConnectableAdvertisingRecurrenceMerged);

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
    >
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<R>(
        self,
        draining: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciCancellationPending<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.phase.cancel_with(
            self.order,
            |order, pending| {
                draining(
                    LegacyConnectableAdvertisingRecurringHciCancellationPending { pending, order },
                )
            },
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurringHciCancellationPending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn recheck_with<R>(
        self,
        waiting: impl FnOnce(Self) -> R,
        disable: impl FnOnce(LegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>) -> R,
        reset: impl FnOnce(LegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            LegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                LegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.pending.recheck_with(
            self.order,
            |order, pending| waiting(Self { pending, order }),
            |order, cancelled| finish_cancelled_with(cancelled, order, disable, reset),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}
