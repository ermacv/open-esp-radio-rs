//! Bounded first-event runner for response-capable legacy LE advertising.
//!
//! The accepted HCI Enable remains affine through fresh controller time,
//! response-graph preparation and the single atomic publication suffix. Only
//! an exact scheduler `RUN` result may create the pending Success response.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        AlwaysAwakePostEnableTimeBeginError, AlwaysAwakePostEnableTimeBeginFailure,
        AlwaysAwakePostEnableTimeError, AlwaysAwakePostEnableTimeFailure,
        AlwaysAwakePostEnableTimePending, AlwaysAwakePostEnableTimeStep,
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginError,
        ControllerSchedulerCurrentBeginFailure, ControllerSchedulerCurrentError,
        ControllerSchedulerCurrentFailure, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, ControllerSchedulerNowReady, SchedulerRunInterruptStorage,
        boot::{
            LegacyConnectableAdvertisingSchedulerFailStop,
            LegacyConnectableAdvertisingSchedulerFailStopCause,
            LegacyConnectableAdvertisingSchedulerStartRetry,
            LegacyConnectableAdvertisingSchedulerStartRetryError,
            LegacyConnectableAdvertisingSchedulerStartStep,
            connectable_advertising::{
                LegacyConnectableAdvertisingControllerFailStopCause,
                LegacyConnectableAdvertisingControllerPreparationError,
                LegacyConnectableAdvertisingControllerPreparationFailStop,
                LegacyConnectableAdvertisingControllerPreparationPending,
                LegacyConnectableAdvertisingRollbackInvariantKind,
            },
        },
    },
    le::{
        advertising::{
            LegacyConnectableAdvertisingRadioContinuations,
            connectable::{
                LegacyConnectableAdvertisingSetError,
                active::LegacyConnectableAdvertisingActiveSession,
                completion::LegacyConnectableAdvertisingCompletionRole,
                hci::{
                    LegacyConnectableAdvertisingActiveResponsePending,
                    LegacyConnectableAdvertisingActiveResponsePublication,
                    LegacyConnectableAdvertisingHciActiveSession,
                },
                prepare_legacy_connectable_advertising_set,
            },
        },
        peripheral::PeripheralConnectionRuntimeBeginError,
    },
    scheduler::{
        SchedulerEmptyListMergeError, SchedulerHeadPublicationError, SchedulerReservationError,
        SchedulerSequenceAuthorizationError,
        core::{
            LegacyConnectableAdvertisingEmptySchedulerMergePrepared, SingleItemSchedulerRunning,
        },
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint,
    LeControllerDeferredLegacyConnectableAdvertisingStart, LeControllerEndpointMismatch,
    LeControllerResponsePending,
};

use oer_esp32s31_bluetooth_memory::{
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    LegacyConnectableAdvertisingMemoryGraphPrepareError, LegacyConnectableAdvertisingPduFitError,
};

type ConnectableSchedulerRunning =
    SingleItemSchedulerRunning<LegacyConnectableAdvertisingCompletionRole>;

#[must_use = "retain the accepted Enable until hardware RUN or idle recovery"]
pub(crate) struct LegacyConnectableAdvertisingDeferredStart<'runtime> {
    command: LeControllerDeferredLegacyConnectableAdvertisingStart<'runtime, ()>,
}

impl<'runtime> LegacyConnectableAdvertisingDeferredStart<'runtime> {
    pub(crate) const fn new(
        command: LeControllerDeferredLegacyConnectableAdvertisingStart<'runtime, ()>,
    ) -> Self {
        Self { command }
    }

    fn into_started_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        self.command.map_owner(|()| owner).into_started_response()
    }

    fn into_hardware_failure_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        self.command
            .map_owner(|()| owner)
            .into_hardware_failure_response()
    }
}

#[must_use = "step or retain the exact connectable-advertising first runner"]
pub struct LegacyConnectableAdvertisingFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: LegacyConnectableAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc phases retain the complete affine controller owner"
)]
enum LegacyConnectableAdvertisingFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: LegacyConnectableAdvertisingControllerPreparationPending<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    },
    Prepared {
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    },
}

/// One finite first-event runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum LegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(LegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(LegacyConnectableAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Hardware-running response graph which still owns the ordered HCI Enable.
#[must_use = "create the pending Success response while retaining the running graph"]
pub struct LegacyConnectableAdvertisingFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: ConnectableSchedulerRunning,
}

/// Pending Success response inseparable from the response-capable running graph.
#[must_use = "the HCI response and running response graph must advance together"]
pub struct LegacyConnectableAdvertisingResponsePending<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pending: LegacyConnectableAdvertisingActiveResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn into_response_pending(
        self,
    ) -> LegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        let active = LegacyConnectableAdvertisingActiveSession::new(self.controller, self.running);
        LegacyConnectableAdvertisingResponsePending {
            pending: LegacyConnectableAdvertisingActiveResponsePending::new(
                self.command.into_started_response(active),
            ),
        }
    }
}

/// Result of publishing Success while retaining the running response graph.
#[must_use = "retain the running session or unchanged response transaction"]
pub enum LegacyConnectableAdvertisingResponsePublication<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Published(LegacyConnectableAdvertisingHciActiveSession<'runtime, S, SCHEDULER_CAPACITY>),
    Pending(LegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    EndpointMismatch(LegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        pending: LegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        error: HciChannelError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Borrow the active radio wait while the Success response is backpressured.
    pub fn radio_wait(
        &self,
    ) -> Option<crate::le::advertising::LegacyConnectableAdvertisingActiveWait<'_>> {
        self.pending.radio_wait()
    }

    /// Borrow the exact pending response across an executor wait.
    pub async fn wait_response_capacity<
        M: RawMutex,
        const H2C: usize,
        const C2H: usize,
        const PACKET: usize,
    >(
        &self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> Result<(), LeControllerEndpointMismatch> {
        self.pending.wait_response_capacity(controller).await
    }

    /// Attempt the sole Success publication without separating its graph.
    pub fn try_publish<M: RawMutex, const H2C: usize, const C2H: usize, const PACKET: usize>(
        self,
        controller: &LeControllerCommandEndpoint<'_, M, H2C, C2H, PACKET>,
    ) -> LegacyConnectableAdvertisingResponsePublication<'runtime, S, SCHEDULER_CAPACITY> {
        let Self { pending } = self;
        match pending.try_publish(controller) {
            LegacyConnectableAdvertisingActiveResponsePublication::Published(active) => {
                LegacyConnectableAdvertisingResponsePublication::Published(active)
            }
            LegacyConnectableAdvertisingActiveResponsePublication::Pending(pending) => {
                LegacyConnectableAdvertisingResponsePublication::Pending(Self { pending })
            }
            LegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(pending) => {
                LegacyConnectableAdvertisingResponsePublication::EndpointMismatch(Self { pending })
            }
            LegacyConnectableAdvertisingActiveResponsePublication::Fault { pending, error } => {
                LegacyConnectableAdvertisingResponsePublication::Fault {
                    pending: Self { pending },
                    error,
                }
            }
        }
    }

    /// Advance radio completion even when the initial Success cannot be queued.
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
        Unrelated: FnOnce(
            Context,
            Self,
            crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved,
        ) -> R,
        NoConnection: FnOnce(
            Context,
            crate::le::advertising::LegacyConnectableAdvertisingNoConnectionResponsePending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            crate::le::advertising::LegacyConnectableAdvertisingConnectionAcceptedResponsePending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        FailStop: FnOnce(
            Context,
            crate::le::advertising::LegacyConnectableAdvertisingActivePendingFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    {
        let (continuing, waiting, unrelated, no_connection, connection_accepted, fail_stop) =
            continuations.into_parts();
        let Self { pending } = self;
        pending.step_radio_with(
            context,
            LegacyConnectableAdvertisingRadioContinuations::new(
                |context, pending| continuing(context, Self { pending }),
                |context, pending| waiting(context, Self { pending }),
                |context, pending, observed| unrelated(context, Self { pending }, observed),
                no_connection,
                connection_accepted,
                fail_stop,
            ),
        )
    }
}

/// Invalid portable configuration rejected before any hardware publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingConfigurationError {
    Role,
    AdvertisingData,
    ScanResponseData,
    Channels,
    Interval,
    MultiplePrimaryChannels { selected: usize },
}

/// Ordinary preparation reason returned only after both runtimes became idle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingFirstRunnerRecoveredError {
    Configuration(LegacyConnectableAdvertisingConfigurationError),
    GenerationExhausted,
    PduFit(LegacyConnectableAdvertisingPduFitError),
    AdvertisingEventActive,
    PeripheralEventActive(PeripheralConnectionRuntimeBeginError),
    MemoryPreparation(LegacyConnectableAdvertisingMemoryGraphPrepareError),
    TimingWindow,
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    EventFields(LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
    EmptyList(SchedulerEmptyListMergeError),
}

/// Idle Controller and HCI order recovered after an ordinary rejection.
#[must_use = "convert the recovered owner to Hardware Failure or retain it"]
pub struct LegacyConnectableAdvertisingFirstRunnerRecovered<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
    controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: LegacyConnectableAdvertisingFirstRunnerRecoveredError,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingFirstRunnerRecovered<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> LegacyConnectableAdvertisingFirstRunnerRecoveredError {
        self.error
    }

    pub fn into_hardware_failure_response(
        self,
    ) -> crate::controller::ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        crate::controller::ControllerIdleResponsePending::new(
            self.command.into_hardware_failure_response(self.controller),
        )
    }
}

/// Finite pre-publication reason for retrying the unchanged owner.
pub enum LegacyConnectableAdvertisingFirstRunnerRetryCause<'a, E> {
    HeadPublication(SchedulerHeadPublicationError),
    SchedulerInterrupts(&'a E),
}

/// Retryable state which has not crossed the first irreversible publication.
#[must_use = "inspect and retry or retain the unchanged pre-publication owner"]
pub struct LegacyConnectableAdvertisingFirstRunnerRetry<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
    retry:
        LegacyConnectableAdvertisingSchedulerStartRetry<'runtime, S, S::Error, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cause(&self) -> LegacyConnectableAdvertisingFirstRunnerRetryCause<'_, S::Error> {
        match self.retry.error() {
            LegacyConnectableAdvertisingSchedulerStartRetryError::Head(error) => {
                LegacyConnectableAdvertisingFirstRunnerRetryCause::HeadPublication(*error)
            }
            LegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(error) => {
                LegacyConnectableAdvertisingFirstRunnerRetryCause::SchedulerInterrupts(error)
            }
        }
    }

    pub fn retry(self) -> LegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        let (controller, merged, _) = self.retry.into_parts();
        LegacyConnectableAdvertisingFirstRunner::from_phase(
            LegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                command: self.command,
                controller,
                merged,
            },
        )
    }
}

/// Lossless rollback invariant which failed before publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingRollbackFailStopCause {
    CancellationOwnership,
    RuntimeRestore,
}

/// Permanent connectable preparation fault without access to private phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingPreparationFailStopCause {
    RuntimeOwnership,
    Rollback(LegacyConnectableAdvertisingRollbackFailStopCause),
    ControllerTime {
        error: crate::scheduler::ControllerTimeAcquisitionError,
        rollback: Option<LegacyConnectableAdvertisingRollbackFailStopCause>,
    },
    PhaseOwnership,
}

/// Irreversible atomic-publication suffix which failed closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingAtomicStartFailStopCause {
    ReceivePublication,
    SchedulerHead,
    SchedulerRun,
}

/// Permanent fault classification without access to the sealed owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingFirstRunnerFailStopCause {
    ColdBegin(AlwaysAwakePostEnableTimeBeginError),
    ColdRecheck(AlwaysAwakePostEnableTimeError),
    WarmBegin(ControllerSchedulerCurrentBeginError),
    WarmRecheck(ControllerSchedulerCurrentError),
    Preparation(LegacyConnectableAdvertisingPreparationFailStopCause),
    AtomicStart(LegacyConnectableAdvertisingAtomicStartFailStopCause),
}

enum LegacyConnectableAdvertisingFirstRunnerFailStopState<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    ColdBegin {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: LegacyConnectableAdvertisingControllerPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    },
    AtomicStart {
        _command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: LegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    },
}

/// Sealed permanent owner. No API can relabel it as an idle Controller.
#[must_use = "the permanently faulted controller and graph must remain fail-stop owned"]
pub struct LegacyConnectableAdvertisingFirstRunnerFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause,
    _state: LegacyConnectableAdvertisingFirstRunnerFailStopState<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &LegacyConnectableAdvertisingFirstRunnerFailStopCause {
        &self.cause
    }
}

/// Exact failure separated by whether idle reuse is actually proven.
#[must_use = "recover HCI order, retry pre-publication, or retain sealed fail-stop ownership"]
pub enum LegacyConnectableAdvertisingFirstRunnerFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Recovered(LegacyConnectableAdvertisingFirstRunnerRecovered<'runtime, S, SCHEDULER_CAPACITY>),
    RetryablePrePublication(
        LegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    FailStop(LegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_phase(
        phase: LegacyConnectableAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact HCI and controller owners"
    )]
    pub(crate) fn begin(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: LeControllerDeferredLegacyConnectableAdvertisingStart<'runtime, ()>,
    ) -> Result<Self, LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>>
    {
        Self::begin_command(
            controller,
            LegacyConnectableAdvertisingDeferredStart::new(command),
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact HCI and controller owners"
    )]
    fn begin_command(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
    ) -> Result<Self, LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>>
    {
        match controller.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    LegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent { command, pending },
                )),
                Err(failure) => {
                    let error = failure.error();
                    Err(Self::warm_begin_fail_stop(command, failure, error))
                }
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(
                    LegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent { command, pending },
                )),
                Err(failure) => {
                    let error = failure.error();
                    Err(Self::cold_begin_fail_stop(command, failure, error))
                }
            },
        }
    }

    pub fn step(
        self,
    ) -> LegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            LegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(AlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        LegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                LegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    }
                    Ok(AlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        LegacyConnectableAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            LegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => {
                        let error = failure.error();
                        LegacyConnectableAdvertisingFirstRunnerStep::Failed(
                            Self::cold_recheck_fail_stop(command, failure, error),
                        )
                    }
                }
            }
            LegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                        LegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                LegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    }
                    Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                        LegacyConnectableAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            LegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current,
                            },
                        ))
                    }
                    Err(failure) => {
                        let error = failure.error();
                        LegacyConnectableAdvertisingFirstRunnerStep::Failed(
                            Self::warm_recheck_fail_stop(command, failure, error),
                        )
                    }
                }
            }
            LegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady { command, current } => {
                let definition =
                    match prepare_legacy_connectable_advertising_set(command.command.request()) {
                        Ok(definition) => definition,
                        Err(error) => {
                            return Self::recovered(
                            command,
                            current.into_retained_epoch().into_task_service(),
                            LegacyConnectableAdvertisingFirstRunnerRecoveredError::Configuration(
                                Self::configuration_error(error),
                            ),
                        );
                        }
                    };
                current.begin_legacy_connectable_advertising_first_event_with(
                    definition,
                    command,
                    |command, pending| {
                        LegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                LegacyConnectableAdvertisingFirstRunnerPhase::Preparation {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    },
                    |command, current, error| {
                        Self::recovered(
                            command,
                            current.into_retained_epoch().into_task_service(),
                            Self::preparation_error(error),
                        )
                    },
                    |command, failure| Self::preparation_fail_stop(command, failure),
                )
            }
            LegacyConnectableAdvertisingFirstRunnerPhase::Preparation { command, pending } => {
                pending.recheck_with(
                    command,
                    |command, pending| {
                        LegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                LegacyConnectableAdvertisingFirstRunnerPhase::Preparation {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    },
                    |command, controller, merged| {
                        LegacyConnectableAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            LegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                                command,
                                controller: controller.into_task_service(),
                                merged,
                            },
                        ))
                    },
                    |command, controller, error| {
                        Self::recovered(
                            command,
                            controller.into_task_service(),
                            Self::preparation_error(error),
                        )
                    },
                    |command, failure| Self::preparation_fail_stop(command, failure),
                )
            }
            LegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                command,
                controller,
                merged,
            } => match controller.start_legacy_connectable_advertising_scheduler(merged) {
                LegacyConnectableAdvertisingSchedulerStartStep::Running {
                    controller,
                    running,
                } => LegacyConnectableAdvertisingFirstRunnerStep::Running(
                    LegacyConnectableAdvertisingFirstRunning {
                        command,
                        controller,
                        running,
                    },
                ),
                LegacyConnectableAdvertisingSchedulerStartStep::Retryable { failure } => {
                    LegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        LegacyConnectableAdvertisingFirstRunnerFailure::RetryablePrePublication(
                            LegacyConnectableAdvertisingFirstRunnerRetry {
                                command,
                                retry: failure,
                            },
                        ),
                    )
                }
                LegacyConnectableAdvertisingSchedulerStartStep::FailStop(failure) => {
                    let cause = failure.cause();
                    LegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
                            LegacyConnectableAdvertisingFirstRunnerFailStop {
                                cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::AtomicStart(
                                    Self::atomic_fail_stop_cause(cause),
                                ),
                                _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::AtomicStart {
                                    _command: command,
                                    _failure: failure,
                                },
                            },
                        ),
                    )
                }
            },
        }
    }

    fn configuration_error(
        error: LegacyConnectableAdvertisingSetError,
    ) -> LegacyConnectableAdvertisingConfigurationError {
        match error {
            LegacyConnectableAdvertisingSetError::Role => {
                LegacyConnectableAdvertisingConfigurationError::Role
            }
            LegacyConnectableAdvertisingSetError::AdvertisingData(_) => {
                LegacyConnectableAdvertisingConfigurationError::AdvertisingData
            }
            LegacyConnectableAdvertisingSetError::ScanResponseData(_) => {
                LegacyConnectableAdvertisingConfigurationError::ScanResponseData
            }
            LegacyConnectableAdvertisingSetError::Channels(_) => {
                LegacyConnectableAdvertisingConfigurationError::Channels
            }
            LegacyConnectableAdvertisingSetError::Interval(_) => {
                LegacyConnectableAdvertisingConfigurationError::Interval
            }
            LegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected } => {
                LegacyConnectableAdvertisingConfigurationError::MultiplePrimaryChannels { selected }
            }
        }
    }

    fn preparation_error(
        error: LegacyConnectableAdvertisingControllerPreparationError,
    ) -> LegacyConnectableAdvertisingFirstRunnerRecoveredError {
        match error {
            LegacyConnectableAdvertisingControllerPreparationError::GenerationExhausted => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::GenerationExhausted
            }
            LegacyConnectableAdvertisingControllerPreparationError::PduFit(error) => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::PduFit(error)
            }
            LegacyConnectableAdvertisingControllerPreparationError::AdvertisingEventActive => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::AdvertisingEventActive
            }
            LegacyConnectableAdvertisingControllerPreparationError::PeripheralEventActive(error) => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::PeripheralEventActive(error)
            }
            LegacyConnectableAdvertisingControllerPreparationError::MemoryPreparation(error) => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::MemoryPreparation(error)
            }
            LegacyConnectableAdvertisingControllerPreparationError::TimingWindow => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::TimingWindow
            }
            LegacyConnectableAdvertisingControllerPreparationError::Event(error) => {
                match error {
                    crate::scheduler::core::LegacyConnectableAdvertisingEventPreparationError::Timeline(error) => {
                        LegacyConnectableAdvertisingFirstRunnerRecoveredError::Timeline(error)
                    }
                    crate::scheduler::core::LegacyConnectableAdvertisingEventPreparationError::Sequence(error) => {
                        LegacyConnectableAdvertisingFirstRunnerRecoveredError::Sequence(error)
                    }
                    crate::scheduler::core::LegacyConnectableAdvertisingEventPreparationError::EventFields(error) => {
                        LegacyConnectableAdvertisingFirstRunnerRecoveredError::EventFields(error)
                    }
                }
            }
            LegacyConnectableAdvertisingControllerPreparationError::EmptyList(error) => {
                LegacyConnectableAdvertisingFirstRunnerRecoveredError::EmptyList(error)
            }
        }
    }

    fn rollback_fail_stop_cause(
        cause: LegacyConnectableAdvertisingRollbackInvariantKind,
    ) -> LegacyConnectableAdvertisingRollbackFailStopCause {
        match cause {
            LegacyConnectableAdvertisingRollbackInvariantKind::CancellationOwnership => {
                LegacyConnectableAdvertisingRollbackFailStopCause::CancellationOwnership
            }
            LegacyConnectableAdvertisingRollbackInvariantKind::RuntimeRestore => {
                LegacyConnectableAdvertisingRollbackFailStopCause::RuntimeRestore
            }
        }
    }

    fn preparation_fail_stop_cause(
        cause: LegacyConnectableAdvertisingControllerFailStopCause,
    ) -> LegacyConnectableAdvertisingPreparationFailStopCause {
        match cause {
            LegacyConnectableAdvertisingControllerFailStopCause::RuntimeOwnership => {
                LegacyConnectableAdvertisingPreparationFailStopCause::RuntimeOwnership
            }
            LegacyConnectableAdvertisingControllerFailStopCause::Rollback(cause) => {
                LegacyConnectableAdvertisingPreparationFailStopCause::Rollback(
                    Self::rollback_fail_stop_cause(cause),
                )
            }
            LegacyConnectableAdvertisingControllerFailStopCause::ControllerTime {
                error,
                rollback,
            } => LegacyConnectableAdvertisingPreparationFailStopCause::ControllerTime {
                error,
                rollback: rollback.map(Self::rollback_fail_stop_cause),
            },
            LegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership => {
                LegacyConnectableAdvertisingPreparationFailStopCause::PhaseOwnership
            }
        }
    }

    fn atomic_fail_stop_cause(
        cause: LegacyConnectableAdvertisingSchedulerFailStopCause,
    ) -> LegacyConnectableAdvertisingAtomicStartFailStopCause {
        match cause {
            LegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(_) => {
                LegacyConnectableAdvertisingAtomicStartFailStopCause::ReceivePublication
            }
            LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(_) => {
                LegacyConnectableAdvertisingAtomicStartFailStopCause::SchedulerHead
            }
            LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(_) => {
                LegacyConnectableAdvertisingAtomicStartFailStopCause::SchedulerRun
            }
        }
    }

    fn recovered(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: LegacyConnectableAdvertisingFirstRunnerRecoveredError,
    ) -> LegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyConnectableAdvertisingFirstRunnerStep::Failed(
            LegacyConnectableAdvertisingFirstRunnerFailure::Recovered(
                LegacyConnectableAdvertisingFirstRunnerRecovered {
                    command,
                    controller,
                    error,
                },
            ),
        )
    }

    fn cold_begin_fail_stop(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: AlwaysAwakePostEnableTimeBeginError,
    ) -> LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            LegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::ColdBegin(error),
                _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::ColdBegin {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn warm_begin_fail_stop(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: ControllerSchedulerCurrentBeginError,
    ) -> LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            LegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::WarmBegin(error),
                _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::WarmBegin {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn cold_recheck_fail_stop(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: AlwaysAwakePostEnableTimeError,
    ) -> LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            LegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::ColdRecheck(error),
                _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::ColdRecheck {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn warm_recheck_fail_stop(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: ControllerSchedulerCurrentError,
    ) -> LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            LegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::WarmRecheck(error),
                _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::WarmRecheck {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn preparation_fail_stop(
        command: LegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: LegacyConnectableAdvertisingControllerPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> LegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let cause = failure.cause();
        LegacyConnectableAdvertisingFirstRunnerStep::Failed(
            LegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
                LegacyConnectableAdvertisingFirstRunnerFailStop {
                    cause: LegacyConnectableAdvertisingFirstRunnerFailStopCause::Preparation(
                        Self::preparation_fail_stop_cause(cause),
                    ),
                    _state: LegacyConnectableAdvertisingFirstRunnerFailStopState::Preparation {
                        _command: command,
                        _failure: failure,
                    },
                },
            ),
        )
    }
}
