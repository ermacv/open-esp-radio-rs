//! HCI ordering around one running legacy connectable-advertising event.
//!
//! The lower active owner remains executor-neutral. This module composes that
//! affine radio axis with the portable `bt-hci` command-order axis without
//! owning recurrence or peripheral-connection policy.

#![forbid(unsafe_code)]

use crate::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyConnectableAdvertisingActiveFailStop, LegacyConnectableAdvertisingActiveSession,
        LegacyConnectableAdvertisingActiveWait,
        LegacyConnectableAdvertisingAwaitingPeripheralStart,
        LegacyConnectableAdvertisingAwaitingRecurrence,
        LegacyConnectableAdvertisingRadioContinuations,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, HciEpochBound, HostToControllerFrame,
    LeControllerActiveLegacyAdvertisingCommandRoute as HciCommandRoute,
    LeControllerClassifiedCommand, LeControllerCommandEndpoint, LeControllerCommandIntake,
    LeControllerCommandReady, LeControllerDeferredLegacyAdvertisingDisable,
    LeControllerEndpointMismatch, LeControllerResetBarrier, LeControllerResponsePending,
    LeControllerResponsePublication,
};

type LowerActive<'runtime, S, const CAPACITY: usize> =
    LegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>;

/// Command-ready order paired with the independently progressing radio event.
#[must_use = "drive radio progress and retain the sole HCI command authority"]
pub struct LegacyConnectableAdvertisingHciActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<'runtime, LowerActive<'runtime, S, CAPACITY>>,
}

/// One bounded radio transition while the next HCI command may be accepted.
#[must_use = "retain the active owner, exact CPU boundary, unrelated list, or fail-stop owner"]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub enum LegacyConnectableAdvertisingHciActiveStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    Waiting(LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    NoConnection(LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>),
    ConnectionAccepted(LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>),
    FailStop(LegacyConnectableAdvertisingHciActiveFailStop<'runtime, S, CAPACITY>),
}

/// Reclaimed no-connection event retaining the next-command authority.
#[must_use = "retain this owner for recurrence or an ordered stop command"]
pub struct LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<
        'runtime,
        LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    >,
}

/// Accepted connection retaining the next-command authority and handoff owner.
#[must_use = "retain this owner until peripheral handoff and HCI ordering are composed"]
pub struct LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ordered: LeControllerCommandReady<
        'runtime,
        LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    >,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Preserve the completed item status across the HCI ordering boundary.
    pub const fn scheduler_status(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus
    {
        self.ordered.owner().scheduler_status()
    }

    /// Received PDUs rejected by connection-request admission.
    pub const fn rejected_packets(&self) -> usize {
        self.ordered.owner().rejected_packets()
    }

    /// Last rejected PDU header and the portable admission reason for this event.
    pub const fn last_receive_rejection(
        &self,
    ) -> Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )> {
        self.ordered.owner().last_receive_rejection()
    }

    pub(crate) fn into_ordered(
        self,
    ) -> LeControllerCommandReady<
        'runtime,
        LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    > {
        self.ordered
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Separate portable HCI order from the exact peripheral-handoff owner.
    pub fn into_parts(
        self,
    ) -> (
        LeControllerCommandReady<'runtime, ()>,
        LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    ) {
        let (accepted, order) = self.ordered.into_parts();
        (order, accepted)
    }
}

/// Sealed lower failure retaining the sole next-command authority.
#[must_use = "retain the failed radio owner and HCI order for diagnostic shutdown"]
pub struct LegacyConnectableAdvertisingHciActiveFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _ordered: LeControllerCommandReady<
        'runtime,
        LegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    >,
}

impl<S, const CAPACITY: usize> LegacyConnectableAdvertisingHciActiveFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(
        &self,
    ) -> crate::le::advertising::LegacyConnectableAdvertisingActiveFailStopCause {
        self._ordered.owner().cause()
    }

    pub fn receive_observations(
        &self,
    ) -> Option<[oer_esp32s31_bluetooth_memory::LeRxNodeObservation; 2]> {
        self._ordered.owner().receive_observations()
    }

    pub fn receive_error(&self) -> Option<oer_esp32s31_bluetooth_memory::LeRxError> {
        self._ordered.owner().receive_error()
    }
}

/// One response whose radio owner must keep progressing under backpressure.
#[must_use = "publish the response while continuing the exact radio event"]
pub struct LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<'runtime, LowerActive<'runtime, S, CAPACITY>>,
}

/// Response publication while the connectable event is still active.
#[must_use = "retain the pending response or returned command-ready owner"]
pub enum LegacyConnectableAdvertisingActiveResponsePublication<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>),
    Pending(LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Fault {
        pending: LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// No-connection boundary retaining the response which was pending at completion.
#[must_use = "publish the response while retaining the recurrence owner"]
pub struct LegacyConnectableAdvertisingNoConnectionResponsePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    >,
}

/// Connection boundary retaining the response which was pending at completion.
#[must_use = "publish the response while retaining the peripheral handoff owner"]
pub struct LegacyConnectableAdvertisingConnectionAcceptedResponsePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    transaction: LeControllerResponsePending<
        'runtime,
        LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    >,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        LeControllerResponsePending<'runtime, ()>,
        LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    ) {
        let (accepted, response) = self.transaction.into_parts();
        (response, accepted)
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Preserve the completed item status across the HCI ordering boundary.
    pub const fn scheduler_status(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus
    {
        self.transaction.owner().scheduler_status()
    }

    /// Received PDUs rejected by connection-request admission.
    pub const fn rejected_packets(&self) -> usize {
        self.transaction.owner().rejected_packets()
    }

    /// Last rejected PDU header and the portable admission reason for this event.
    pub const fn last_receive_rejection(
        &self,
    ) -> Option<(
        u8,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableConnectionRequestRejection,
    )> {
        self.transaction.owner().last_receive_rejection()
    }

    pub(crate) fn into_transaction(
        self,
    ) -> LeControllerResponsePending<
        'runtime,
        LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    > {
        self.transaction
    }
}

/// Pending response and sealed radio failure retained together.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct LegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _transaction: LeControllerResponsePending<
        'runtime,
        LegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    >,
}

/// Publication at a no-connection CPU boundary.
#[must_use = "retain the pending response or recurrence owner"]
pub enum LegacyConnectableAdvertisingNoConnectionResponsePublication<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyConnectableAdvertisingNoConnectionReady<'runtime, S, CAPACITY>),
    Pending(LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(
        LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ),
    Fault {
        pending: LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Publication at an accepted-connection CPU boundary.
#[must_use = "retain the pending response or peripheral handoff owner"]
pub enum LegacyConnectableAdvertisingConnectionAcceptedResponsePublication<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>),
    Pending(LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>),
    EndpointMismatch(
        LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
    ),
    Fault {
        pending:
            LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
        error: HciChannelError,
    },
}

/// Exact active-advertising stop order retained across hardware completion.
#[must_use = "complete this order only after its radio owner is CPU-owned"]
pub enum LegacyConnectableAdvertisingStopOrder<'runtime> {
    Disable(LeControllerDeferredLegacyAdvertisingDisable<'runtime, ()>),
    Reset(LeControllerResetBarrier<'runtime, ()>),
}

/// Semantic kind of a retained connectable-advertising stop order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingStopKind {
    Disable,
    Reset,
}

impl LegacyConnectableAdvertisingStopOrder<'_> {
    pub const fn kind(&self) -> LegacyConnectableAdvertisingStopKind {
        match self {
            Self::Disable(_) => LegacyConnectableAdvertisingStopKind::Disable,
            Self::Reset(_) => LegacyConnectableAdvertisingStopKind::Reset,
        }
    }
}

/// Active event carrying an accepted Disable or Reset to its first CPU boundary.
#[must_use = "continue radio progress before completing the retained stop order"]
pub struct LegacyConnectableAdvertisingStopping<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    active: LowerActive<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
}

/// One bounded stop transition.
#[must_use = "retain the stop order through radio completion"]
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub enum LegacyConnectableAdvertisingStoppingStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    Waiting(LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    UnrelatedList {
        stopping: LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    NoConnection(LegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>),
    ConnectionAccepted(
        LegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>,
    ),
    FailStop(LegacyConnectableAdvertisingStoppingFailStop<'runtime, S, CAPACITY>),
}

/// No-connection CPU boundary retaining the undispatched Disable or Reset.
#[must_use = "complete recurrence/stop policy without losing either owner"]
pub struct LegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    completed: LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
}

/// Accepted-connection CPU boundary retaining the undispatched Disable or Reset.
#[must_use = "resolve peripheral handoff and stop order without losing either owner"]
pub struct LegacyConnectableAdvertisingConnectionAcceptedStopping<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    accepted: LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingNoConnectionStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> LegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
        LegacyConnectableAdvertisingStopOrder<'runtime>,
    ) {
        (self.completed, self.order)
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn stop_kind(&self) -> LegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }

    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
        LegacyConnectableAdvertisingStopOrder<'runtime>,
    ) {
        (self.accepted, self.order)
    }
}

/// Sealed radio failure retaining the undispatched Disable or Reset.
#[must_use = "retain the exact failed stop transaction for diagnostic shutdown"]
pub struct LegacyConnectableAdvertisingStoppingFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    fault: LegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
    order: LegacyConnectableAdvertisingStopOrder<'runtime>,
}

impl<S, const CAPACITY: usize> LegacyConnectableAdvertisingStoppingFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(
        &self,
    ) -> crate::le::advertising::LegacyConnectableAdvertisingActiveFailStopCause {
        self.fault.cause()
    }

    pub const fn stop_kind(&self) -> LegacyConnectableAdvertisingStopKind {
        self.order.kind()
    }
}

/// Opaque endpoint mismatch retaining the complete classified command.
#[must_use = "retain the command, radio owner and HCI order"]
pub struct LegacyConnectableAdvertisingCommandMismatch<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<'runtime, 'command, LowerActive<'runtime, S, CAPACITY>>,
}

/// Routed command while one connectable advertising event remains active.
#[must_use = "publish, stop, or retain the endpoint mismatch owner"]
pub enum LegacyConnectableAdvertisingCommandRoute<'runtime, 'command, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ResponsePending(LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>),
    EndpointMismatch(LegacyConnectableAdvertisingCommandMismatch<'runtime, 'command, S, CAPACITY>),
}

/// One non-blocking command intake while the radio event remains in flight.
#[must_use = "route one command or retain the unchanged active owner"]
pub enum LegacyConnectableAdvertisingCommandIntake<
    'runtime,
    'command,
    'buffer,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Routed {
        route: LegacyConnectableAdvertisingCommandRoute<'runtime, 'command, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Empty {
        active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    EndpointMismatch {
        active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
    },
    Channel {
        active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        buffer: &'buffer mut [u8],
        error: HciChannelError,
    },
    NonCommand {
        active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
        frame: HciEpochBound<'command, HostToControllerFrame<'buffer>>,
    },
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn from_ordered(
        ordered: LeControllerCommandReady<'runtime, LowerActive<'runtime, S, CAPACITY>>,
    ) -> Self {
        Self { ordered }
    }

    pub fn radio_wait(&self) -> Option<LegacyConnectableAdvertisingActiveWait<'_>> {
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

    pub fn step_radio(self) -> LegacyConnectableAdvertisingHciActiveStep<'runtime, S, CAPACITY> {
        let (active, order) = self.ordered.into_parts();
        active.step_radio_with(
            order,
            LegacyConnectableAdvertisingRadioContinuations::new(
                |order: LeControllerCommandReady<'runtime, ()>, active| {
                    LegacyConnectableAdvertisingHciActiveStep::Continue(Self::from_ordered(
                        order.map_owner(|()| active),
                    ))
                },
                |order: LeControllerCommandReady<'runtime, ()>, active| {
                    LegacyConnectableAdvertisingHciActiveStep::Waiting(Self::from_ordered(
                        order.map_owner(|()| active),
                    ))
                },
                |order: LeControllerCommandReady<'runtime, ()>, active, observed| {
                    LegacyConnectableAdvertisingHciActiveStep::UnrelatedList {
                        session: Self::from_ordered(order.map_owner(|()| active)),
                        observed,
                    }
                },
                |order: LeControllerCommandReady<'runtime, ()>, completed| {
                    LegacyConnectableAdvertisingHciActiveStep::NoConnection(
                        LegacyConnectableAdvertisingNoConnectionReady {
                            ordered: order.map_owner(|()| completed),
                        },
                    )
                },
                |order: LeControllerCommandReady<'runtime, ()>, accepted| {
                    LegacyConnectableAdvertisingHciActiveStep::ConnectionAccepted(
                        LegacyConnectableAdvertisingConnectionAcceptedReady {
                            ordered: order.map_owner(|()| accepted),
                        },
                    )
                },
                |order: LeControllerCommandReady<'runtime, ()>, fault| {
                    LegacyConnectableAdvertisingHciActiveStep::FailStop(
                        LegacyConnectableAdvertisingHciActiveFailStop {
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
    ) -> LegacyConnectableAdvertisingCommandIntake<'runtime, 'command, 'buffer, S, CAPACITY> {
        match controller.try_receive_classified_command_with_buffer(self.ordered, buffer) {
            LeControllerCommandIntake::Command { command, buffer } => {
                let route =
                    match controller.route_active_legacy_advertising_classified_command(command) {
                        HciCommandRoute::ResponsePending(transaction) => {
                            LegacyConnectableAdvertisingCommandRoute::ResponsePending(
                                LegacyConnectableAdvertisingActiveResponsePending { transaction },
                            )
                        }
                        HciCommandRoute::Disable(disable) => {
                            let (active, disable) = disable.into_parts();
                            LegacyConnectableAdvertisingCommandRoute::Stopping(
                                LegacyConnectableAdvertisingStopping {
                                    active,
                                    order: LegacyConnectableAdvertisingStopOrder::Disable(disable),
                                },
                            )
                        }
                        HciCommandRoute::ResetBarrier(barrier) => {
                            let (active, barrier) = barrier.into_parts();
                            LegacyConnectableAdvertisingCommandRoute::Stopping(
                                LegacyConnectableAdvertisingStopping {
                                    active,
                                    order: LegacyConnectableAdvertisingStopOrder::Reset(barrier),
                                },
                            )
                        }
                        HciCommandRoute::EndpointMismatch(command) => {
                            LegacyConnectableAdvertisingCommandRoute::EndpointMismatch(
                                LegacyConnectableAdvertisingCommandMismatch { _command: command },
                            )
                        }
                    };
                LegacyConnectableAdvertisingCommandIntake::Routed { route, buffer }
            }
            LeControllerCommandIntake::Empty { ready, buffer } => {
                LegacyConnectableAdvertisingCommandIntake::Empty {
                    active: Self::from_ordered(ready),
                    buffer,
                }
            }
            LeControllerCommandIntake::EndpointMismatch { ready, buffer } => {
                LegacyConnectableAdvertisingCommandIntake::EndpointMismatch {
                    active: Self::from_ordered(ready),
                    buffer,
                }
            }
            LeControllerCommandIntake::Channel {
                ready,
                buffer,
                error,
            } => LegacyConnectableAdvertisingCommandIntake::Channel {
                active: Self::from_ordered(ready),
                buffer,
                error,
            },
            LeControllerCommandIntake::NonCommand { ready, frame } => {
                LegacyConnectableAdvertisingCommandIntake::NonCommand {
                    active: Self::from_ordered(ready),
                    frame,
                }
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        transaction: LeControllerResponsePending<'runtime, LowerActive<'runtime, S, CAPACITY>>,
    ) -> Self {
        Self { transaction }
    }

    pub fn radio_wait(&self) -> Option<LegacyConnectableAdvertisingActiveWait<'_>> {
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
    ) -> LegacyConnectableAdvertisingActiveResponsePublication<'runtime, S, CAPACITY> {
        match self.transaction.try_publish(controller) {
            LeControllerResponsePublication::Published(ordered) => {
                LegacyConnectableAdvertisingActiveResponsePublication::Published(
                    LegacyConnectableAdvertisingHciActiveSession::from_ordered(ordered),
                )
            }
            LeControllerResponsePublication::Pending(transaction) => {
                LegacyConnectableAdvertisingActiveResponsePublication::Pending(Self { transaction })
            }
            LeControllerResponsePublication::EndpointMismatch(transaction) => {
                LegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(Self {
                    transaction,
                })
            }
            LeControllerResponsePublication::Fault {
                pending: transaction,
                error,
            } => LegacyConnectableAdvertisingActiveResponsePublication::Fault {
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
        continuations: LegacyConnectableAdvertisingRadioContinuations<
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
            LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
        ) -> R,
        FailStop: FnOnce(
            Context,
            LegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    {
        let (continuing, waiting, unrelated, no_connection, connection_accepted, fail_stop) =
            continuations.into_parts();
        let (active, response) = self.transaction.into_parts();
        active.step_radio_with(
            (context, response),
            LegacyConnectableAdvertisingRadioContinuations::new(
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
                        LegacyConnectableAdvertisingNoConnectionResponsePending {
                            transaction: response.map_owner(|()| completed),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 accepted| {
                    connection_accepted(
                        context,
                        LegacyConnectableAdvertisingConnectionAcceptedResponsePending {
                            transaction: response.map_owner(|()| accepted),
                        },
                    )
                },
                |(context, response): (Context, LeControllerResponsePending<'runtime, ()>),
                 fault| {
                    fail_stop(
                        context,
                        LegacyConnectableAdvertisingActivePendingFailStop {
                            _transaction: response.map_owner(|()| fault),
                        },
                    )
                },
            ),
        )
    }
}

impl<S, const CAPACITY: usize> LegacyConnectableAdvertisingActivePendingFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(
        &self,
    ) -> crate::le::advertising::LegacyConnectableAdvertisingActiveFailStopCause {
        self._transaction.owner().cause()
    }
}

macro_rules! impl_boundary_response {
    ($pending:ident, $publication:ident, $ready:ident) => {
        impl<'runtime, S, const CAPACITY: usize> $pending<'runtime, S, CAPACITY>
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
    LegacyConnectableAdvertisingNoConnectionResponsePending,
    LegacyConnectableAdvertisingNoConnectionResponsePublication,
    LegacyConnectableAdvertisingNoConnectionReady
);
impl_boundary_response!(
    LegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    LegacyConnectableAdvertisingConnectionAcceptedResponsePublication,
    LegacyConnectableAdvertisingConnectionAcceptedReady
);

impl<'runtime, S, const CAPACITY: usize> LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn from_parts(
        active: LowerActive<'runtime, S, CAPACITY>,
        order: LegacyConnectableAdvertisingStopOrder<'runtime>,
    ) -> Self {
        Self { active, order }
    }

    pub fn radio_wait(&self) -> Option<LegacyConnectableAdvertisingActiveWait<'_>> {
        self.active.radio_wait()
    }

    pub fn step(self) -> LegacyConnectableAdvertisingStoppingStep<'runtime, S, CAPACITY> {
        let Self { active, order } = self;
        active.step_radio_with(
            order,
            LegacyConnectableAdvertisingRadioContinuations::new(
                |order, active| {
                    LegacyConnectableAdvertisingStoppingStep::Continue(Self { active, order })
                },
                |order, active| {
                    LegacyConnectableAdvertisingStoppingStep::Waiting(Self { active, order })
                },
                |order, active, observed| LegacyConnectableAdvertisingStoppingStep::UnrelatedList {
                    stopping: Self { active, order },
                    observed,
                },
                |order, completed| {
                    LegacyConnectableAdvertisingStoppingStep::NoConnection(
                        LegacyConnectableAdvertisingNoConnectionStopping { completed, order },
                    )
                },
                |order, accepted| {
                    LegacyConnectableAdvertisingStoppingStep::ConnectionAccepted(
                        LegacyConnectableAdvertisingConnectionAcceptedStopping { accepted, order },
                    )
                },
                |order, fault| {
                    LegacyConnectableAdvertisingStoppingStep::FailStop(
                        LegacyConnectableAdvertisingStoppingFailStop { fault, order },
                    )
                },
            ),
        )
    }
}
