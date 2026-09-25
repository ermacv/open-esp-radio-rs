//! Thin Embassy drive for the first peripheral event after `CONNECT_IND`.
//!
//! The chip compositor owns every protocol transition. This adapter only
//! normalizes immediate recovered transitions and turns external controller-
//! time notification into an affine, cancellation-safe resume token.

#![forbid(unsafe_code)]

use core::{future::Future, ops::ControlFlow};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerEndpointMismatch,
};

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::{
        advertising::{
            LegacyConnectableAdvertisingConnectionAcceptedReady,
            LegacyConnectableAdvertisingConnectionAcceptedResponsePending,
            LegacyConnectableAdvertisingConnectionAcceptedStopping,
        },
        peripheral::{
            LegacyConnectablePeripheralFirstHciAxis, LegacyConnectablePeripheralFirstHciFailStop,
            LegacyConnectablePeripheralFirstHciProgress,
            LegacyConnectablePeripheralFirstHciResetOutcome,
            LegacyConnectablePeripheralFirstHciResponsePublication,
            LegacyConnectablePeripheralFirstHciResponseWait,
            LegacyConnectablePeripheralFirstHciRetry, LegacyConnectablePeripheralFirstHciRunner,
            LegacyConnectablePeripheralFirstHciRunning, LegacyConnectablePeripheralFirstHciStep,
        },
    },
};

/// Executor disposition after all immediately-ready chip transitions were driven.
#[must_use = "retain the controller-time wait, retry edge, or running peripheral owner"]
pub enum LegacyConnectablePeripheralFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>),
    Retry(PeripheralFirstSessionRetry<'runtime, S, CAPACITY>),
    Running(LegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>),
}

/// Normal drive result; `Break` retains a sealed chip failure.
pub type LegacyConnectablePeripheralFirstDriveStep<'runtime, S, const CAPACITY: usize> =
    ControlFlow<
        LegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
        LegacyConnectablePeripheralFirstDrive<'runtime, S, CAPACITY>,
    >;

/// Accepted stop result; `Break` is the explicit typed Reset branch.
pub type LegacyConnectablePeripheralFirstStoppingStep<'runtime, S, const CAPACITY: usize> =
    ControlFlow<
        LegacyConnectablePeripheralFirstHciResetOutcome<'runtime, S, CAPACITY>,
        LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>,
    >;

/// Controller-time wait retaining the exact chip compositor and HCI axis.
#[must_use = "wait for a durable recheck source or retain the exact compositor"]
pub struct LegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: LegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>,
}

/// Affine proof that the caller-selected controller-time source completed.
#[must_use = "resume the retained wait exactly once"]
pub struct LegacyConnectablePeripheralFirstControllerTimeReady {
    _private: (),
}

/// Retryable pre-RUN edge retaining its independently progressing HCI axis.
#[must_use = "publish a pending response, retry the edge, or retain it"]
pub struct PeripheralFirstSessionRetry<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    retry: LegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>,
}

/// Response publication retaining the exact adapter state in every outcome.
#[must_use = "retain the returned state and response authority"]
pub enum LegacyConnectablePeripheralFirstResponsePublication<State> {
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
    accepted: LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    drive_step(LegacyConnectablePeripheralFirstHciRunner::begin_command_ready(accepted))
}

/// Begin while an earlier ordered Controller response is still backpressured.
pub fn begin_legacy_connectable_peripheral_first_response_pending<
    'runtime,
    S,
    const CAPACITY: usize,
>(
    accepted: LegacyConnectableAdvertisingConnectionAcceptedResponsePending<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    drive_step(LegacyConnectablePeripheralFirstHciRunner::begin_response_pending(accepted))
}

/// Begin from an accepted Disable or Reset CPU boundary.
pub fn begin_legacy_connectable_peripheral_first_stopping<'runtime, S, const CAPACITY: usize>(
    accepted: LegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstStoppingStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match LegacyConnectablePeripheralFirstHciRunner::begin_stopping(accepted) {
        ControlFlow::Break(reset) => ControlFlow::Break(reset),
        ControlFlow::Continue(step) => ControlFlow::Continue(drive_step(step)),
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectablePeripheralFirstControllerTimeWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn hci_axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
        self.runner.hci_axis()
    }

    /// Wait without moving the compositor; cancellation leaves it in this owner.
    pub async fn wait_controller_time<R>(
        &self,
        recheck: R,
    ) -> LegacyConnectablePeripheralFirstControllerTimeReady
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        LegacyConnectablePeripheralFirstControllerTimeReady { _private: () }
    }

    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<LegacyConnectablePeripheralFirstHciResponseWait, LeControllerEndpointMismatch> {
        self.runner.wait_response_capacity(controller).await
    }

    pub fn resume_controller_time(
        self,
        _ready: LegacyConnectablePeripheralFirstControllerTimeReady,
    ) -> LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY> {
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
    ) -> LegacyConnectablePeripheralFirstResponsePublication<Self> {
        map_response_publication(self.runner.try_publish_response(controller), |runner| {
            Self { runner }
        })
    }
}

impl<'runtime, S, const CAPACITY: usize> PeripheralFirstSessionRetry<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn hci_axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
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
    ) -> Result<LegacyConnectablePeripheralFirstHciResponseWait, LeControllerEndpointMismatch> {
        self.retry.wait_response_capacity(controller).await
    }

    pub fn retry(self) -> LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY> {
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
    ) -> LegacyConnectablePeripheralFirstResponsePublication<Self> {
        map_response_publication(self.retry.try_publish_response(controller), |retry| Self {
            retry,
        })
    }
}

fn drive_step<'runtime, S, const CAPACITY: usize>(
    mut step: LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstDriveStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match step {
            ControlFlow::Break(failure) => return ControlFlow::Break(failure),
            ControlFlow::Continue(
                LegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(runner),
            ) => {
                return ControlFlow::Continue(
                    LegacyConnectablePeripheralFirstDrive::WaitControllerTime(
                        LegacyConnectablePeripheralFirstControllerTimeWait { runner },
                    ),
                );
            }
            ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Recovered(
                recovered,
            )) => step = recovered.retry(),
            ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Retryable(
                retry,
            )) => {
                return ControlFlow::Continue(LegacyConnectablePeripheralFirstDrive::Retry(
                    PeripheralFirstSessionRetry { retry },
                ));
            }
            ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Running(
                running,
            )) => {
                return ControlFlow::Continue(LegacyConnectablePeripheralFirstDrive::Running(
                    running,
                ));
            }
        }
    }
}

fn map_response_publication<State, Next>(
    publication: LegacyConnectablePeripheralFirstHciResponsePublication<State>,
    map: impl FnOnce(State) -> Next,
) -> LegacyConnectablePeripheralFirstResponsePublication<Next> {
    match publication {
        LegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(state) => {
            LegacyConnectablePeripheralFirstResponsePublication::CommandReady(map(state))
        }
        LegacyConnectablePeripheralFirstHciResponsePublication::Published(state) => {
            LegacyConnectablePeripheralFirstResponsePublication::Published(map(state))
        }
        LegacyConnectablePeripheralFirstHciResponsePublication::Pending(state) => {
            LegacyConnectablePeripheralFirstResponsePublication::Pending(map(state))
        }
        LegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(state) => {
            LegacyConnectablePeripheralFirstResponsePublication::EndpointMismatch(map(state))
        }
        LegacyConnectablePeripheralFirstHciResponsePublication::Fault { state, error } => {
            LegacyConnectablePeripheralFirstResponsePublication::Fault {
                state: map(state),
                error,
            }
        }
    }
}
