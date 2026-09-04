//! HCI-order composition for the first peripheral event after `CONNECT_IND`.
//!
//! Advertising completion has already returned its scheduler list to the CPU.
//! This module retains the exact accepted connection allocation while the
//! phase-typed peripheral runner acquires fresh controller time and publishes
//! its first scheduler `RUN`. A pending Controller response may be published in
//! parallel; next-command authority is deliberately not consumed before RUN.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerEndpointMismatch, LeControllerResetBarrier, LeControllerResponsePending,
    LeControllerResponsePublication,
};
use open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeReceivedPdu, BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
};

use crate::legacy_connectable_advertising_active::{
    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation,
    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop,
    BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
};
use crate::peripheral_connection::BluetoothPeripheralConnectionAcceptedResetCancellationError;
use crate::{
    BluetoothControllerIdleResetBarrier, BluetoothLegacyAdvertisingEventPhase,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending,
    BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping,
    BluetoothLegacyConnectableAdvertisingStopOrder,
    BluetoothLegacyConnectablePeripheralFirstBeginStep,
    BluetoothLegacyConnectablePeripheralFirstCurrentFailStop,
    BluetoothLegacyConnectablePeripheralFirstFailStop,
    BluetoothLegacyConnectablePeripheralFirstFailStopCause,
    BluetoothLegacyConnectablePeripheralFirstPreparationFailStop,
    BluetoothLegacyConnectablePeripheralFirstPreparationPending,
    BluetoothLegacyConnectablePeripheralFirstPreparationStep,
    BluetoothLegacyConnectablePeripheralFirstPrepared,
    BluetoothLegacyConnectablePeripheralFirstPublicationFailStop,
    BluetoothLegacyConnectablePeripheralFirstPublicationStep,
    BluetoothLegacyConnectablePeripheralFirstRecovered,
    BluetoothLegacyConnectablePeripheralFirstRetry,
    BluetoothLegacyConnectablePeripheralFirstRetryCause,
    BluetoothLegacyConnectablePeripheralFirstRetryStep,
    BluetoothLegacyConnectablePeripheralFirstRunStep,
    BluetoothLegacyConnectablePeripheralFirstRunner,
    BluetoothLegacyConnectablePeripheralFirstRunnerStep,
    BluetoothLegacyConnectablePeripheralFirstRunning, BluetoothSchedulerRunInterruptStorage,
};

/// HCI-order axis retained beside the first peripheral scheduler transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstHciAxis {
    CommandReady,
    ResponsePending,
}

/// Result of waiting for response capacity on the current HCI-order axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstHciResponseWait {
    CommandReady,
    CapacityAvailable,
}

/// HCI order separated from a first peripheral RUN for the peripheral-active layer.
#[must_use = "rejoin this exact order with the running peripheral session"]
pub enum BluetoothLegacyConnectablePeripheralFirstHciRunningOrder<'runtime> {
    CommandReady(LeControllerCommandReady<'runtime, ()>),
    ResponsePending(LeControllerResponsePending<'runtime, ()>),
}

enum BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    CommandReady(LeControllerCommandReady<'runtime, Owner>),
    ResponsePending(LeControllerResponsePending<'runtime, Owner>),
}

impl<'runtime, Owner> BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    const fn axis(&self) -> BluetoothLegacyConnectablePeripheralFirstHciAxis {
        match self {
            Self::CommandReady(_) => BluetoothLegacyConnectablePeripheralFirstHciAxis::CommandReady,
            Self::ResponsePending(_) => {
                BluetoothLegacyConnectablePeripheralFirstHciAxis::ResponsePending
            }
        }
    }

    fn into_parts(
        self,
    ) -> (
        Owner,
        BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    ) {
        match self {
            Self::CommandReady(ordered) => {
                let (owner, ordered) = ordered.into_parts();
                (
                    owner,
                    BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => {
                let (owner, response) = response.into_parts();
                (
                    owner,
                    BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response),
                )
            }
        }
    }

    fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Next> {
        match self {
            Self::CommandReady(ordered) => {
                BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(
                    ordered.map_owner(map),
                )
            }
            Self::ResponsePending(response) => {
                BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(
                    response.map_owner(map),
                )
            }
        }
    }

    async fn wait_response_capacity<
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
        match self {
            Self::CommandReady(_) => {
                Ok(BluetoothLegacyConnectablePeripheralFirstHciResponseWait::CommandReady)
            }
            Self::ResponsePending(response) => {
                controller.wait_response_capacity(response).await?;
                Ok(BluetoothLegacyConnectablePeripheralFirstHciResponseWait::CapacityAvailable)
            }
        }
    }

    fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> BluetoothLegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
        match self {
            Self::CommandReady(ordered) => {
                BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::CommandReady(
                    Self::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => match response.try_publish(controller) {
                LeControllerResponsePublication::Published(ordered) => {
                    BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Published(
                        Self::CommandReady(ordered),
                    )
                }
                LeControllerResponsePublication::Pending(response) => {
                    BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Pending(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::EndpointMismatch(response) => {
                    BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::EndpointMismatch(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::Fault {
                    pending: response,
                    error,
                } => BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Fault {
                    order: Self::ResponsePending(response),
                    error,
                },
            },
        }
    }
}

enum BluetoothLegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
    CommandReady(BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Published(BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Pending(BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    EndpointMismatch(BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Fault {
        order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>,
        error: HciChannelError,
    },
}

/// One response-publication transition retaining the exact lower phase.
#[must_use = "retain the lower phase and its HCI-order authority"]
pub enum BluetoothLegacyConnectablePeripheralFirstHciResponsePublication<State> {
    CommandReady(State),
    Published(State),
    Pending(State),
    EndpointMismatch(State),
    Fault {
        state: State,
        error: HciChannelError,
    },
}

/// Advertising evidence retained after Reset retires an accepted request.
#[must_use = "retain the causal packet and advertising completion diagnostics"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciResetEvidence {
    evidence: BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
}

impl BluetoothLegacyConnectablePeripheralFirstHciResetEvidence {
    pub const fn advertising_event_identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.evidence.identity()
    }

    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.evidence.advertising_set()
    }

    pub const fn accepted_packet(&self) -> &BluetoothLeReceivedPdu {
        self.evidence.accepted_packet()
    }

    pub const fn advertising_phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.evidence.phase()
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.evidence.scheduler_status()
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.evidence.rejected_packets()
    }
}

/// Quiescent Reset boundary after the accepted request was retired losslessly.
#[must_use = "apply Reset only through the matching HCI endpoint"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciResetReady<
    'runtime,
    S,
    const CAPACITY: usize,
> {
    reset: BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>,
    evidence: BluetoothLegacyConnectablePeripheralFirstHciResetEvidence,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciResetReady<'runtime, S, CAPACITY>
{
    pub const fn evidence(&self) -> &BluetoothLegacyConnectablePeripheralFirstHciResetEvidence {
        &self.evidence
    }

    /// Separate the idle Reset barrier from immutable causal evidence.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerIdleResetBarrier<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectablePeripheralFirstHciResetEvidence,
    ) {
        (self.reset, self.evidence)
    }
}

/// Public diagnostic for a rejected accepted-request Reset cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause {
    RuntimeBusy,
    GraphIdentityMismatch,
    ReceiveIdentityMismatch,
}

/// Sealed Reset mismatch retaining both the Reset and accepted connection owners.
#[must_use = "retain both affine owners for diagnostic shutdown"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciResetFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _barrier: LeControllerResetBarrier<'runtime, ()>,
    failure: BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop<
        'runtime,
        S,
        CAPACITY,
    >,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciResetFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause {
        match self.failure.cause() {
            BluetoothPeripheralConnectionAcceptedResetCancellationError::RuntimeBusy => {
                BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause::RuntimeBusy
            }
            BluetoothPeripheralConnectionAcceptedResetCancellationError::GraphIdentityMismatch => {
                BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause::GraphIdentityMismatch
            }
            BluetoothPeripheralConnectionAcceptedResetCancellationError::ReceiveIdentityMismatch => {
                BluetoothLegacyConnectablePeripheralFirstHciResetFailStopCause::ReceiveIdentityMismatch
            }
        }
    }
}

/// Reset outcome at the accepted pre-publication boundary.
#[must_use = "retain the quiescent Reset barrier or the sealed cancellation owner"]
pub enum BluetoothLegacyConnectablePeripheralFirstHciResetOutcome<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Ready(BluetoothLegacyConnectablePeripheralFirstHciResetReady<'runtime, S, CAPACITY>),
    FailStop(BluetoothLegacyConnectablePeripheralFirstHciResetFailStop<'runtime, S, CAPACITY>),
}

/// Accepted stop command: Disable continues the connection, Reset retires it.
pub type BluetoothLegacyConnectablePeripheralFirstHciStoppingStep<
    'runtime,
    S,
    const CAPACITY: usize,
> = ControlFlow<
    BluetoothLegacyConnectablePeripheralFirstHciResetOutcome<'runtime, S, CAPACITY>,
    BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>,
>;

enum BluetoothLegacyConnectablePeripheralFirstWait<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Current(BluetoothLegacyConnectablePeripheralFirstRunner<'runtime, S, CAPACITY>),
    Preparation(BluetoothLegacyConnectablePeripheralFirstPreparationPending<'runtime, S, CAPACITY>),
}

/// First peripheral event waiting for a causal controller-time transition.
#[must_use = "step controller time or retain the complete HCI/radio owner"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstWait<'runtime, S, CAPACITY>,
    >,
}

/// One successful or retryable compositor transition before/through RUN.
#[must_use = "retain the wait, recovery, retry, or RUN owner"]
pub enum BluetoothLegacyConnectablePeripheralFirstHciProgress<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothLegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>),
    Recovered(BluetoothLegacyConnectablePeripheralFirstHciRecovered<'runtime, S, CAPACITY>),
    Retryable(BluetoothLegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>),
    Running(BluetoothLegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>),
}

/// One stable compositor transition. `Break` is a sealed lower failure.
pub type BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, const CAPACITY: usize> =
    ControlFlow<
        BluetoothLegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectablePeripheralFirstHciProgress<'runtime, S, CAPACITY>,
    >;

/// Recoverable controller preparation with its exact HCI-order axis.
#[must_use = "retry the same accepted allocation or retain it"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciRecovered<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstRecovered<'runtime, S, CAPACITY>,
    >,
}

/// Retryable pre-RUN edge with its exact HCI-order axis.
#[must_use = "retry the exact publication edge or retain it"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciRetry<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>,
    >,
}

/// First peripheral scheduler event after the irreversible RUN edge.
#[must_use = "retain the running peripheral graph and its HCI-order axis"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciRunning<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstRunning<'runtime, S, CAPACITY>,
    >,
}

type BluetoothLegacyConnectablePeripheralFirstFailureOwner<'runtime, S, const CAPACITY: usize> =
    ControlFlow<
        BluetoothLegacyConnectablePeripheralFirstFailStop<'runtime, S, CAPACITY>,
        ControlFlow<
            BluetoothLegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, CAPACITY>,
            ControlFlow<
                BluetoothLegacyConnectablePeripheralFirstPreparationFailStop<'runtime, S, CAPACITY>,
                BluetoothLegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, CAPACITY>,
            >,
        >,
    >;

const fn failure_cause<S, const CAPACITY: usize>(
    failure: &BluetoothLegacyConnectablePeripheralFirstFailureOwner<'_, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match failure {
        ControlFlow::Break(failure) => failure.cause(),
        ControlFlow::Continue(ControlFlow::Break(failure)) => failure.cause(),
        ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure))) => {
            failure.cause()
        }
        ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(failure))) => {
            failure.cause()
        }
    }
}

/// Sealed lower failure retaining either command-ready or pending-response order.
#[must_use = "retain both affine axes for diagnostic shutdown"]
pub struct BluetoothLegacyConnectablePeripheralFirstHciFailStop<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstFailureOwner<'runtime, S, CAPACITY>,
    >,
}

impl<S, const CAPACITY: usize> BluetoothLegacyConnectablePeripheralFirstHciFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause {
        match &self.order {
            BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                failure_cause(ordered.owner())
            }
            BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                failure_cause(response.owner())
            }
        }
    }

    pub const fn hci_axis(&self) -> BluetoothLegacyConnectablePeripheralFirstHciAxis {
        self.order.axis()
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn begin_command_ready(
        accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedReady<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (ordered, accepted) = accepted.into_parts();
        begin_with_order(
            BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered),
            accepted,
        )
    }

    pub fn begin_response_pending(
        accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (response, accepted) = accepted.into_parts();
        begin_with_order(
            BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response),
            accepted,
        )
    }

    pub fn begin_stopping(
        accepted: BluetoothLegacyConnectableAdvertisingConnectionAcceptedStopping<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> BluetoothLegacyConnectablePeripheralFirstHciStoppingStep<'runtime, S, CAPACITY> {
        let (accepted, order) = accepted.into_parts();
        match order {
            BluetoothLegacyConnectableAdvertisingStopOrder::Disable(disable) => {
                ControlFlow::Continue(begin_with_order(
                    BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(
                        disable.into_stopped_response(),
                    ),
                    accepted,
                ))
            }
            BluetoothLegacyConnectableAdvertisingStopOrder::Reset(barrier) => {
                match accepted.cancel_connection_for_reset() {
                    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation::Cancelled(
                        cancelled,
                    ) => {
                        let (task, evidence) = cancelled.into_parts();
                        ControlFlow::Break(
                            BluetoothLegacyConnectablePeripheralFirstHciResetOutcome::Ready(
                                BluetoothLegacyConnectablePeripheralFirstHciResetReady {
                                    reset: BluetoothControllerIdleResetBarrier::new(
                                        barrier.map_owner(|()| task),
                                    ),
                                    evidence:
                                        BluetoothLegacyConnectablePeripheralFirstHciResetEvidence {
                                            evidence,
                                        },
                                },
                            ),
                        )
                    }
                    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation::FailStop(
                        failure,
                    ) => ControlFlow::Break(
                        BluetoothLegacyConnectablePeripheralFirstHciResetOutcome::FailStop(
                            BluetoothLegacyConnectablePeripheralFirstHciResetFailStop {
                                _barrier: barrier,
                                failure,
                            },
                        ),
                    ),
                }
            }
        }
    }

    pub fn step(self) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (waiting, order) = self.order.into_parts();
        match waiting {
            BluetoothLegacyConnectablePeripheralFirstWait::Current(runner) => match runner.step() {
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::WaitControllerTime(runner) => {
                    wait_current(order, runner)
                }
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::Preparation(step) => {
                    preparation_step(order, step)
                }
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::FailStop(failure) => {
                    fail_stop(order, ControlFlow::Continue(ControlFlow::Break(failure)))
                }
            },
            BluetoothLegacyConnectablePeripheralFirstWait::Preparation(pending) => {
                preparation_step(order, pending.recheck())
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciRecovered<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn retry(self) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (recovered, order) = self.order.into_parts();
        begin_step(order, recovered.retry())
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstRetryCause<'_, S::Error> {
        match &self.order {
            BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                ordered.owner().cause()
            }
            BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                response.owner().cause()
            }
        }
    }

    pub fn retry(self) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (retry, order) = self.order.into_parts();
        match retry.retry() {
            ControlFlow::Break(failure) => fail_stop(
                order,
                ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(failure))),
            ),
            ControlFlow::Continue(step) => retry_step(order, step),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn event_counter(&self) -> u16 {
        match &self.order {
            BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                ordered.owner().event_counter()
            }
            BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                response.owner().event_counter()
            }
        }
    }

    /// Separate the running peripheral owner from its exact HCI-order axis.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectablePeripheralFirstRunning<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectablePeripheralFirstHciRunningOrder<'runtime>,
    ) {
        let (running, order) = self.order.into_parts();
        let order = match order {
            BluetoothLegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                BluetoothLegacyConnectablePeripheralFirstHciRunningOrder::CommandReady(ordered)
            }
            BluetoothLegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                BluetoothLegacyConnectablePeripheralFirstHciRunningOrder::ResponsePending(response)
            }
        };
        (running, order)
    }
}

macro_rules! impl_hci_io {
    ($state:ident) => {
        impl<'runtime, S, const CAPACITY: usize> $state<'runtime, S, CAPACITY>
        where
            S: BluetoothSchedulerRunInterruptStorage,
        {
            pub const fn hci_axis(&self) -> BluetoothLegacyConnectablePeripheralFirstHciAxis {
                self.order.axis()
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
                self.order.wait_response_capacity(controller).await
            }

            pub fn try_publish_response<
                M: RawMutex,
                const H2C: usize,
                const C2H: usize,
                const PACKET: usize,
            >(
                self,
                controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
            ) -> BluetoothLegacyConnectablePeripheralFirstHciResponsePublication<Self> {
                map_order_publication(self.order.try_publish_response(controller), |order| Self {
                    order,
                })
            }
        }
    };
}

impl_hci_io!(BluetoothLegacyConnectablePeripheralFirstHciRunner);
impl_hci_io!(BluetoothLegacyConnectablePeripheralFirstHciRecovered);
impl_hci_io!(BluetoothLegacyConnectablePeripheralFirstHciRetry);
impl_hci_io!(BluetoothLegacyConnectablePeripheralFirstHciRunning);

fn map_order_publication<'runtime, Owner, State>(
    publication: BluetoothLegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner>,
    map: impl FnOnce(BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>) -> State,
) -> BluetoothLegacyConnectablePeripheralFirstHciResponsePublication<State> {
    match publication {
        BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::CommandReady(order) => {
            BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(map(
                order,
            ))
        }
        BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Published(order) => {
            BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Published(map(order))
        }
        BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Pending(order) => {
            BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Pending(map(order))
        }
        BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::EndpointMismatch(order) => {
            BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(map(
                order,
            ))
        }
        BluetoothLegacyConnectablePeripheralFirstHciOrderPublication::Fault { order, error } => {
            BluetoothLegacyConnectablePeripheralFirstHciResponsePublication::Fault {
                state: map(order),
                error,
            }
        }
    }
}

fn begin_with_order<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    accepted: crate::BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<
        'runtime,
        S,
        CAPACITY,
    >,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    begin_step(
        order,
        BluetoothLegacyConnectablePeripheralFirstRunner::begin(accepted),
    )
}

fn begin_step<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothLegacyConnectablePeripheralFirstBeginStep::WaitControllerTime(runner) => {
            wait_current(order, runner)
        }
        BluetoothLegacyConnectablePeripheralFirstBeginStep::FailStop(failure) => {
            fail_stop(order, ControlFlow::Break(failure))
        }
    }
}

fn wait_current<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    runner: BluetoothLegacyConnectablePeripheralFirstRunner<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ControlFlow::Continue(
        BluetoothLegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(
            BluetoothLegacyConnectablePeripheralFirstHciRunner {
                order: order
                    .map_owner(|()| BluetoothLegacyConnectablePeripheralFirstWait::Current(runner)),
            },
        ),
    )
}

fn preparation_step<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: BluetoothLegacyConnectablePeripheralFirstPreparationStep<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothLegacyConnectablePeripheralFirstPreparationStep::WaitControllerTime(pending) => {
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(
                    BluetoothLegacyConnectablePeripheralFirstHciRunner {
                        order: order.map_owner(|()| {
                            BluetoothLegacyConnectablePeripheralFirstWait::Preparation(pending)
                        }),
                    },
                ),
            )
        }
        BluetoothLegacyConnectablePeripheralFirstPreparationStep::Prepared(prepared) => {
            publish_prepared(order, prepared)
        }
        BluetoothLegacyConnectablePeripheralFirstPreparationStep::Recovered(recovered) => {
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::Recovered(
                    BluetoothLegacyConnectablePeripheralFirstHciRecovered {
                        order: order.map_owner(|()| recovered),
                    },
                ),
            )
        }
        BluetoothLegacyConnectablePeripheralFirstPreparationStep::FailStop(failure) => fail_stop(
            order,
            ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure))),
        ),
    }
}

fn publish_prepared<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    prepared: BluetoothLegacyConnectablePeripheralFirstPrepared<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match prepared.publish() {
        ControlFlow::Break(failure) => fail_stop(
            order,
            ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(failure))),
        ),
        ControlFlow::Continue(step) => publication_step(order, step),
    }
}

fn publication_step<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: BluetoothLegacyConnectablePeripheralFirstPublicationStep<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothLegacyConnectablePeripheralFirstPublicationStep::HeadPublished(head) => {
            run_step(order, head.start())
        }
        BluetoothLegacyConnectablePeripheralFirstPublicationStep::Retryable(retry) => {
            retryable(order, retry)
        }
    }
}

fn retry_step<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: BluetoothLegacyConnectablePeripheralFirstRetryStep<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothLegacyConnectablePeripheralFirstRetryStep::HeadPublication(step) => {
            publication_step(order, step)
        }
        BluetoothLegacyConnectablePeripheralFirstRetryStep::InterruptStorage(step) => {
            run_step(order, step)
        }
    }
}

fn run_step<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: BluetoothLegacyConnectablePeripheralFirstRunStep<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothLegacyConnectablePeripheralFirstRunStep::Running(running) => {
            ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstHciProgress::Running(
                    BluetoothLegacyConnectablePeripheralFirstHciRunning {
                        order: order.map_owner(|()| running),
                    },
                ),
            )
        }
        BluetoothLegacyConnectablePeripheralFirstRunStep::Retryable(retry) => {
            retryable(order, retry)
        }
    }
}

fn retryable<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    retry: BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ControlFlow::Continue(
        BluetoothLegacyConnectablePeripheralFirstHciProgress::Retryable(
            BluetoothLegacyConnectablePeripheralFirstHciRetry {
                order: order.map_owner(|()| retry),
            },
        ),
    )
}

fn fail_stop<'runtime, S, const CAPACITY: usize>(
    order: BluetoothLegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    failure: BluetoothLegacyConnectablePeripheralFirstFailureOwner<'runtime, S, CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ControlFlow::Break(BluetoothLegacyConnectablePeripheralFirstHciFailStop {
        order: order.map_owner(|()| failure),
    })
}
