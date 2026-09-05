//! Finite Embassy-side drive for legacy connectable advertising HCI/radio axes.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyConnectableAdvertisingActivePendingFailStop,
    BluetoothLegacyConnectableAdvertisingActiveResponsePending,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    BluetoothLegacyConnectableAdvertisingHciActiveSession,
    BluetoothLegacyConnectableAdvertisingHciActiveStep,
    BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending,
    BluetoothLegacyConnectableAdvertisingRadioContinuations,
    BluetoothLegacyConnectableAdvertisingResponsePending,
    BluetoothLegacyConnectableAdvertisingStopping,
    BluetoothLegacyConnectableAdvertisingStoppingStep,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
};

/// Five terminal continuations for an Embassy bounded ready drive.
///
/// Immediate lower `Continue` transitions remain internal to the drive loop;
/// exactly one of these callbacks receives the caller's affine context and
/// terminal owner.
pub struct EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations<
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
    EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations<
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
    mut pending: BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
    mut context: Context,
    continuations: EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations<
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >,
) -> R
where
    S: BluetoothSchedulerRunInterruptStorage,
    Waiting: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    Unrelated: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, CAPACITY>,
        BluetoothSchedulerFinishedHardwareListObserved,
    ) -> R,
    NoConnection: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    ConnectionAccepted: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> R,
    FailStop: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ) -> R,
{
    let (mut waiting, mut unrelated, mut no_connection, mut connection_accepted, mut fail_stop) =
        continuations.into_parts();
    loop {
        match pending.step_radio_with(
            context,
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
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
    mut active: BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectableAdvertisingHciActiveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match active.step_radio() {
            BluetoothLegacyConnectableAdvertisingHciActiveStep::Continue(next) => active = next,
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
    mut pending: BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    mut context: Context,
    continuations: EmbassyBluetoothLegacyConnectableAdvertisingReadyContinuations<
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >,
) -> R
where
    S: BluetoothSchedulerRunInterruptStorage,
    Waiting: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    Unrelated: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, CAPACITY>,
        BluetoothSchedulerFinishedHardwareListObserved,
    ) -> R,
    NoConnection: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<'runtime, S, CAPACITY>,
    ) -> R,
    ConnectionAccepted: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> R,
    FailStop: FnMut(
        Context,
        BluetoothLegacyConnectableAdvertisingActivePendingFailStop<'runtime, S, CAPACITY>,
    ) -> R,
{
    let (mut waiting, mut unrelated, mut no_connection, mut connection_accepted, mut fail_stop) =
        continuations.into_parts();
    loop {
        match pending.step_radio_with(
            context,
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
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
    mut stopping: BluetoothLegacyConnectableAdvertisingStopping<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectableAdvertisingStoppingStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match stopping.step() {
            BluetoothLegacyConnectableAdvertisingStoppingStep::Continue(next) => stopping = next,
            step => return step,
        }
    }
}
