//! HCI-order composition for the first peripheral event after `CONNECT_IND`.
//!
//! Advertising completion has already returned its scheduler list to the CPU.
//! This module retains the exact accepted connection allocation while the
//! phase-typed peripheral runner acquires fresh controller time and publishes
//! its first scheduler `RUN`. A pending Controller response may be published in
//! parallel; next-command authority is deliberately not consumed before RUN.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use crate::{
    controller::{ControllerIdleResetBarrier, SchedulerRunInterruptStorage},
    le::{
        advertising::{
            LegacyAdvertisingEventPhase, LegacyConnectableAdvertisingConnectionAcceptedReady,
            LegacyConnectableAdvertisingConnectionAcceptedResponsePending,
            LegacyConnectableAdvertisingConnectionAcceptedStopping,
            LegacyConnectableAdvertisingStopOrder,
            connectable::active::{
                LegacyConnectableAdvertisingPeripheralResetCancellation,
                LegacyConnectableAdvertisingPeripheralResetCancellationFailStop,
                LegacyConnectableAdvertisingPeripheralResetEvidence,
            },
        },
        peripheral::{
            BluetoothLegacyConnectablePeripheralFirstRetry,
            LegacyConnectablePeripheralFirstBeginStep,
            LegacyConnectablePeripheralFirstCurrentFailStop,
            LegacyConnectablePeripheralFirstFailStop,
            LegacyConnectablePeripheralFirstFailStopCause,
            LegacyConnectablePeripheralFirstPreparationFailStop,
            LegacyConnectablePeripheralFirstPreparationPending,
            LegacyConnectablePeripheralFirstPreparationStep,
            LegacyConnectablePeripheralFirstPrepared,
            LegacyConnectablePeripheralFirstPublicationFailStop,
            LegacyConnectablePeripheralFirstPublicationStep,
            LegacyConnectablePeripheralFirstRecovered, LegacyConnectablePeripheralFirstRetryCause,
            LegacyConnectablePeripheralFirstRetryStep, LegacyConnectablePeripheralFirstRunStep,
            LegacyConnectablePeripheralFirstRunner, LegacyConnectablePeripheralFirstRunnerStep,
            LegacyConnectablePeripheralFirstRunning,
            connection::PeripheralConnectionAcceptedResetCancellationError,
        },
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint, LeControllerCommandReady,
    LeControllerEndpointMismatch, LeControllerResetBarrier, LeControllerResponsePending,
    LeControllerResponsePublication,
};

use oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet;

use oer_esp32s31_bluetooth_memory::{
    LeReceivedPdu, LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
};

/// HCI-order axis retained beside the first peripheral scheduler transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstHciAxis {
    CommandReady,
    ResponsePending,
}

/// Result of waiting for response capacity on the current HCI-order axis.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstHciResponseWait {
    CommandReady,
    CapacityAvailable,
}

/// HCI order separated from a first peripheral RUN for the peripheral-active layer.
#[must_use = "rejoin this exact order with the running peripheral session"]
pub enum LegacyConnectablePeripheralFirstHciRunningOrder<'runtime> {
    CommandReady(LeControllerCommandReady<'runtime, ()>),
    ResponsePending(LeControllerResponsePending<'runtime, ()>),
}

pub(super) enum LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    CommandReady(LeControllerCommandReady<'runtime, Owner>),
    ResponsePending(LeControllerResponsePending<'runtime, Owner>),
}

impl<'runtime, Owner> LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner> {
    pub(super) fn owner(&self) -> &Owner {
        match self {
            Self::CommandReady(ordered) => ordered.owner(),
            Self::ResponsePending(response) => response.owner(),
        }
    }

    pub(super) const fn axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
        match self {
            Self::CommandReady(_) => LegacyConnectablePeripheralFirstHciAxis::CommandReady,
            Self::ResponsePending(_) => LegacyConnectablePeripheralFirstHciAxis::ResponsePending,
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (
        Owner,
        LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    ) {
        match self {
            Self::CommandReady(ordered) => {
                let (owner, ordered) = ordered.into_parts();
                (
                    owner,
                    LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => {
                let (owner, response) = response.into_parts();
                (
                    owner,
                    LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response),
                )
            }
        }
    }

    pub(super) fn map_owner<Next>(
        self,
        map: impl FnOnce(Owner) -> Next,
    ) -> LegacyConnectablePeripheralFirstHciOrder<'runtime, Next> {
        match self {
            Self::CommandReady(ordered) => {
                LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered.map_owner(map))
            }
            Self::ResponsePending(response) => {
                LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response.map_owner(map))
            }
        }
    }

    pub(super) async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<LegacyConnectablePeripheralFirstHciResponseWait, LeControllerEndpointMismatch> {
        match self {
            Self::CommandReady(_) => {
                Ok(LegacyConnectablePeripheralFirstHciResponseWait::CommandReady)
            }
            Self::ResponsePending(response) => {
                controller.wait_response_capacity(response).await?;
                Ok(LegacyConnectablePeripheralFirstHciResponseWait::CapacityAvailable)
            }
        }
    }

    pub(super) fn try_publish_response<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> LegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
        match self {
            Self::CommandReady(ordered) => {
                LegacyConnectablePeripheralFirstHciOrderPublication::CommandReady(
                    Self::CommandReady(ordered),
                )
            }
            Self::ResponsePending(response) => match response.try_publish(controller) {
                LeControllerResponsePublication::Published(ordered) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::Published(
                        Self::CommandReady(ordered),
                    )
                }
                LeControllerResponsePublication::Pending(response) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::Pending(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::EndpointMismatch(response) => {
                    LegacyConnectablePeripheralFirstHciOrderPublication::EndpointMismatch(
                        Self::ResponsePending(response),
                    )
                }
                LeControllerResponsePublication::Fault {
                    pending: response,
                    error,
                } => LegacyConnectablePeripheralFirstHciOrderPublication::Fault {
                    order: Self::ResponsePending(response),
                    error,
                },
            },
        }
    }
}

pub(super) enum LegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner> {
    CommandReady(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Published(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Pending(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    EndpointMismatch(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>),
    Fault {
        order: LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>,
        error: HciChannelError,
    },
}

/// One response-publication transition retaining the exact lower phase.
#[must_use = "retain the lower phase and its HCI-order authority"]
pub enum LegacyConnectablePeripheralFirstHciResponsePublication<State> {
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
pub struct LegacyConnectablePeripheralFirstHciResetEvidence {
    evidence: LegacyConnectableAdvertisingPeripheralResetEvidence,
}

impl LegacyConnectablePeripheralFirstHciResetEvidence {
    pub const fn advertising_event_identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.evidence.identity()
    }

    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.evidence.advertising_set()
    }

    pub const fn accepted_packet(&self) -> &LeReceivedPdu {
        self.evidence.accepted_packet()
    }

    pub const fn advertising_phase(&self) -> LegacyAdvertisingEventPhase {
        self.evidence.phase()
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.evidence.scheduler_status()
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.evidence.rejected_packets()
    }
}

/// Quiescent Reset boundary after the accepted request was retired losslessly.
#[must_use = "apply Reset only through the matching HCI endpoint"]
pub struct LegacyConnectablePeripheralFirstHciResetReady<'runtime, S, const CAPACITY: usize> {
    reset: ControllerIdleResetBarrier<'runtime, S, CAPACITY>,
    evidence: LegacyConnectablePeripheralFirstHciResetEvidence,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectablePeripheralFirstHciResetReady<'runtime, S, CAPACITY>
{
    pub const fn evidence(&self) -> &LegacyConnectablePeripheralFirstHciResetEvidence {
        &self.evidence
    }

    /// Separate the idle Reset barrier from immutable causal evidence.
    pub fn into_parts(
        self,
    ) -> (
        ControllerIdleResetBarrier<'runtime, S, CAPACITY>,
        LegacyConnectablePeripheralFirstHciResetEvidence,
    ) {
        (self.reset, self.evidence)
    }
}

/// Public diagnostic for a rejected accepted-request Reset cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstHciResetFailStopCause {
    RuntimeBusy,
    GraphIdentityMismatch,
    ReceiveIdentityMismatch,
}

/// Sealed Reset mismatch retaining both the Reset and accepted connection owners.
#[must_use = "retain both affine owners for diagnostic shutdown"]
pub struct LegacyConnectablePeripheralFirstHciResetFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _barrier: LeControllerResetBarrier<'runtime, ()>,
    failure: LegacyConnectableAdvertisingPeripheralResetCancellationFailStop<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> LegacyConnectablePeripheralFirstHciResetFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstHciResetFailStopCause {
        match self.failure.cause() {
            PeripheralConnectionAcceptedResetCancellationError::RuntimeBusy => {
                LegacyConnectablePeripheralFirstHciResetFailStopCause::RuntimeBusy
            }
            PeripheralConnectionAcceptedResetCancellationError::GraphIdentityMismatch => {
                LegacyConnectablePeripheralFirstHciResetFailStopCause::GraphIdentityMismatch
            }
            PeripheralConnectionAcceptedResetCancellationError::ReceiveIdentityMismatch => {
                LegacyConnectablePeripheralFirstHciResetFailStopCause::ReceiveIdentityMismatch
            }
        }
    }
}

/// Reset outcome at the accepted pre-publication boundary.
#[must_use = "retain the quiescent Reset barrier or the sealed cancellation owner"]
pub enum LegacyConnectablePeripheralFirstHciResetOutcome<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Ready(LegacyConnectablePeripheralFirstHciResetReady<'runtime, S, CAPACITY>),
    FailStop(LegacyConnectablePeripheralFirstHciResetFailStop<'runtime, S, CAPACITY>),
}

/// Accepted stop command: Disable continues the connection, Reset retires it.
pub type LegacyConnectablePeripheralFirstHciStoppingStep<'runtime, S, const CAPACITY: usize> =
    ControlFlow<
        LegacyConnectablePeripheralFirstHciResetOutcome<'runtime, S, CAPACITY>,
        LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>,
    >;

enum LegacyConnectablePeripheralFirstWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Current(LegacyConnectablePeripheralFirstRunner<'runtime, S, CAPACITY>),
    Preparation(LegacyConnectablePeripheralFirstPreparationPending<'runtime, S, CAPACITY>),
}

/// First peripheral event waiting for a causal controller-time transition.
#[must_use = "step controller time or retain the complete HCI/radio owner"]
pub struct LegacyConnectablePeripheralFirstHciRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    order: LegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        LegacyConnectablePeripheralFirstWait<'runtime, S, CAPACITY>,
    >,
}

/// One successful or retryable compositor transition before/through RUN.
#[must_use = "retain the wait, recovery, retry, or RUN owner"]
pub enum LegacyConnectablePeripheralFirstHciProgress<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>),
    Recovered(LegacyConnectablePeripheralFirstHciRecovered<'runtime, S, CAPACITY>),
    Retryable(LegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>),
    Running(LegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>),
}

/// One stable compositor transition. `Break` is a sealed lower failure.
pub type LegacyConnectablePeripheralFirstHciStep<'runtime, S, const CAPACITY: usize> = ControlFlow<
    LegacyConnectablePeripheralFirstHciFailStop<'runtime, S, CAPACITY>,
    LegacyConnectablePeripheralFirstHciProgress<'runtime, S, CAPACITY>,
>;

/// Recoverable controller preparation with its exact HCI-order axis.
#[must_use = "retry the same accepted allocation or retain it"]
pub struct LegacyConnectablePeripheralFirstHciRecovered<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    order: LegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        LegacyConnectablePeripheralFirstRecovered<'runtime, S, CAPACITY>,
    >,
}

/// Retryable pre-RUN edge with its exact HCI-order axis.
#[must_use = "retry the exact publication edge or retain it"]
pub struct LegacyConnectablePeripheralFirstHciRetry<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    order: LegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>,
    >,
}

/// First peripheral scheduler event after the irreversible RUN edge.
#[must_use = "retain the running peripheral graph and its HCI-order axis"]
pub struct LegacyConnectablePeripheralFirstHciRunning<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    order: LegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        LegacyConnectablePeripheralFirstRunning<'runtime, S, CAPACITY>,
    >,
}

type LegacyConnectablePeripheralFirstFailureOwner<'runtime, S, const CAPACITY: usize> = ControlFlow<
    LegacyConnectablePeripheralFirstFailStop<'runtime, S, CAPACITY>,
    ControlFlow<
        LegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, CAPACITY>,
        ControlFlow<
            LegacyConnectablePeripheralFirstPreparationFailStop<'runtime, S, CAPACITY>,
            LegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, CAPACITY>,
        >,
    >,
>;

const fn failure_cause<S, const CAPACITY: usize>(
    failure: &LegacyConnectablePeripheralFirstFailureOwner<'_, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstFailStopCause
where
    S: SchedulerRunInterruptStorage,
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
pub struct LegacyConnectablePeripheralFirstHciFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    order: LegacyConnectablePeripheralFirstHciOrder<
        'runtime,
        LegacyConnectablePeripheralFirstFailureOwner<'runtime, S, CAPACITY>,
    >,
}

impl<S, const CAPACITY: usize> LegacyConnectablePeripheralFirstHciFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstFailStopCause {
        match &self.order {
            LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                failure_cause(ordered.owner())
            }
            LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                failure_cause(response.owner())
            }
        }
    }

    pub const fn hci_axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
        self.order.axis()
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectablePeripheralFirstHciRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn begin_command_ready(
        accepted: LegacyConnectableAdvertisingConnectionAcceptedReady<'runtime, S, CAPACITY>,
    ) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (ordered, accepted) = accepted.into_parts();
        begin_with_order(
            LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered),
            accepted,
        )
    }

    pub fn begin_response_pending(
        accepted: LegacyConnectableAdvertisingConnectionAcceptedResponsePending<
            'runtime,
            S,
            CAPACITY,
        >,
    ) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (response, accepted) = accepted.into_parts();
        begin_with_order(
            LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response),
            accepted,
        )
    }

    pub fn begin_stopping(
        accepted: LegacyConnectableAdvertisingConnectionAcceptedStopping<'runtime, S, CAPACITY>,
    ) -> LegacyConnectablePeripheralFirstHciStoppingStep<'runtime, S, CAPACITY> {
        let (accepted, order) = accepted.into_parts();
        match order {
            LegacyConnectableAdvertisingStopOrder::Disable(disable) => {
                ControlFlow::Continue(begin_with_order(
                    LegacyConnectablePeripheralFirstHciOrder::ResponsePending(
                        disable.into_stopped_response(),
                    ),
                    accepted,
                ))
            }
            LegacyConnectableAdvertisingStopOrder::Reset(barrier) => {
                match accepted.cancel_connection_for_reset() {
                    LegacyConnectableAdvertisingPeripheralResetCancellation::Cancelled(
                        cancelled,
                    ) => {
                        let (task, evidence) = cancelled.into_parts();
                        ControlFlow::Break(LegacyConnectablePeripheralFirstHciResetOutcome::Ready(
                            LegacyConnectablePeripheralFirstHciResetReady {
                                reset: ControllerIdleResetBarrier::new(
                                    barrier.map_owner(|()| task),
                                ),
                                evidence: LegacyConnectablePeripheralFirstHciResetEvidence {
                                    evidence,
                                },
                            },
                        ))
                    }
                    LegacyConnectableAdvertisingPeripheralResetCancellation::FailStop(failure) => {
                        ControlFlow::Break(
                            LegacyConnectablePeripheralFirstHciResetOutcome::FailStop(
                                LegacyConnectablePeripheralFirstHciResetFailStop {
                                    _barrier: barrier,
                                    failure,
                                },
                            ),
                        )
                    }
                }
            }
        }
    }

    pub fn step(self) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (waiting, order) = self.order.into_parts();
        match waiting {
            LegacyConnectablePeripheralFirstWait::Current(runner) => match runner.step() {
                LegacyConnectablePeripheralFirstRunnerStep::WaitControllerTime(runner) => {
                    wait_current(order, runner)
                }
                LegacyConnectablePeripheralFirstRunnerStep::Preparation(step) => {
                    preparation_step(order, step)
                }
                LegacyConnectablePeripheralFirstRunnerStep::FailStop(failure) => {
                    fail_stop(order, ControlFlow::Continue(ControlFlow::Break(failure)))
                }
            },
            LegacyConnectablePeripheralFirstWait::Preparation(pending) => {
                preparation_step(order, pending.recheck())
            }
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectablePeripheralFirstHciRecovered<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn retry(self) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
        let (recovered, order) = self.order.into_parts();
        begin_step(order, recovered.retry())
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectablePeripheralFirstHciRetry<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cause(&self) -> LegacyConnectablePeripheralFirstRetryCause<'_, S::Error> {
        match &self.order {
            LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                ordered.owner().cause()
            }
            LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                response.owner().cause()
            }
        }
    }

    pub fn retry(self) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY> {
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
    LegacyConnectablePeripheralFirstHciRunning<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn event_counter(&self) -> u16 {
        match &self.order {
            LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                ordered.owner().event_counter()
            }
            LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                response.owner().event_counter()
            }
        }
    }

    /// Separate the running peripheral owner from its exact HCI-order axis.
    pub fn into_parts(
        self,
    ) -> (
        LegacyConnectablePeripheralFirstRunning<'runtime, S, CAPACITY>,
        LegacyConnectablePeripheralFirstHciRunningOrder<'runtime>,
    ) {
        let (running, order) = self.order.into_parts();
        let order = match order {
            LegacyConnectablePeripheralFirstHciOrder::CommandReady(ordered) => {
                LegacyConnectablePeripheralFirstHciRunningOrder::CommandReady(ordered)
            }
            LegacyConnectablePeripheralFirstHciOrder::ResponsePending(response) => {
                LegacyConnectablePeripheralFirstHciRunningOrder::ResponsePending(response)
            }
        };
        (running, order)
    }
}

macro_rules! impl_hci_io {
    ($state:ident) => {
        impl<'runtime, S, const CAPACITY: usize> $state<'runtime, S, CAPACITY>
        where
            S: SchedulerRunInterruptStorage,
        {
            pub const fn hci_axis(&self) -> LegacyConnectablePeripheralFirstHciAxis {
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
                LegacyConnectablePeripheralFirstHciResponseWait,
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
            ) -> LegacyConnectablePeripheralFirstHciResponsePublication<Self> {
                map_order_publication(self.order.try_publish_response(controller), |order| Self {
                    order,
                })
            }
        }
    };
}

impl_hci_io!(LegacyConnectablePeripheralFirstHciRunner);
impl_hci_io!(LegacyConnectablePeripheralFirstHciRecovered);
impl_hci_io!(LegacyConnectablePeripheralFirstHciRetry);
impl_hci_io!(LegacyConnectablePeripheralFirstHciRunning);

pub(super) fn map_order_publication<'runtime, Owner, State>(
    publication: LegacyConnectablePeripheralFirstHciOrderPublication<'runtime, Owner>,
    map: impl FnOnce(LegacyConnectablePeripheralFirstHciOrder<'runtime, Owner>) -> State,
) -> LegacyConnectablePeripheralFirstHciResponsePublication<State> {
    match publication {
        LegacyConnectablePeripheralFirstHciOrderPublication::CommandReady(order) => {
            LegacyConnectablePeripheralFirstHciResponsePublication::CommandReady(map(order))
        }
        LegacyConnectablePeripheralFirstHciOrderPublication::Published(order) => {
            LegacyConnectablePeripheralFirstHciResponsePublication::Published(map(order))
        }
        LegacyConnectablePeripheralFirstHciOrderPublication::Pending(order) => {
            LegacyConnectablePeripheralFirstHciResponsePublication::Pending(map(order))
        }
        LegacyConnectablePeripheralFirstHciOrderPublication::EndpointMismatch(order) => {
            LegacyConnectablePeripheralFirstHciResponsePublication::EndpointMismatch(map(order))
        }
        LegacyConnectablePeripheralFirstHciOrderPublication::Fault { order, error } => {
            LegacyConnectablePeripheralFirstHciResponsePublication::Fault {
                state: map(order),
                error,
            }
        }
    }
}

fn begin_with_order<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    accepted: crate::le::advertising::LegacyConnectableAdvertisingAwaitingPeripheralStart<
        'runtime,
        S,
        CAPACITY,
    >,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    begin_step(
        order,
        LegacyConnectablePeripheralFirstRunner::begin(accepted),
    )
}

fn begin_step<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: LegacyConnectablePeripheralFirstBeginStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        LegacyConnectablePeripheralFirstBeginStep::WaitControllerTime(runner) => {
            wait_current(order, runner)
        }
        LegacyConnectablePeripheralFirstBeginStep::FailStop(failure) => {
            fail_stop(order, ControlFlow::Break(failure))
        }
    }
}

fn wait_current<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    runner: LegacyConnectablePeripheralFirstRunner<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    ControlFlow::Continue(
        LegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(
            LegacyConnectablePeripheralFirstHciRunner {
                order: order.map_owner(|()| LegacyConnectablePeripheralFirstWait::Current(runner)),
            },
        ),
    )
}

fn preparation_step<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: LegacyConnectablePeripheralFirstPreparationStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        LegacyConnectablePeripheralFirstPreparationStep::WaitControllerTime(pending) => {
            ControlFlow::Continue(
                LegacyConnectablePeripheralFirstHciProgress::WaitControllerTime(
                    LegacyConnectablePeripheralFirstHciRunner {
                        order: order.map_owner(|()| {
                            LegacyConnectablePeripheralFirstWait::Preparation(pending)
                        }),
                    },
                ),
            )
        }
        LegacyConnectablePeripheralFirstPreparationStep::Prepared(prepared) => {
            publish_prepared(order, prepared)
        }
        LegacyConnectablePeripheralFirstPreparationStep::Recovered(recovered) => {
            ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Recovered(
                LegacyConnectablePeripheralFirstHciRecovered {
                    order: order.map_owner(|()| recovered),
                },
            ))
        }
        LegacyConnectablePeripheralFirstPreparationStep::FailStop(failure) => fail_stop(
            order,
            ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure))),
        ),
    }
}

fn publish_prepared<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    prepared: LegacyConnectablePeripheralFirstPrepared<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
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
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: LegacyConnectablePeripheralFirstPublicationStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        LegacyConnectablePeripheralFirstPublicationStep::HeadPublished(head) => {
            run_step(order, head.start())
        }
        LegacyConnectablePeripheralFirstPublicationStep::Retryable(retry) => {
            retryable(order, retry)
        }
    }
}

fn retry_step<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: LegacyConnectablePeripheralFirstRetryStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        LegacyConnectablePeripheralFirstRetryStep::HeadPublication(step) => {
            publication_step(order, step)
        }
        LegacyConnectablePeripheralFirstRetryStep::InterruptStorage(step) => run_step(order, step),
    }
}

fn run_step<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    step: LegacyConnectablePeripheralFirstRunStep<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        LegacyConnectablePeripheralFirstRunStep::Running(running) => {
            ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Running(
                LegacyConnectablePeripheralFirstHciRunning {
                    order: order.map_owner(|()| running),
                },
            ))
        }
        LegacyConnectablePeripheralFirstRunStep::Retryable(retry) => retryable(order, retry),
    }
}

fn retryable<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    retry: BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    ControlFlow::Continue(LegacyConnectablePeripheralFirstHciProgress::Retryable(
        LegacyConnectablePeripheralFirstHciRetry {
            order: order.map_owner(|()| retry),
        },
    ))
}

fn fail_stop<'runtime, S, const CAPACITY: usize>(
    order: LegacyConnectablePeripheralFirstHciOrder<'runtime, ()>,
    failure: LegacyConnectablePeripheralFirstFailureOwner<'runtime, S, CAPACITY>,
) -> LegacyConnectablePeripheralFirstHciStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    ControlFlow::Break(LegacyConnectablePeripheralFirstHciFailStop {
        order: order.map_owner(|()| failure),
    })
}
