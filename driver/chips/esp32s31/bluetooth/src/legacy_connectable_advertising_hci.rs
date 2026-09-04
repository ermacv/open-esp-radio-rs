//! HCI ordering around one running legacy connectable-advertising event.
//!
//! The lower active owner remains executor-neutral. This module composes that
//! affine radio axis with the portable `bt-hci` command-order axis without
//! owning recurrence or peripheral-connection policy.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerDeferredLegacyAdvertisingDisable,
    LeControllerEndpointMismatch, LeControllerResetBarrier, LeControllerResponsePending,
    LeControllerResponsePublication,
};

use crate::{
    BluetoothLegacyConnectableAdvertisingActiveFailStop,
    BluetoothLegacyConnectableAdvertisingActiveSession,
    BluetoothLegacyConnectableAdvertisingActiveWait,
    BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart,
    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence,
    BluetoothLegacyConnectableAdvertisingRadioContinuations,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
};

type LowerActive<'runtime, S, const CAPACITY: usize> =
    BluetoothLegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>;

/// Command-ready order paired with the independently progressing radio event.
#[must_use = "drive radio progress and retain the sole HCI command authority"]
pub struct BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<'runtime, LowerActive<'runtime, S, CAPACITY>>,
}

/// One bounded radio transition while the next HCI command may be accepted.
#[must_use = "retain the active owner, exact CPU boundary, unrelated list, or fail-stop owner"]
pub enum BluetoothLegacyConnectableAdvertisingHciActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    Waiting(BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    NoConnection(BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>),
    ConnectionAccepted(
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>,
    ),
    FailStop(BluetoothLegacyConnectableAdvertisingHciActiveFailStop<'runtime, S, CAPACITY>),
}

/// Reclaimed no-connection event retaining the next-command authority.
#[must_use = "retain this owner for recurrence or an ordered stop command"]
pub struct BluetoothLegacyConnectableAdvertisingNoConnectionReady<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    >,
}

/// Accepted connection retaining the next-command authority and handoff owner.
#[must_use = "retain this owner until peripheral handoff and HCI ordering are composed"]
pub struct BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    >,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_ordered(
        self,
    ) -> LeControllerCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    > {
        self.ordered
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Separate portable HCI order from the exact peripheral-handoff owner.
    pub fn into_parts(
        self,
    ) -> (
        LeControllerCommandReady<'runtime, ()>,
        BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    ) {
        let (accepted, order) = self.ordered.into_parts();
        (order, accepted)
    }
}

/// Sealed lower failure retaining the sole next-command authority.
#[must_use = "retain the failed radio owner and HCI order for diagnostic shutdown"]
pub struct BluetoothLegacyConnectableAdvertisingHciActiveFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _ordered: LeControllerCommandReady<
        'runtime,
        BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    >,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingHciActiveFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> crate::BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
        self._ordered.owner().cause()
    }
}

/// One response whose radio owner must keep progressing under backpressure.
#[must_use = "publish the response while continuing the exact radio event"]
pub struct BluetoothLegacyConnectableAdvertisingActiveResponsePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, LowerActive<'runtime, S, CAPACITY>>,
}

/// Response publication while the connectable event is still active.
#[must_use = "retain the pending response or returned command-ready owner"]
pub enum BluetoothLegacyConnectableAdvertisingActiveResponsePublication<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    Pending(BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ),
    Fault {
        pending: BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// No-connection boundary retaining the response which was pending at completion.
#[must_use = "publish the response while retaining the recurrence owner"]
pub struct BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    >,
}

/// Connection boundary retaining the response which was pending at completion.
#[must_use = "publish the response while retaining the peripheral handoff owner"]
pub struct BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    >,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        LeControllerResponsePending<'runtime, ()>,
        BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    ) {
        let (accepted, response) = self.transaction.into_parts();
        (response, accepted)
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_transaction(
        self,
    ) -> LeControllerResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    > {
        self.transaction
    }
}

/// Pending response and sealed radio failure retained together.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct BluetoothLegacyConnectableAdvertisingActivePendingFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _transaction: LeControllerResponsePending<
        'runtime,
        BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    >,
}

/// Publication at a no-connection CPU boundary.
#[must_use = "retain the pending response or recurrence owner"]
pub enum BluetoothLegacyConnectableAdvertisingNoConnectionResponsePublication<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>),
    Pending(
        BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ),
    EndpointMismatch(
        BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ),
    Fault {
        pending:
            BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Publication at an accepted-connection CPU boundary.
#[must_use = "retain the pending response or peripheral handoff owner"]
pub enum BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePublication<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>),
    Pending(
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ),
    EndpointMismatch(
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ),
    Fault {
        pending: BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
        error: HciChannelError,
    },
}

/// Exact active-advertising stop order retained across hardware completion.
#[must_use = "complete this order only after its radio owner is CPU-owned"]
pub enum BluetoothLegacyConnectableAdvertisingStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Semantic kind of a retained connectable-advertising stop order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingStopKind {
    Disable,
    Reset,
}

impl BluetoothLegacyConnectableAdvertisingStopOrder<'_> {
    pub const fn kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        match self {
            Self::Disable(_) => BluetoothLegacyConnectableAdvertisingStopKind::Disable,
            Self::Reset(_) => BluetoothLegacyConnectableAdvertisingStopKind::Reset,
        }
    }
}

/// Active event carrying an accepted Disable or Reset to its first CPU boundary.
#[must_use = "continue radio progress before completing the retained stop order"]
pub struct BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    active: LowerActive<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
}

/// One bounded stop transition.
#[must_use = "retain the stop order through radio completion"]
pub enum BluetoothLegacyConnectableAdvertisingStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    Waiting(BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    NoConnection(BluetoothLegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>),
    ConnectionAccepted(
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>,
    ),
    FailStop(BluetoothLegacyConnectableAdvertisingStoppingFailStop<'runtime, S, CAPACITY>),
}

/// No-connection CPU boundary retaining the undispatched Disable or Reset.
#[must_use = "complete recurrence/stop policy without losing either owner"]
pub struct BluetoothLegacyConnectableAdvertisingNoConnectionStopping<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    completed: BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
}

/// Accepted-connection CPU boundary retaining the undispatched Disable or Reset.
#[must_use = "resolve peripheral handoff and stop order without losing either owner"]
pub struct BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    accepted: BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
    ) {
        (self.completed, self.order)
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
    ) {
        (self.accepted, self.order)
    }
}

/// Sealed radio failure retaining the undispatched Disable or Reset.
#[must_use = "retain the exact failed stop transaction for diagnostic shutdown"]
pub struct BluetoothLegacyConnectableAdvertisingStoppingFailStop<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fault: BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingStoppingFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> crate::BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
        self.fault.cause()
    }

    pub const fn stop_kind(&self) -> BluetoothLegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

/// Opaque endpoint mismatch retaining the complete classified command.
#[must_use = "retain the command, radio owner and HCI order"]
pub struct BluetoothLegacyConnectableAdvertisingCommandMismatch<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<'runtime, 'command, LowerActive<'runtime, S, CAPACITY>>,
}

/// Routed command while one connectable advertising event remains active.
#[must_use = "publish, stop, or retain the endpoint mismatch owner"]
pub enum BluetoothLegacyConnectableAdvertisingCommandRoute<
    'runtime,
    'command,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ResponsePending(
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ),
    Stopping(BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyConnectableAdvertisingCommandMismatch<'runtime, 'command, S, CAPACITY>,
    ),
}

/// One non-blocking command intake while the radio event remains in flight.
#[must_use = "route one command or retain the unchanged active owner"]
pub enum BluetoothLegacyConnectableAdvertisingCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Routed {
        route: BluetoothLegacyConnectableAdvertisingCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn from_ordered(
        ordered: LeControllerCommandReady<'runtime, LowerActive<'runtime, S, CAPACITY>>,
    ) -> Self {
        Self { ordered }
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyConnectableAdvertisingActiveWait<'_>> {
        self.ordered.owner().radio_wait()
    }

    pub fn accepts_hci_endpoint<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> bool {
        self.ordered.accepts_endpoint(controller)
    }

    pub async fn wait_command_available<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_command_available(&self.ordered).await
    }

    pub fn step_radio(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingHciActiveStep<'runtime, S, CAPACITY> {
        let (active, order) = self.ordered.into_parts();
        active.step_radio_with(
            order,
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
                |order: LeControllerCommandReady<'runtime, ()>, active| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::Continue(
                        Self::from_ordered(order.map_owner(|()| active)),
                    )
                },
                |order: LeControllerCommandReady<'runtime, ()>, active| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::Waiting(Self::from_ordered(
                        order.map_owner(|()| active),
                    ))
                },
                |order: LeControllerCommandReady<'runtime, ()>, active, observed| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::UnrelatedList {
                        session: Self::from_ordered(order.map_owner(|()| active)),
                        observed,
                    }
                },
                |order: LeControllerCommandReady<'runtime, ()>, completed| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::NoConnection(
                        BluetoothLegacyConnectableAdvertisingNoConnectionReady {
                            ordered: order.map_owner(|()| completed),
                        },
                    )
                },
                |order: LeControllerCommandReady<'runtime, ()>, accepted| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::ConnectionAccepted(
                        BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady {
                            ordered: order.map_owner(|()| accepted),
                        },
                    )
                },
                |order: LeControllerCommandReady<'runtime, ()>, fault| {
                    BluetoothLegacyConnectableAdvertisingHciActiveStep::FailStop(
                        BluetoothLegacyConnectableAdvertisingHciActiveFailStop {
                            _ordered: order.map_owner(|()| fault),
                        },
                    )
                },
            ),
        )
    }

    pub fn try_route_controller_command_with_buffer<
        'command,
        'buffer,
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &mut LeControllerCommandEndpoint<'command, M, H2C, C2H, PACKET>,
        buffer: &'buffer mut [u8],
    ) -> BluetoothLegacyConnectableAdvertisingCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY>
    {
        match controller.try_receive_classified_command_with_buffer(self.ordered, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_advertising_classified_command(command) {
                        HciCommandRoute::ResponsePending(transaction) => {
                            BluetoothLegacyConnectableAdvertisingCommandRoute::ResponsePending(
                                BluetoothLegacyConnectableAdvertisingActiveResponsePending {
                                    transaction,
                                },
                            )
                        }
                        HciCommandRoute::Disable(disable) => {
                            let (active, disable) = disable.into_parts();
                            BluetoothLegacyConnectableAdvertisingCommandRoute::Stopping(
                                BluetoothLegacyConnectableAdvertisingStopping {
                                    active,
                                    order: BluetoothLegacyConnectableAdvertisingStopOrder::Disable(
                                        disable,
                                    ),
                                },
                            )
                        }
                        HciCommandRoute::ResetBarrier(barrier) => {
                            let (active, barrier) = barrier.into_parts();
                            BluetoothLegacyConnectableAdvertisingCommandRoute::Stopping(
                                BluetoothLegacyConnectableAdvertisingStopping {
                                    active,
                                    order: BluetoothLegacyConnectableAdvertisingStopOrder::Reset(
                                        barrier,
                                    ),
                                },
                            )
                        }
                        HciCommandRoute::EndpointMismatch(command) => {
                            BluetoothLegacyConnectableAdvertisingCommandRoute::EndpointMismatch(
                                BluetoothLegacyConnectableAdvertisingCommandMismatch {
                                    _command: command,
                                },
                            )
                        }
                    };
                BluetoothLegacyConnectableAdvertisingCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                BluetoothLegacyConnectableAdvertisingCommandIntake::Empty {
                    active: Self::from_ordered(ready),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                BluetoothLegacyConnectableAdvertisingCommandIntake::EndpointMismatch {
                    active: Self::from_ordered(ready),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => BluetoothLegacyConnectableAdvertisingCommandIntake::Channel {
                active: Self::from_ordered(ready),
                buffer,
                error,
            },
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                BluetoothLegacyConnectableAdvertisingCommandIntake::NonCommand {
                    active: Self::from_ordered(ready),
                    frame,
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        transaction: LeControllerResponsePending<'runtime, LowerActive<'runtime, S, CAPACITY>>,
    ) -> Self {
        Self { transaction }
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyConnectableAdvertisingActiveWait<'_>> {
        self.transaction.owner().radio_wait()
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        controller.wait_response_capacity(&self.transaction).await
    }

    pub fn try_publish<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothLegacyConnectableAdvertisingActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ordered) => {
                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Published(
                    BluetoothLegacyConnectableAdvertisingHciActiveSession::from_ordered(ordered),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Pending(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                BluetoothLegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(
                    Self { transaction },
                )
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Fault {
                pending: Self { transaction },
                error,
            },
        }
    }

    pub fn step_radio_with<
        R,
        Context,
        Continuing,
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >(
        self,
        context: Context,
        continuations: BluetoothLegacyConnectableAdvertisingRadioContinuations<
            Continuing,
            Waiting,
            Unrelated,
            NoConnection,
            ConnectionAccepted,
            FailStop,
        >,
    ) -> R
    where
        Continuing: FnOnce(Context, Self) -> R,
        Waiting: FnOnce(Context, Self) -> R,
        Unrelated: FnOnce(Context, Self, BluetoothSchedulerFinishedHardwareListObserved) -> R,
        NoConnection: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
                'runtime,
                S,
                CAPACITY,
            >,
        ) -> R,
        FailStop: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    {
        let (continuing, waiting, unrelated, no_connection, connection_accepted, fail_stop) =
            continuations.into_parts();
        let (active, response) = self.transaction.into_parts();
        active.step_radio_with(
            (context, response),
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 active| {
                    continuing(
                        context,
                        Self {
                            transaction: response.map_owner(|()| active),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 active| {
                    waiting(
                        context,
                        Self {
                            transaction: response.map_owner(|()| active),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 active,
                 observed| {
                    unrelated(
                        context,
                        Self {
                            transaction: response.map_owner(|()| active),
                        },
                        observed,
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 completed| {
                    no_connection(
                        context,
                        BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending {
                            transaction: response.map_owner(|()| completed),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 accepted| {
                    connection_accepted(
                        context,
                        BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending {
                            transaction: response.map_owner(|()| accepted),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 fault| {
                    fail_stop(
                        context,
                        BluetoothLegacyConnectableAdvertisingActivePendingFailStop {
                            _transaction: response.map_owner(|()| fault),
                        },
                    )
                },
            ),
        )
    }
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingActivePendingFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> crate::BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
        self._transaction.owner().cause()
    }
}

macro_rules! impl_boundary_response {
    ($pending:ident, $publication:ident, $ready:ident) => {
        impl<'runtime, S, const CAPACITY: usize> $pending<'runtime, S, CAPACITY>
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
                controller.wait_response_capacity(&self.transaction).await
            }

            pub fn try_publish<
                M: RawMutex,
                const H2C: usize,
                const C2H: usize,
                const PACKET: usize,
            >(
                self,
                controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
            ) -> $publication<'runtime, S, CAPACITY> {
                match self.transaction.try_publish(controller) {
                    LeControllerResponsePublication::Published(ordered) => {
                        $publication::Published($ready { ordered })
                    }
                    LeControllerResponsePublication::Pending(transaction) => {
                        $publication::Pending(Self { transaction })
                    }
                    LeControllerResponsePublication::EndpointMismatch(transaction) => {
                        $publication::EndpointMismatch(Self { transaction })
                    }
                    LeControllerResponsePublication::Fault {
                        pending: transaction,
                        error,
                    } => $publication::Fault {
                        pending: Self { transaction },
                        error,
                    },
                }
            }
        }
    };
}

impl_boundary_response!(
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePublication,
    BluetoothLegacyConnectableAdvertisingNoConnectionReady
);
impl_boundary_response!(
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePublication,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady
);

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn from_parts(
        active: LowerActive<'runtime, S, CAPACITY>,
        order: BluetoothLegacyConnectableAdvertisingStopOrder<'runtime>,
    ) -> Self {
        Self { active, order }
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyConnectableAdvertisingActiveWait<'_>> {
        self.active.radio_wait()
    }

    pub fn step(self) -> BluetoothLegacyConnectableAdvertisingStoppingStep<'runtime, S, CAPACITY> {
        let Self { active, order } = self;
        active.step_radio_with(
            order,
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
                |order, active| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::Continue(Self {
                        active,
                        order,
                    })
                },
                |order, active| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::Waiting(Self {
                        active,
                        order,
                    })
                },
                |order, active, observed| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::UnrelatedList {
                        stopping: Self { active, order },
                        observed,
                    }
                },
                |order, completed| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::NoConnection(
                        BluetoothLegacyConnectableAdvertisingNoConnectionStopping {
                            completed,
                            order,
                        },
                    )
                },
                |order, accepted| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::ConnectionAccepted(
                        BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping {
                            accepted,
                            order,
                        },
                    )
                },
                |order, fault| {
                    BluetoothLegacyConnectableAdvertisingStoppingStep::FailStop(
                        BluetoothLegacyConnectableAdvertisingStoppingFailStop { fault, order },
                    )
                },
            ),
        )
    }
}
