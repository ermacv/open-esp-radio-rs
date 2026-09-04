//! Thin Embassy drive for the first peripheral event after `CONNECT_IND`.
//!
//! The chip compositor owns every protocol transition. This adapter only
//! normalizes immediate recovered transitions and turns external controller-
//! time notification into an affine, cancellation-safe resume token.

#![forbid(unsafe_code)]

use core::{future::Future, ops::ControlFlow};

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerEndpointMismatch,
};
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping,
    BluetoothLegacyConnectablePeripheralFirstHciAxis,
    BluetoothLegacyConnectablePeripheralFirstHciFailStop,
    BluetoothLegacyConnectablePeripheralFirstHciProgress,
    BluetoothLegacyConnectablePeripheralFirstHciResetOutcome,
    BluetoothLegacyConnectablePeripheralFirstHciResponsePublication,
    BluetoothLegacyConnectablePeripheralFirstHciResponseWait,
    BluetoothLegacyConnectablePeripheralFirstHciRetry,
    BluetoothLegacyConnectablePeripheralFirstHciRunner,
    BluetoothLegacyConnectablePeripheralFirstHciRunning,
    BluetoothLegacyConnectablePeripheralFirstHciStep, BluetoothSchedulerRunInterruptStorage,
};

/// Executor disposition after all immediately-ready chip transitions were driven.
#[must_use = "retain the controller-time wait, retry edge, or running peripheral owner"]
pub enum EmbassyBluetoothLegacyConnectablePeripheralFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(
        EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    Retry(EmbassyBluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>),
    Running(BluetoothLegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>),
}

/// Normal drive result; `Break` retains a sealed chip failure.
pub type EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<
    'runtime,
    S,
    const CAPACITY: usize,
> = ControlFlow<
    BluetoothLegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive<'runtime, S, CAPACITY>,
>;

/// Accepted stop result; `Break` is the explicit typed Reset branch.
pub type EmbassyBluetoothLegacyConnectablePeripheralFirstStoppingStep<
    'runtime,
    S,
    const CAPACITY: usize,
> = ControlFlow<
    BluetoothLegacyConnectablePeripheralFirstHciResetOutcome<'runtime, S, CAPACITY>,
    EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>,
>;

/// Controller-time wait retaining the exact chip compositor and HCI axis.
#[must_use = "wait for a durable recheck source or retain the exact compositor"]
pub struct EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothLegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>,
}

/// Affine proof that the caller-selected controller-time source completed.
#[must_use = "resume the retained wait exactly once"]
pub struct EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeReady {
    _private: (),
}

/// Retryable pre-RUN edge retaining its independently progressing HCI axis.
#[must_use = "publish a pending response, retry the edge, or retain it"]
pub struct EmbassyBluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    retry: BluetoothLegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>,
}

/// Response publication retaining the exact adapter state in every outcome.
#[must_use = "retain the returned state and response authority"]
pub enum EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication<State> {
    CommandReady(State),
    Published(State),
    Pending(State),
    EndpointMismatch(State),
    Fault {
        state: State,
        error: HciChannelError,
    },
}

/// Begin from an accepted connection with next-command authority.
pub fn begin_legacy_connectable_peripheral_first_command_ready<'runtime, S, const CAPACITY: usize>(
    accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    drive_step(BluetoothLegacyConnectablePeripheralFirstHciRunner::begin_command_ready(accepted))
}

/// Begin while an earlier ordered Controller response is still backpressured.
pub fn begin_legacy_connectable_peripheral_first_response_pending<
    'runtime,
    S,
    const CAPACITY: usize,
>(
    accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
        'runtime,
        S,
        CAPACITY,
    >,
) -> EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    drive_step(BluetoothLegacyConnectablePeripheralFirstHciRunner::begin_response_pending(accepted))
}

/// Begin from an accepted Disable or Reset CPU boundary.
pub fn begin_legacy_connectable_peripheral_first_stopping<'runtime, S, const CAPACITY: usize>(
    accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping<
        'runtime,
        S,
        CAPACITY,
    >,
) -> EmbassyBluetoothLegacyConnectablePeripheralFirstStoppingStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match BluetoothLegacyConnectablePeripheralFirstHciRunner::begin_stopping(accepted) {
        ControlFlow::Break(reset) => ControlFlow::Break(reset),
        ControlFlow::Continue(step) => ControlFlow::Continue(drive_step(step)),
    }
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn hci_axis(&self) -> BluetoothLegacyConnectablePeripheralFirstHciAxis {
        self.runner.hci_axis()
    }

    /// Wait without moving the compositor; cancellation leaves it in this owner.
    pub async fn wait_controller_time<R>(
        &self,
        recheck: R,
    ) -> EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeReady { _private: () }
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<
        BluetoothLegacyConnectablePeripheralFirstHciResponseWait,
        LeControllerEndpointMismatch,
    > {
        self.runner.wait_response_capacity(controller).await
    }

    pub fn resume_controller_time(
        self,
        _ready: EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeReady,
    ) -> EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY> {
        drive_step(self.runner.step())
    }

    pub fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication<Self> {
        map_response_publication(self.runner.try_publish_response(controller), |runner| {
            Self { runner }
        })
    }
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn hci_axis(&self) -> BluetoothLegacyConnectablePeripheralFirstHciAxis {
        self.retry.hci_axis()
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<
        BluetoothLegacyConnectablePeripheralFirstHciResponseWait,
        LeControllerEndpointMismatch,
    > {
        self.retry.wait_response_capacity(controller).await
    }

    pub fn retry(
        self,
    ) -> EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY> {
        drive_step(self.retry.retry())
    }

    pub fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication<Self> {
        map_response_publication(self.retry.try_publish_response(controller), |retry| Self {
            retry,
        })
    }
}

fn drive_step<'runtime, S, const CAPACITY: usize>(
    mut step: BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match step {
            ControlFlow::Break(failure) => return ControlFlow::Break(failure),
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(runner),
            ) => {
                return ControlFlow::Continue(
                    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::WaitControllerTime(
                        EmbassyBluetoothLegacyConnectablePeripheralFirstControllerTimeWait {
                            runner,
                        },
                    ),
                );
            }
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::Recovered(recovered),
            ) => step = recovered.retry(),
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::Retryable(retry),
            ) => {
                return ControlFlow::Continue(
                    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::Retry(
                        EmbassyBluetoothLegacyConnectablePeripheralFirstRetry { retry },
                    ),
                );
            }
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::Running(running),
            ) => {
                return ControlFlow::Continue(
                    EmbassyBluetoothLegacyConnectablePeripheralFirstDrive::Running(running),
                );
            }
        }
    }
}

fn map_response_publication<State, Next>(
    publication: BluetoothLegacyConnectablePeripheralFirstHciResponsePublication<State>,
    map: impl FnOnce(State) -> Next,
) -> EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication<Next> {
    match publication {
        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(state) => {
            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::CommandReady(map(
                state,
            ))
        }
        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Published(state) => {
            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Published(map(
                state,
            ))
        }
        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Pending(state) => {
            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Pending(map(state))
        }
        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(
            state,
        ) => EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(
            map(state),
        ),
        BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Fault { state, error } => {
            EmbassyBluetoothLegacyConnectablePeripheralFirstResponsePublication::Fault {
                state: map(state),
                error,
            }
        }
    }
}
