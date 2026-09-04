//! Bounded first-event runner for response-capable legacy LE advertising.
//!
//! The accepted HCI Enable remains affine through fresh controller time,
//! response-graph preparation and the single atomic publication suffix. Only
//! an exact scheduler `RUN` result may create the pending Success response.

#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::RawMutex;
use open_esp_radio_bluetooth_hci::{
    HciChannelError, LeControllerCommandEndpoint,
    LeControllerDeferredLegacyConnectableAdvertisingStart, LeControllerEndpointMismatch,
    LeControllerResponsePending,
};
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    BluetoothLegacyConnectableAdvertisingPduFitError,
};

use crate::{
    BluetoothAlwaysAwakePostEnableTimeBeginError, BluetoothAlwaysAwakePostEnableTimeBeginFailure,
    BluetoothAlwaysAwakePostEnableTimeError, BluetoothAlwaysAwakePostEnableTimeFailure,
    BluetoothAlwaysAwakePostEnableTimePending, BluetoothAlwaysAwakePostEnableTimeStep,
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginError,
    BluetoothControllerSchedulerCurrentBeginFailure, BluetoothControllerSchedulerCurrentError,
    BluetoothControllerSchedulerCurrentFailure, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothControllerSchedulerNowReady,
    BluetoothLegacyConnectableAdvertisingRadioContinuations,
    BluetoothPeripheralConnectionRuntimeBeginError, BluetoothSchedulerEmptyListMergeError,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerReservationError,
    BluetoothSchedulerRunInterruptStorage, BluetoothSchedulerSequenceAuthorizationError,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingSetError, prepare_legacy_connectable_advertising_set,
    },
    controller_start::connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingControllerFailStopCause,
        BluetoothLegacyConnectableAdvertisingControllerPreparationError,
        BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop,
        BluetoothLegacyConnectableAdvertisingControllerPreparationPending,
        BluetoothLegacyConnectableAdvertisingRollbackInvariantKind,
    },
    controller_start::{
        BluetoothLegacyConnectableAdvertisingSchedulerFailStop,
        BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause,
        BluetoothLegacyConnectableAdvertisingSchedulerStartRetry,
        BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError,
        BluetoothLegacyConnectableAdvertisingSchedulerStartStep,
    },
    legacy_connectable_advertising_active::BluetoothLegacyConnectableAdvertisingActiveSession,
    legacy_connectable_advertising_completion::BluetoothLegacyConnectableAdvertisingCompletionRole,
    legacy_connectable_advertising_hci::{
        BluetoothLegacyConnectableAdvertisingActiveResponsePending,
        BluetoothLegacyConnectableAdvertisingActiveResponsePublication,
        BluetoothLegacyConnectableAdvertisingHciActiveSession,
    },
    scheduler::{
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothSingleItemSchedulerRunning,
    },
};

type ConnectableSchedulerRunning =
    BluetoothSingleItemSchedulerRunning<BluetoothLegacyConnectableAdvertisingCompletionRole>;

#[must_use = "retain the accepted Enable until hardware RUN or idle recovery"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime> {
    command: LeControllerDeferredLegacyConnectableAdvertisingStart<'runtime, ()>,
}

impl<'runtime> BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime> {
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
pub struct BluetoothLegacyConnectableAdvertisingFirstRunner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothLegacyConnectableAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc phases retain the complete affine controller owner"
)]
enum BluetoothLegacyConnectableAdvertisingFirstRunnerPhase<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        pending: BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    },
    Prepared {
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
    },
}

/// One finite first-event runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum BluetoothLegacyConnectableAdvertisingFirstRunnerStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(
        BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Continue(BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(BluetoothLegacyConnectableAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

/// Hardware-running response graph which still owns the ordered HCI Enable.
#[must_use = "create the pending Success response while retaining the running graph"]
pub struct BluetoothLegacyConnectableAdvertisingFirstRunning<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: ConnectableSchedulerRunning,
}

/// Pending Success response inseparable from the response-capable running graph.
#[must_use = "the HCI response and running response graph must advance together"]
pub struct BluetoothLegacyConnectableAdvertisingResponsePending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending:
        BluetoothLegacyConnectableAdvertisingActiveResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn into_response_pending(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        let active =
            BluetoothLegacyConnectableAdvertisingActiveSession::new(self.controller, self.running);
        BluetoothLegacyConnectableAdvertisingResponsePending {
            pending: BluetoothLegacyConnectableAdvertisingActiveResponsePending::new(
                self.command.into_started_response(active),
            ),
        }
    }
}

/// Result of publishing Success while retaining the running response graph.
#[must_use = "retain the running session or unchanged response transaction"]
pub enum BluetoothLegacyConnectableAdvertisingResponsePublication<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Published(
        BluetoothLegacyConnectableAdvertisingHciActiveSession<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Pending(BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    EndpointMismatch(
        BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Fault {
        pending:
            BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        error: HciChannelError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingResponsePending<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Borrow the active radio wait while the Success response is backpressured.
    pub fn radio_wait(&self) -> Option<crate::BluetoothLegacyConnectableAdvertisingActiveWait<'_>> {
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
    ) -> BluetoothLegacyConnectableAdvertisingResponsePublication<'runtime, S, SCHEDULER_CAPACITY>
    {
        let Self { pending } = self;
        match pending.try_publish(controller) {
            BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Published(active) => {
                BluetoothLegacyConnectableAdvertisingResponsePublication::Published(active)
            }
            BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Pending(pending) => {
                BluetoothLegacyConnectableAdvertisingResponsePublication::Pending(Self { pending })
            }
            BluetoothLegacyConnectableAdvertisingActiveResponsePublication::EndpointMismatch(
                pending,
            ) => BluetoothLegacyConnectableAdvertisingResponsePublication::EndpointMismatch(Self {
                pending,
            }),
            BluetoothLegacyConnectableAdvertisingActiveResponsePublication::Fault {
                pending,
                error,
            } => BluetoothLegacyConnectableAdvertisingResponsePublication::Fault {
                pending: Self { pending },
                error,
            },
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
        Unrelated:
            FnOnce(Context, Self, crate::BluetoothSchedulerFinishedHardwareListObserved) -> R,
        NoConnection: FnOnce(
            Context,
            crate::BluetoothLegacyConnectableAdvertisingNoConnectionResponsePending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            crate::BluetoothLegacyConnectableAdvertisingConnectionAcceptedResponsePending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        FailStop: FnOnce(
            Context,
            crate::BluetoothLegacyConnectableAdvertisingActivePendingFailStop<
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
            BluetoothLegacyConnectableAdvertisingRadioContinuations::new(
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
pub enum BluetoothLegacyConnectableAdvertisingConfigurationError {
    Role,
    AdvertisingData,
    ScanResponseData,
    Channels,
    Interval,
    MultiplePrimaryChannels { selected: usize },
}

/// Ordinary preparation reason returned only after both runtimes became idle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError {
    Configuration(BluetoothLegacyConnectableAdvertisingConfigurationError),
    GenerationExhausted,
    PduFit(BluetoothLegacyConnectableAdvertisingPduFitError),
    AdvertisingEventActive,
    PeripheralEventActive(BluetoothPeripheralConnectionRuntimeBeginError),
    MemoryPreparation(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError),
    TimingWindow,
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    EventFields(BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
    EmptyList(BluetoothSchedulerEmptyListMergeError),
}

/// Idle Controller and HCI order recovered after an ordinary rejection.
#[must_use = "convert the recovered owner to Hardware Failure or retain it"]
pub struct BluetoothLegacyConnectableAdvertisingFirstRunnerRecovered<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
    controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingFirstRunnerRecovered<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError {
        self.error
    }

    pub fn into_hardware_failure_response(
        self,
    ) -> crate::BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        crate::BluetoothControllerIdleResponsePending::new(
            self.command.into_hardware_failure_response(self.controller),
        )
    }
}

/// Finite pre-publication reason for retrying the unchanged owner.
pub enum BluetoothLegacyConnectableAdvertisingFirstRunnerRetryCause<'a, E> {
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    SchedulerInterrupts(&'a E),
}

/// Retryable state which has not crossed the first irreversible publication.
#[must_use = "inspect and retry or retain the unchanged pre-publication owner"]
pub struct BluetoothLegacyConnectableAdvertisingFirstRunnerRetry<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
    retry: BluetoothLegacyConnectableAdvertisingSchedulerStartRetry<
        'runtime,
        S,
        S::Error,
        SCHEDULER_CAPACITY,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cause(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerRetryCause<'_, S::Error> {
        match self.retry.error() {
            BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Head(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRetryCause::HeadPublication(*error)
            }
            BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRetryCause::SchedulerInterrupts(
                    error,
                )
            }
        }
    }

    pub fn retry(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        let (controller, merged, _) = self.retry.into_parts();
        BluetoothLegacyConnectableAdvertisingFirstRunner::from_phase(
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                command: self.command,
                controller,
                merged,
            },
        )
    }
}

/// Lossless rollback invariant which failed before publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingRollbackFailStopCause {
    CancellationOwnership,
    RuntimeRestore,
}

/// Permanent connectable preparation fault without access to private phases.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingPreparationFailStopCause {
    RuntimeOwnership,
    Rollback(BluetoothLegacyConnectableAdvertisingRollbackFailStopCause),
    ControllerTime {
        error: crate::BluetoothControllerTimeAcquisitionError,
        rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackFailStopCause>,
    },
    PhaseOwnership,
}

/// Irreversible atomic-publication suffix which failed closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause {
    ReceivePublication,
    SchedulerHead,
    SchedulerRun,
}

/// Permanent fault classification without access to the sealed owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause {
    ColdBegin(BluetoothAlwaysAwakePostEnableTimeBeginError),
    ColdRecheck(BluetoothAlwaysAwakePostEnableTimeError),
    WarmBegin(BluetoothControllerSchedulerCurrentBeginError),
    WarmRecheck(BluetoothControllerSchedulerCurrentError),
    Preparation(BluetoothLegacyConnectableAdvertisingPreparationFailStopCause),
    AtomicStart(BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause),
}

enum BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdBegin {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure: BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    },
    AtomicStart {
        _command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        _failure:
            BluetoothLegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    },
}

/// Sealed permanent owner. No API can relabel it as an idle Controller.
#[must_use = "the permanently faulted controller and graph must remain fail-stop owned"]
pub struct BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause,
    _state: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause {
        &self.cause
    }
}

/// Exact failure separated by whether idle reuse is actually proven.
#[must_use = "recover HCI order, retry pre-publication, or retain sealed fail-stop ownership"]
pub enum BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Recovered(
        BluetoothLegacyConnectableAdvertisingFirstRunnerRecovered<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    RetryablePrePublication(
        BluetoothLegacyConnectableAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    FailStop(
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_phase(
        phase: BluetoothLegacyConnectableAdvertisingFirstRunnerPhase<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact HCI and controller owners"
    )]
    pub(crate) fn begin(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: LeControllerDeferredLegacyConnectableAdvertisingStart<'runtime, ()>,
    ) -> Result<
        Self,
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        Self::begin_command(
            controller,
            BluetoothLegacyConnectableAdvertisingDeferredStart::new(command),
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact HCI and controller owners"
    )]
    fn begin_command(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
    ) -> Result<
        Self,
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match controller.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent {
                        command,
                        pending,
                    },
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
                    BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent {
                        command,
                        pending,
                    },
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
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent {
                command,
                pending,
            } => match pending.recheck() {
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::ColdCurrent {
                                command,
                                pending,
                            },
                        ),
                    )
                }
                Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Continue(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ),
                    )
                }
                Err(failure) => {
                    let error = failure.error();
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        Self::cold_recheck_fail_stop(command, failure, error),
                    )
                }
            },
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent {
                command,
                pending,
            } => match pending.recheck() {
                Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::WarmCurrent {
                                command,
                                pending,
                            },
                        ),
                    )
                }
                Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Continue(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current,
                            },
                        ),
                    )
                }
                Err(failure) => {
                    let error = failure.error();
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        Self::warm_recheck_fail_stop(command, failure, error),
                    )
                }
            },
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::CurrentReady {
                command,
                current,
            } => {
                let definition = match prepare_legacy_connectable_advertising_set(
                    command.command.request(),
                ) {
                    Ok(definition) => definition,
                    Err(error) => {
                        return Self::recovered(
                            command,
                            current.into_retained_epoch().into_task_service(),
                            BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::Configuration(
                                Self::configuration_error(error),
                            ),
                        );
                    }
                };
                current.begin_legacy_connectable_advertising_first_event_with(
                    definition,
                    command,
                    |command, pending| {
                        BluetoothLegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Preparation {
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
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Preparation {
                command,
                pending,
            } => pending.recheck_with(
                command,
                |command, pending| {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Preparation {
                                command,
                                pending,
                            },
                        ),
                    )
                },
                |command, controller, merged| {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Continue(
                        Self::from_phase(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                                command,
                                controller: controller.into_task_service(),
                                merged,
                            },
                        ),
                    )
                },
                |command, controller, error| {
                    Self::recovered(
                        command,
                        controller.into_task_service(),
                        Self::preparation_error(error),
                    )
                },
                |command, failure| Self::preparation_fail_stop(command, failure),
            ),
            BluetoothLegacyConnectableAdvertisingFirstRunnerPhase::Prepared {
                command,
                controller,
                merged,
            } => match controller.start_legacy_connectable_advertising_scheduler(merged) {
                BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Running {
                    controller,
                    running,
                } => BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Running(
                    BluetoothLegacyConnectableAdvertisingFirstRunning {
                        command,
                        controller,
                        running,
                    },
                ),
                BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Retryable { failure } => {
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::RetryablePrePublication(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerRetry {
                                command,
                                retry: failure,
                            },
                        ),
                    )
                }
                BluetoothLegacyConnectableAdvertisingSchedulerStartStep::FailStop(failure) => {
                    let cause = failure.cause();
                    BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
                            BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                                cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::AtomicStart(
                                    Self::atomic_fail_stop_cause(cause),
                                ),
                                _state: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::AtomicStart {
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
        error: BluetoothLegacyConnectableAdvertisingSetError,
    ) -> BluetoothLegacyConnectableAdvertisingConfigurationError {
        match error {
            BluetoothLegacyConnectableAdvertisingSetError::Role => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::Role
            }
            BluetoothLegacyConnectableAdvertisingSetError::AdvertisingData(_) => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::AdvertisingData
            }
            BluetoothLegacyConnectableAdvertisingSetError::ScanResponseData(_) => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::ScanResponseData
            }
            BluetoothLegacyConnectableAdvertisingSetError::Channels(_) => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::Channels
            }
            BluetoothLegacyConnectableAdvertisingSetError::Interval(_) => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::Interval
            }
            BluetoothLegacyConnectableAdvertisingSetError::MultiplePrimaryChannels { selected } => {
                BluetoothLegacyConnectableAdvertisingConfigurationError::MultiplePrimaryChannels {
                    selected,
                }
            }
        }
    }

    fn preparation_error(
        error: BluetoothLegacyConnectableAdvertisingControllerPreparationError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError {
        match error {
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::GenerationExhausted => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::GenerationExhausted
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::PduFit(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::PduFit(error)
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::AdvertisingEventActive => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::AdvertisingEventActive
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::PeripheralEventActive(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::PeripheralEventActive(error)
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::MemoryPreparation(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::MemoryPreparation(error)
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::TimingWindow => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::TimingWindow
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::Event(error) => {
                match error {
                    crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(error) => {
                        BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::Timeline(error)
                    }
                    crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(error) => {
                        BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::Sequence(error)
                    }
                    crate::scheduler::BluetoothLegacyConnectableAdvertisingEventPreparationError::EventFields(error) => {
                        BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::EventFields(error)
                    }
                }
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationError::EmptyList(error) => {
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError::EmptyList(error)
            }
        }
    }

    fn rollback_fail_stop_cause(
        cause: BluetoothLegacyConnectableAdvertisingRollbackInvariantKind,
    ) -> BluetoothLegacyConnectableAdvertisingRollbackFailStopCause {
        match cause {
            BluetoothLegacyConnectableAdvertisingRollbackInvariantKind::CancellationOwnership => {
                BluetoothLegacyConnectableAdvertisingRollbackFailStopCause::CancellationOwnership
            }
            BluetoothLegacyConnectableAdvertisingRollbackInvariantKind::RuntimeRestore => {
                BluetoothLegacyConnectableAdvertisingRollbackFailStopCause::RuntimeRestore
            }
        }
    }

    fn preparation_fail_stop_cause(
        cause: BluetoothLegacyConnectableAdvertisingControllerFailStopCause,
    ) -> BluetoothLegacyConnectableAdvertisingPreparationFailStopCause {
        match cause {
            BluetoothLegacyConnectableAdvertisingControllerFailStopCause::RuntimeOwnership => {
                BluetoothLegacyConnectableAdvertisingPreparationFailStopCause::RuntimeOwnership
            }
            BluetoothLegacyConnectableAdvertisingControllerFailStopCause::Rollback(cause) => {
                BluetoothLegacyConnectableAdvertisingPreparationFailStopCause::Rollback(
                    Self::rollback_fail_stop_cause(cause),
                )
            }
            BluetoothLegacyConnectableAdvertisingControllerFailStopCause::ControllerTime {
                error,
                rollback,
            } => BluetoothLegacyConnectableAdvertisingPreparationFailStopCause::ControllerTime {
                error,
                rollback: rollback.map(Self::rollback_fail_stop_cause),
            },
            BluetoothLegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership => {
                BluetoothLegacyConnectableAdvertisingPreparationFailStopCause::PhaseOwnership
            }
        }
    }

    fn atomic_fail_stop_cause(
        cause: BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause,
    ) -> BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause {
        match cause {
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(_) => {
                BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause::ReceivePublication
            }
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(_) => {
                BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause::SchedulerHead
            }
            BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(_) => {
                BluetoothLegacyConnectableAdvertisingAtomicStartFailStopCause::SchedulerRun
            }
        }
    }

    fn recovered(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothLegacyConnectableAdvertisingFirstRunnerRecoveredError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::Recovered(
                BluetoothLegacyConnectableAdvertisingFirstRunnerRecovered {
                    command,
                    controller,
                    error,
                },
            ),
        )
    }

    fn cold_begin_fail_stop(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothAlwaysAwakePostEnableTimeBeginError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::ColdBegin(
                    error,
                ),
                _state: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::ColdBegin {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn warm_begin_fail_stop(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothControllerSchedulerCurrentBeginError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::WarmBegin(
                    error,
                ),
                _state: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::WarmBegin {
                    _command: command,
                    _failure: failure,
                },
            },
        )
    }

    fn cold_recheck_fail_stop(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothAlwaysAwakePostEnableTimeError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::ColdRecheck(
                    error,
                ),
                _state:
                    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::ColdRecheck {
                        _command: command,
                        _failure: failure,
                    },
            },
        )
    }

    fn warm_recheck_fail_stop(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothControllerSchedulerCurrentError,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
    {
        BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                cause: BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::WarmRecheck(
                    error,
                ),
                _state:
                    BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::WarmRecheck {
                        _command: command,
                        _failure: failure,
                    },
            },
        )
    }

    fn preparation_fail_stop(
        command: BluetoothLegacyConnectableAdvertisingDeferredStart<'runtime>,
        failure: BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> BluetoothLegacyConnectableAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let cause = failure.cause();
        BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(
            BluetoothLegacyConnectableAdvertisingFirstRunnerFailure::FailStop(
                BluetoothLegacyConnectableAdvertisingFirstRunnerFailStop {
                    cause:
                        BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopCause::Preparation(
                            Self::preparation_fail_stop_cause(cause),
                        ),
                    _state:
                        BluetoothLegacyConnectableAdvertisingFirstRunnerFailStopState::Preparation {
                            _command: command,
                            _failure: failure,
                        },
                },
            ),
        )
    }
}
