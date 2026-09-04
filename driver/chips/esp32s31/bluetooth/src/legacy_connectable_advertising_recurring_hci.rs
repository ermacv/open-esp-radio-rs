//! HCI-order composition while preparing a connectable advertising successor.
//!
//! The recurrence phase and HCI order are independent affine axes. Controller
//! responses never pause radio preparation; Disable and Reset instead replace
//! the command axis with a stop order and cancel the exact unpublished phase.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerEndpointMismatch, LeControllerResponsePending,
    LeControllerResponsePublication,
};
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;

use crate::{
    BluetoothLegacyAdvertisingDisableResponsePending,
    BluetoothLegacyAdvertisingResetCompletionReady,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingNoConnectionReady,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingNoConnectionStopping,
    BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending,
    BluetoothLegacyConnectableAdvertisingRecurrenceCancelled,
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady,
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRecurringRetry,
    BluetoothLegacyConnectableAdvertisingRecurringRetryCause,
    BluetoothLegacyConnectableAdvertisingStopKind, BluetoothLegacyConnectableAdvertisingStopOrder,
    BluetoothLegacyConnectableAdvertisingStopping, BluetoothSchedulerRunInterruptStorage,
};

pub use crate::legacy_connectable_advertising_recurring_hci_state::BluetoothLegacyConnectableAdvertisingRecurringHci;

/// A recurrence phase retaining the authority to accept the next HCI command.
pub type BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase> =
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        Phase,
        LeControllerCommandReady<'runtime, ()>,
    >;

/// A recurrence phase retaining one response under Controller-to-Host backpressure.
pub type BluetoothLegacyConnectableAdvertisingRecurringResponsePending<'runtime, Phase> =
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        Phase,
        LeControllerResponsePending<'runtime, ()>,
    >;

/// An unpublished recurrence phase retaining an ordered Disable or Reset.
pub type BluetoothLegacyConnectableAdvertisingRecurringStopping<'runtime, Phase> =
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        Phase,
        BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
    >;

macro_rules! wrap_retry {
    ($order:expr, $retry:expr) => {
        BluetoothLegacyConnectableAdvertisingRecurringHciRetry {
            retry: $retry,
            order: $order,
        }
    };
}

macro_rules! wrap_fail_stop {
    ($order:expr, $failure:expr) => {
        BluetoothLegacyConnectableAdvertisingRecurringHciFailStop {
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
pub struct BluetoothLegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order> {
    retry: BluetoothLegacyConnectableAdvertisingRecurringRetry<Phase, E>,
    order: Order,
}

impl<Phase, E, Order> BluetoothLegacyConnectableAdvertisingRecurringHciRetry<Phase, E, Order> {
    pub const fn cause(&self) -> &BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E> {
        self.retry.cause()
    }

    pub fn retry(self) -> BluetoothLegacyConnectableAdvertisingRecurringHci<Phase, Order> {
        BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
            self.retry.retry(),
            self.order,
        )
    }
}

/// Sealed recurrence failure retaining the exact HCI-order axis.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
    Order,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    failure: BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    _order: Order,
}

impl<S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<'_, S, CAPACITY, Order>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectableAdvertisingRecurringFailStopCause {
        self.failure.cause()
    }
}

/// Endpoint mismatch retaining the complete command and recurrence phase.
#[must_use = "retain the command, phase and HCI authority together"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<'runtime, 'command, Phase>
{
    _command: LeControllerClassifiedCommand<'runtime, 'command, Phase>,
}

/// Exhaustive continuation for one non-blocking recurring command intake.
///
/// Exactly one method consumes the handler. Every branch therefore retains the
/// complete phase, scratch buffer and HCI authority without a maximum-sized
/// result enum.
pub trait BluetoothLegacyConnectableAdvertisingRecurringCommandHandler<
    'runtime,
    'command,
    'buffer,
    Phase,
>: Sized
{
    type Output;

    fn response_pending(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringResponsePending<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn stopping(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringStopping<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn command_mismatch(
        self,
        mismatch: BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch<
            'runtime,
            'command,
            Phase,
        >,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn empty(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn endpoint_mismatch(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
    ) -> Self::Output;

    fn channel_fault(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    ) -> Self::Output;

    fn non_command(
        self,
        state: BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    ) -> Self::Output;
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Start the next portable interval without separating HCI authority.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyConnectableAdvertisingRecurringCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    > {
        let ordered = self.into_ordered();
        let (completed, order) = ordered.into_parts();
        BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
            completed.begin_recurring(delay),
            order,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Start the next interval while retaining an unpublished Controller response.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyConnectableAdvertisingRecurringResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    > {
        let transaction = self.into_transaction();
        let (completed, response) = transaction.into_parts();
        BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
            completed.begin_recurring(delay),
            response,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Complete the retained stop at the already-restored CPU boundary.
    pub fn finish_with<R>(
        self,
        disable: impl FnOnce(
            BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        reset: impl FnOnce(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        let (completed, order) = self.into_parts();
        completed.stop_recurrence_with(
            order,
            |order, cancelled| finish_cancelled_with(cancelled, order, disable, reset),
            |order, failure| {
                fail_stop(BluetoothLegacyConnectableAdvertisingRecurringHciFailStop {
                    failure,
                    _order: order,
                })
            },
        )
    }
}

impl<'runtime, Phase> BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase> {
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
        H: BluetoothLegacyConnectableAdvertisingRecurringCommandHandler<
                'runtime,
                'command,
                'buffer,
                Phase,
            >,
    {
        let ordered = self.order.map_owner(|()| self.phase);
        match controller.try_receive_classified_command_with_buffer(ordered, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                match controller.route_active_legacy_advertising_classified_command(command) {
                    HciCommandRoute::ResponsePending(transaction) => {
                        let (phase, response) = transaction.into_parts();
                        handler.response_pending(
                            BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
                                phase, response,
                            ),
                            buffer,
                        )
                    }
                    HciCommandRoute::Disable(disable) => {
                        let (phase, disable) = disable.into_parts();
                        handler.stopping(
                            BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
                                phase,
                                BluetoothLegacyConnectableAdvertisingStopOrder::Disable(disable),
                            ),
                            buffer,
                        )
                    }
                    HciCommandRoute::ResetBarrier(barrier) => {
                        let (phase, barrier) = barrier.into_parts();
                        handler.stopping(
                            BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(
                                phase,
                                BluetoothLegacyConnectableAdvertisingStopOrder::Reset(barrier),
                            ),
                            buffer,
                        )
                    }
                    HciCommandRoute::EndpointMismatch(command) => handler.command_mismatch(
                        BluetoothLegacyConnectableAdvertisingRecurringCommandMismatch {
                            _command: command,
                        },
                        buffer,
                    ),
                }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                let (phase, order) = ready.into_parts();
                handler.empty(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    buffer,
                )
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                let (phase, order) = ready.into_parts();
                handler.endpoint_mismatch(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
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
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    buffer,
                    error,
                )
            }
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                let (phase, order) = ready.into_parts();
                handler.non_command(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                    frame,
                )
            }
        }
    }
}

impl<'runtime, Phase>
    BluetoothLegacyConnectableAdvertisingRecurringResponsePending<'runtime, Phase>
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
        controller.wait_response_capacity(&self.order).await
    }

    /// Attempt publication without consuming or pausing the recurrence phase.
    pub fn try_publish_response_with<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
        R,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
        published: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringCommandReady<'runtime, Phase>,
        ) -> R,
        pending: impl FnOnce(Self) -> R,
        endpoint_mismatch: impl FnOnce(Self) -> R,
        fault: impl FnOnce(Self, HciChannelError) -> R,
    ) -> R {
        match self
            .order
            .map_owner(|()| self.phase)
            .try_publish(controller)
        {
            LeControllerResponsePublication::Published(ordered) => {
                let (phase, order) = ordered.into_parts();
                published(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                let (phase, response) = transaction.into_parts();
                pending(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, response),
                )
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                let (phase, response) = transaction.into_parts();
                endpoint_mismatch(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, response),
                )
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => {
                let (phase, response) = transaction.into_parts();
                fault(
                    BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, response),
                    error,
                )
            }
        }
    }
}

impl<Phase> BluetoothLegacyConnectableAdvertisingRecurringStopping<'_, Phase> {
    pub const fn stop_kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn prepare_with<R>(
        self,
        ready: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.prepare_with(
            self.order,
            |order, phase| {
                ready(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn retry_timing_with<R>(
        self,
        ready: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                SelfPhase<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.retry_timing_with(
            self.order,
            |order, phase| {
                ready(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

type SelfPhase<'runtime, S, const CAPACITY: usize> =
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>;

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn begin_sequence_with<R>(
        self,
        waiting: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<
                    'runtime,
                    S,
                    CAPACITY,
                >,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.begin_sequence_with(
            self.order,
            |order, phase| {
                waiting(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn recheck_with<R>(
        self,
        waiting: impl FnOnce(Self) -> R,
        ready: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<'runtime, S, CAPACITY, Order>,
        ) -> R,
    ) -> R {
        self.phase.recheck_with(
            self.order,
            |order, phase| {
                waiting(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, phase| {
                ready(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn prepare_with<R>(
        self,
        ready: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
    ) -> R {
        self.phase.prepare_with(
            self.order,
            |order, phase| {
                ready(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize, Order>
    BluetoothLegacyConnectableAdvertisingRecurringHci<
        BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        Order,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
    Order: BluetoothLegacyConnectableAdvertisingRecurringForwardOrder,
{
    pub fn merge_with<R>(
        self,
        ready: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHci<
                BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                Order,
            >,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
                S::Error,
                Order,
            >,
        ) -> R,
    ) -> R {
        self.phase.merge_with(
            self.order,
            |order, phase| {
                ready(BluetoothLegacyConnectableAdvertisingRecurringHci::from_parts(phase, order))
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                LeControllerCommandReady<'runtime, ()>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
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
                running(
                    BluetoothLegacyConnectableAdvertisingHciActiveSession::from_ordered(
                        order.map_owner(|()| active),
                    ),
                )
            },
            |order, retry_owner| retry(wrap_retry!(order, retry_owner)),
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                LeControllerResponsePending<'runtime, ()>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
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
                running(
                    BluetoothLegacyConnectableAdvertisingActiveResponsePending::new(
                        response.map_owner(|()| active),
                    ),
                )
            },
            |response, retry_owner| retry(wrap_retry!(response, retry_owner)),
            |response, failure| fail_stop(wrap_fail_stop!(response, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn start_with<R>(
        self,
        running: impl FnOnce(BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>) -> R,
        retry: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
                S::Error,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.phase.start_with(
            self.order,
            |order, active| {
                running(BluetoothLegacyConnectableAdvertisingStopping::from_parts(
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
pub struct BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending:
        BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

fn finish_cancelled_with<'runtime, S, const CAPACITY: usize, R>(
    cancelled: BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
    disable: impl FnOnce(BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>) -> R,
    reset: impl FnOnce(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
) -> R
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let (task, _set, _phase, _scheduler_status, _rejected_packets) = cancelled.into_parts();
    match order {
        BluetoothLegacyConnectableAdvertisingStopOrder::Disable(deferred) => disable(
            BluetoothLegacyAdvertisingDisableResponsePending::from_cancelled(task, deferred),
        ),
        BluetoothLegacyConnectableAdvertisingStopOrder::Reset(barrier) => {
            reset(BluetoothLegacyAdvertisingResetCompletionReady::from_cancelled(task, barrier))
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<R>(
        self,
        disable: impl FnOnce(
            BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        reset: impl FnOnce(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
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
            BluetoothLegacyConnectableAdvertisingRecurringStopping<
                'runtime,
                $phase<'runtime, S, CAPACITY>,
            >
        where
            S: BluetoothSchedulerRunInterruptStorage,
        {
            pub fn cancel_with<R>(
                self,
                disable: impl FnOnce(
                    BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
                ) -> R,
                reset: impl FnOnce(
                    BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>,
                ) -> R,
                fail_stop: impl FnOnce(
                    BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                        'runtime,
                        S,
                        CAPACITY,
                        BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
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

impl_immediate_stop_cancellation!(BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared);
impl_immediate_stop_cancellation!(BluetoothLegacyConnectableAdvertisingRecurrenceCandidate);
impl_immediate_stop_cancellation!(BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady);
impl_immediate_stop_cancellation!(BluetoothLegacyConnectableAdvertisingRecurrencePrepared);
impl_immediate_stop_cancellation!(BluetoothLegacyConnectableAdvertisingRecurrenceMerged);

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringStopping<
        'runtime,
        BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<R>(
        self,
        draining: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending<
                'runtime,
                S,
                CAPACITY,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
            >,
        ) -> R,
    ) -> R {
        self.phase.cancel_with(
            self.order,
            |order, pending| {
                draining(
                    BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending {
                        pending,
                        order,
                    },
                )
            },
            |order, failure| fail_stop(wrap_fail_stop!(order, failure)),
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringHciCancellationPending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn recheck_with<R>(
        self,
        waiting: impl FnOnce(Self) -> R,
        disable: impl FnOnce(
            BluetoothLegacyAdvertisingDisableResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        reset: impl FnOnce(BluetoothLegacyAdvertisingResetCompletionReady<'runtime, S, CAPACITY>) -> R,
        fail_stop: impl FnOnce(
            BluetoothLegacyConnectableAdvertisingRecurringHciFailStop<
                'runtime,
                S,
                CAPACITY,
                BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
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
