//! Finite Embassy-side drive for legacy connectable advertising HCI/radio axes.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyConnectableAdvertisingActivePendingFailStop,
        LegacyConnectableAdvertisingActiveResponsePending,
        LegacyConnectableAdvertisingConnectionAcceptedResponsePending,
        LegacyConnectableAdvertisingHciActiveSession, LegacyConnectableAdvertisingHciActiveStep,
        LegacyConnectableAdvertisingNoConnectionResponsePending,
        LegacyConnectableAdvertisingRadioContinuations,
        LegacyConnectableAdvertisingResponsePending, LegacyConnectableAdvertisingStopping,
        LegacyConnectableAdvertisingStoppingStep,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

/// Five terminal continuations for an Embassy bounded ready drive.
///
/// Immediate lower `Continue` transitions remain internal to the drive loop;
/// exactly one of these callbacks receives the caller's affine context and
/// terminal owner.
pub struct LegacyConnectableAdvertisingReadyContinuations<
    Waiting,
    Unrelated,
    NoConnection,
    ConnectionAccepted,
    FailStop,
> {
    waiting: Waiting,
    unrelated: Unrelated,
    no_connection: NoConnection,
    connection_accepted: ConnectionAccepted,
    fail_stop: FailStop,
}

impl<Waiting, Unrelated, NoConnection, ConnectionAccepted, FailStop>
    LegacyConnectableAdvertisingReadyContinuations<
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >
{
    pub const fn new(
        waiting: Waiting,
        unrelated: Unrelated,
        no_connection: NoConnection,
        connection_accepted: ConnectionAccepted,
        fail_stop: FailStop,
    ) -> Self {
        Self {
            waiting,
            unrelated,
            no_connection,
            connection_accepted,
            fail_stop,
        }
    }

    fn into_parts(
        self,
    ) -> (
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    ) {
        (
            self.waiting,
            self.unrelated,
            self.no_connection,
            self.connection_accepted,
            self.fail_stop,
        )
    }
}

/// Collapse immediately ready lower edges without adding another ownership envelope.
pub fn drive_legacy_connectable_advertising_initial_pending_ready_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Context,
    R,
    Waiting,
    Unrelated,
    NoConnection,
    ConnectionAccepted,
    FailStop,
>(
    mut pending: LegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
    mut context: Context,
    continuations: LegacyConnectableAdvertisingReadyContinuations<
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >,
) -> R
where
    S: SchedulerRunInterruptStorage,
    Waiting:
        FnMut(Context, LegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>) -> R,
    Unrelated: FnMut(
        Context,
        LegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
        BluetoothSchedulerFinishedHardwareListObserved,
    ) -> R,
    NoConnection: FnMut(
        Context,
        LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    ConnectionAccepted: FnMut(
        Context,
        LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    FailStop: FnMut(
        Context,
        LegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ) -> R,
{
    let (mut waiting, mut unrelated, mut no_connection, mut connection_accepted, mut fail_stop) =
        continuations.into_parts();
    loop {
        match pending.step_radio_with(
            context,
            LegacyConnectableAdvertisingRadioContinuations::new(
                |context, pending| ControlFlow::Continue((context, pending)),
                |context, pending| ControlFlow::Break(waiting(context, pending)),
                |context, pending, observed| {
                    ControlFlow::Break(unrelated(context, pending, observed))
                },
                |context, completed| ControlFlow::Break(no_connection(context, completed)),
                |context, accepted| ControlFlow::Break(connection_accepted(context, accepted)),
                |context, fault| ControlFlow::Break(fail_stop(context, fault)),
            ),
        ) {
            ControlFlow::Continue((next_context, next)) => {
                context = next_context;
                pending = next;
            }
            ControlFlow::Break(result) => return result,
        }
    }
}

/// Collapse immediately ready command-ready radio edges.
pub fn drive_legacy_connectable_advertising_active_ready<'runtime, S, const CAPACITY: usize>(
    mut active: LegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
) -> LegacyConnectableAdvertisingHciActiveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match active.step_radio() {
            LegacyConnectableAdvertisingHciActiveStep::Continue(next) => active = next,
            step => return step,
        }
    }
}

/// Collapse immediately ready radio edges while a response remains pending.
pub fn drive_legacy_connectable_advertising_pending_ready_with<
    'runtime,
    S,
    const CAPACITY: usize,
    Context,
    R,
    Waiting,
    Unrelated,
    NoConnection,
    ConnectionAccepted,
    FailStop,
>(
    mut pending: LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    mut context: Context,
    continuations: LegacyConnectableAdvertisingReadyContinuations<
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >,
) -> R
where
    S: SchedulerRunInterruptStorage,
    Waiting: FnMut(
        Context,
        LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    Unrelated: FnMut(
        Context,
        LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        BluetoothSchedulerFinishedHardwareListObserved,
    ) -> R,
    NoConnection: FnMut(
        Context,
        LegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    ConnectionAccepted: FnMut(
        Context,
        LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    FailStop: FnMut(
        Context,
        LegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ) -> R,
{
    let (mut waiting, mut unrelated, mut no_connection, mut connection_accepted, mut fail_stop) =
        continuations.into_parts();
    loop {
        match pending.step_radio_with(
            context,
            LegacyConnectableAdvertisingRadioContinuations::new(
                |context, pending| ControlFlow::Continue((context, pending)),
                |context, pending| ControlFlow::Break(waiting(context, pending)),
                |context, pending, observed| {
                    ControlFlow::Break(unrelated(context, pending, observed))
                },
                |context, completed| ControlFlow::Break(no_connection(context, completed)),
                |context, accepted| ControlFlow::Break(connection_accepted(context, accepted)),
                |context, fault| ControlFlow::Break(fail_stop(context, fault)),
            ),
        ) {
            ControlFlow::Continue((next_context, next)) => {
                context = next_context;
                pending = next;
            }
            ControlFlow::Break(result) => return result,
        }
    }
}

/// Collapse immediately ready stop edges while retaining Disable/Reset order.
pub fn drive_legacy_connectable_advertising_stopping_ready<'runtime, S, const CAPACITY: usize>(
    mut stopping: LegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
) -> LegacyConnectableAdvertisingStoppingStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match stopping.step() {
            LegacyConnectableAdvertisingStoppingStep::Continue(next) => stopping = next,
            step => return step,
        }
    }
}
