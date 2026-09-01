//! Bounded first-event runner for legacy LE Direct Test Mode.
//!
//! The runner owns one semantic HCI command together with every affine
//! Controller typestate needed to reach the first scheduler `RUN`. Each
//! [`BluetoothDtmFirstRunner::step`] call performs exactly one finite lower
//! transition. In particular, a controller-time `Waiting` result is returned
//! to the executor instead of being polled in a hidden loop.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_hci::{
    LeControllerClassifiedCommand, LeControllerDeferredReceiverStart,
    LeControllerDeferredTransmitterStart, LeControllerResponsePending,
};

use crate::scheduler::{
    BluetoothDtmFirstPreparationCompletionClass, classify_dtm_first_preparation_completion,
};

use crate::{
    BluetoothAlwaysAwakePostEnableTimeBeginFailure, BluetoothAlwaysAwakePostEnableTimeError,
    BluetoothAlwaysAwakePostEnableTimeFailure, BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep,
    BluetoothAlwaysAwakePostEnableTimePending, BluetoothAlwaysAwakePostEnableTimeStep,
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginFailure,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentFailure,
    BluetoothControllerSchedulerCurrentPending, BluetoothControllerSchedulerCurrentStep,
    BluetoothControllerSchedulerEpochRetained, BluetoothControllerSchedulerNowReady,
    BluetoothControllerTimeOrphanDrainStep, BluetoothDtmControllerEventPreparationError,
    BluetoothDtmControllerInitialPreparationFailure, BluetoothDtmControllerPreparationOutcome,
    BluetoothDtmControllerPreparationPending, BluetoothDtmControllerPreparationStep,
    BluetoothDtmControllerPreparationTerminal, BluetoothDtmControllerRxPreparationFailure,
    BluetoothDtmControllerTxPreparationFailure, BluetoothDtmEmptySchedulerMergePrepared,
    BluetoothDtmInitialSchedulerItemPhase, BluetoothDtmReceiverEvent,
    BluetoothDtmSchedulerHeadPublished, BluetoothDtmSchedulerRunning, BluetoothDtmSessionIdle,
    BluetoothDtmTransmitterEvent, BluetoothSchedulerHeadPublicationError,
    BluetoothSchedulerRunInterruptStorage,
};

enum BluetoothDtmDeferredStartKind<'runtime> {
    Transmitter(LeControllerDeferredTransmitterStart<'runtime, ()>),
    Receiver(LeControllerDeferredReceiverStart<'runtime, ()>),
}

/// Opaque accepted DTM start retaining its semantic command and HCI order.
///
/// The portable role-specific deferred start is never decomposed through this
/// public API. Chip phases may move the hardware owner independently, while
/// the exact start response remains constructible only after scheduler `RUN`.
#[must_use = "retain the accepted start until hardware starts or is recovered"]
pub struct BluetoothDtmDeferredStart<'runtime> {
    kind: BluetoothDtmDeferredStartKind<'runtime>,
}

impl BluetoothDtmDeferredStart<'_> {
    /// Hardware role selected by the accepted semantic command.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        match self.kind {
            BluetoothDtmDeferredStartKind::Transmitter(_) => crate::BluetoothDtmRole::Transmitter,
            BluetoothDtmDeferredStartKind::Receiver(_) => crate::BluetoothDtmRole::Receiver,
        }
    }
}

impl<'runtime> BluetoothDtmDeferredStart<'runtime> {
    pub(crate) fn transmitter(start: LeControllerDeferredTransmitterStart<'runtime, ()>) -> Self {
        Self {
            kind: BluetoothDtmDeferredStartKind::Transmitter(start),
        }
    }

    pub(crate) fn receiver(start: LeControllerDeferredReceiverStart<'runtime, ()>) -> Self {
        Self {
            kind: BluetoothDtmDeferredStartKind::Receiver(start),
        }
    }

    fn into_started_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        match self.kind {
            BluetoothDtmDeferredStartKind::Transmitter(start) => {
                start.map_owner(|()| owner).into_started_response()
            }
            BluetoothDtmDeferredStartKind::Receiver(start) => {
                start.map_owner(|()| owner).into_started_response()
            }
        }
    }

    fn into_hardware_failure_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        match self.kind {
            BluetoothDtmDeferredStartKind::Transmitter(start) => {
                start.map_owner(|()| owner).into_hardware_failure_response()
            }
            BluetoothDtmDeferredStartKind::Receiver(start) => {
                start.map_owner(|()| owner).into_hardware_failure_response()
            }
        }
    }
}

/// Opaque accepted start paired with its complete powered task owner.
///
/// This aggregate is used by every pre-`RUN` recovery boundary which has
/// regained a task service. It deliberately exposes only the retained role;
/// command order and task ownership cannot be separated by callers.
#[must_use = "retain the accepted start and its task as one recovery owner"]
struct BluetoothDtmFirstTaskOwner<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: BluetoothDtmDeferredStart<'runtime>,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Hardware role retained by this exact recovery owner.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.deferred.role()
    }

    fn into_hardware_failure_response(
        self,
    ) -> crate::BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        crate::BluetoothControllerIdleResponsePending::new(
            self.deferred.into_hardware_failure_response(self.task),
        )
    }
}

/// Neutral cancellation whose complete idle task owner has been recovered.
///
/// This owner proves that shutdown cleanup recovered the idle graph; it carries
/// no HCI completion authority. Dropping an await, Controller shutdown and
/// semantic Host cancellation are deliberately not conflated.
#[must_use = "retain the recovered shutdown owner for teardown"]
pub struct BluetoothDtmFirstCancellationCleanTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    _owner: BluetoothDtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    preparation_error: Option<BluetoothDtmControllerEventPreparationError>,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstCancellationCleanTask<'_, S, SCHEDULER_CAPACITY>
{
    /// Hardware role retained by the neutrally cancelled command.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self._owner.role()
    }

    /// Lower preparation outcome retained when cancellation interrupted that phase.
    pub const fn preparation_error(&self) -> Option<BluetoothDtmControllerEventPreparationError> {
        self.preparation_error
    }
}

/// Neutral cancellation paired with a retained scheduler epoch.
#[must_use = "recover the idle task without manufacturing response authority"]
pub struct BluetoothDtmFirstCancellationEpoch<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: BluetoothDtmDeferredStart<'runtime>,
    _epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstCancellationEpoch<'_, S, SCHEDULER_CAPACITY>
{
    /// Hardware role retained by this exact recovery owner.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.deferred.role()
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstCancellationEpoch<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Discard an unused current sample and recover a neutral cleanup owner.
    pub fn into_clean_task(
        self,
    ) -> BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothDtmFirstCancellationCleanTask {
            _owner: BluetoothDtmFirstTaskOwner {
                deferred: self.deferred,
                task: self._epoch.into_task_service(),
            },
            preparation_error: None,
        }
    }
}

/// Cancelled cold-current request awaiting bounded orphan drain.
///
/// Although the DTM graph itself is still idle, this type does not expose a
/// response edge because the time worker cannot accept the next command yet.
#[must_use = "drain the abandoned cold-current request before completing cancellation"]
pub struct BluetoothDtmFirstColdTimeDrain<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: BluetoothDtmDeferredStart<'runtime>,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded cold-current cancellation drain result.
#[must_use = "retain Waiting or Fault; CleanTask remains neutral shutdown ownership"]
pub enum BluetoothDtmFirstColdTimeDrainStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(BluetoothDtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        drain: BluetoothDtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothAlwaysAwakePostEnableTimeError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform one bounded observation without manufacturing a failure response.
    pub fn step(mut self) -> BluetoothDtmFirstColdTimeDrainStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.task.drain_abandoned_always_awake_post_enable_time() {
            Ok(BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Waiting) => {
                BluetoothDtmFirstColdTimeDrainStep::Waiting(self)
            }
            Ok(
                BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Idle
                | BluetoothAlwaysAwakePostEnableTimeOrphanDrainStep::Drained,
            ) => BluetoothDtmFirstColdTimeDrainStep::CleanTask(
                BluetoothDtmFirstCancellationCleanTask {
                    _owner: BluetoothDtmFirstTaskOwner {
                        deferred: self.deferred,
                        task: self.task,
                    },
                    preparation_error: None,
                },
            ),
            Err(error) => BluetoothDtmFirstColdTimeDrainStep::Fault { drain: self, error },
        }
    }
}

/// Cancelled warm-current request awaiting bounded orphan drain.
#[must_use = "drain the abandoned warm-current request before completing cancellation"]
pub struct BluetoothDtmFirstWarmTimeDrain<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: BluetoothDtmDeferredStart<'runtime>,
    epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded warm-current cancellation drain result.
#[must_use = "retain Waiting or Fault; CleanTask remains neutral shutdown ownership"]
pub enum BluetoothDtmFirstWarmTimeDrainStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(BluetoothDtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        drain: BluetoothDtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothControllerSchedulerCurrentError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform one bounded observation without manufacturing a failure response.
    pub fn step(mut self) -> BluetoothDtmFirstWarmTimeDrainStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.epoch.drain_abandoned_controller_time() {
            Ok(BluetoothControllerTimeOrphanDrainStep::Waiting) => {
                BluetoothDtmFirstWarmTimeDrainStep::Waiting(self)
            }
            Ok(
                BluetoothControllerTimeOrphanDrainStep::Idle
                | BluetoothControllerTimeOrphanDrainStep::Drained,
            ) => BluetoothDtmFirstWarmTimeDrainStep::CleanTask(
                BluetoothDtmFirstCancellationCleanTask {
                    _owner: BluetoothDtmFirstTaskOwner {
                        deferred: self.deferred,
                        task: self.epoch.into_task_service(),
                    },
                    preparation_error: None,
                },
            ),
            Err(error) => BluetoothDtmFirstWarmTimeDrainStep::Fault { drain: self, error },
        }
    }
}

/// Opaque accepted start paired with one lower pre-`RUN` owner.
///
/// Private fields make it impossible to separate portable command order from
/// the lower failure/current owner at a public recovery boundary.
#[must_use = "retain the accepted start and lower owner as one transaction"]
pub struct BluetoothDtmFirstAcceptedFailure<'runtime, Owner> {
    deferred: BluetoothDtmDeferredStart<'runtime>,
    owner: Owner,
}

impl<'runtime, Owner> BluetoothDtmFirstAcceptedFailure<'runtime, Owner> {
    fn new(deferred: BluetoothDtmDeferredStart<'runtime>, owner: Owner) -> Self {
        Self { deferred, owner }
    }

    /// Hardware role retained by this exact failed transition.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.deferred.role()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstAcceptedFailure<
        '_,
        BluetoothAlwaysAwakePostEnableTimeBeginFailure<'_, S, SCHEDULER_CAPACITY>,
    >
{
    /// Exact cold-acquisition rejection retained by this owner.
    pub const fn cold_begin_error(&self) -> crate::BluetoothAlwaysAwakePostEnableTimeBeginError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstAcceptedFailure<
        '_,
        BluetoothAlwaysAwakePostEnableTimeFailure<'_, S, SCHEDULER_CAPACITY>,
    >
{
    /// Exact cold-acquisition fail-stop observation retained by this owner.
    pub const fn cold_recheck_error(&self) -> crate::BluetoothAlwaysAwakePostEnableTimeError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstAcceptedFailure<
        '_,
        BluetoothControllerSchedulerCurrentBeginFailure<'_, S, SCHEDULER_CAPACITY>,
    >
{
    /// Exact warm-acquisition rejection retained by this owner.
    pub const fn warm_begin_error(&self) -> crate::BluetoothControllerSchedulerCurrentBeginError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstAcceptedFailure<
        '_,
        BluetoothControllerSchedulerCurrentFailure<'_, S, SCHEDULER_CAPACITY>,
    >
{
    /// Exact warm-acquisition fail-stop observation retained by this owner.
    pub const fn warm_recheck_error(&self) -> crate::BluetoothControllerSchedulerCurrentError {
        self.owner.error()
    }
}

/// Opaque fail-stop owner for an impossible mismatch after combined intake.
///
/// Task ownership, classification and next-command authority cannot be
/// decomposed through this chip API.
#[must_use = "retain the complete post-intake mismatch owner"]
pub struct BluetoothControllerIdleCommandMismatch<
    'runtime,
    'command,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

impl<'runtime, 'command, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerIdleCommandMismatch<'runtime, 'command, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        command: LeControllerClassifiedCommand<
            'runtime,
            'command,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ) -> Self {
        Self { _command: command }
    }
}

/// Endpoint-checked disposition of one Controller command while both radios are idle.
///
/// Every branch retains the sole task service: a start moves it into the first
/// runner (or that runner's lossless begin failure), while immediate responses
/// and Reset retain their exact affine HCI order.
#[must_use = "start the first event, publish idle Test End, or retain the mismatch"]
pub enum BluetoothControllerIdleCommandRoute<'runtime, 'command, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// A validated RX/TX command entered the sole first-event runner.
    Start(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// A validated non-connectable Enable entered the advertising runner.
    StartLegacyAdvertising(
        crate::BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// A validated passive scanner Enable entered the HCI-composed first runner.
    StartPassiveScanning(
        crate::BluetoothPassiveScanHciFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Initial Controller-time acquisition failed without losing any owner.
    StartFailed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
    /// Advertising current acquisition failed without losing HCI order.
    LegacyAdvertisingStartFailed(
        crate::BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Passive scanner start failed without losing HCI order or lower ownership.
    PassiveScanStartFailed(
        crate::BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Idle Test End became an ordered standard zero-count response.
    ResponsePending(crate::BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// Idle Reset retains its exact command/order until lifecycle completion.
    ResetBarrier(crate::BluetoothControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>),
    /// Defensive fail-stop owner for an impossible post-intake epoch mismatch.
    ///
    /// Classification, task ownership and command authority remain inseparable.
    EndpointMismatch(
        BluetoothControllerIdleCommandMismatch<'runtime, 'command, S, SCHEDULER_CAPACITY>,
    ),
}

/// Bounded first-event Controller runner.
///
/// Every variant retains the sole task service either directly or inside one
/// lower affine state. The private role-specific variants prevent a TX command
/// from being paired with an RX descriptor graph.
#[must_use = "step or explicitly cancel the affine DTM runner"]
pub struct BluetoothDtmFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain complete affine Controller owners"
)]
enum BluetoothDtmFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: BluetoothDtmDeferredStart<'runtime>,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: BluetoothDtmDeferredStart<'runtime>,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: BluetoothDtmDeferredStart<'runtime>,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: BluetoothDtmDeferredStart<'runtime>,
        pending: BluetoothDtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    TransmitterPrepared {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmTransmitterEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    },
    ReceiverPrepared {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothDtmEmptySchedulerMergePrepared<
            BluetoothDtmReceiverEvent,
            BluetoothDtmInitialSchedulerItemPhase,
        >,
    },
    TransmitterHead {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmTransmitterEvent>,
    },
    ReceiverHead {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothDtmSchedulerHeadPublished<BluetoothDtmReceiverEvent>,
    },
}

/// Result of one bounded runner step.
#[must_use = "retain pending ownership or handle the terminal first-event result"]
pub enum BluetoothDtmFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// A self-clearing controller-time latch still needs a later observation.
    WaitControllerTime(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The previous bounded transition completed; another can run immediately.
    Continue(BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact first graph reached hardware scheduler `RUN`.
    Running(BluetoothDtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite transition failed while retaining its exact owner.
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First DTM event admitted to the hardware scheduler.
///
/// The semantic command remains owned until its Command Complete response is
/// durably published. Hardware progress therefore does not depend on HCI
/// Controller-to-Host queue capacity.
#[must_use = "the running graph and pending HCI response authority must remain owned"]
pub struct BluetoothDtmFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    running: BluetoothDtmFirstRunningPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothDtmFirstRunningPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    },
    Receiver {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    },
}

pub(crate) enum BluetoothDtmFirstRunningParts<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Transmitter {
        response: LeControllerResponsePending<
            'runtime,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmTransmitterEvent>,
    },
    Receiver {
        response: LeControllerResponsePending<
            'runtime,
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
        running: BluetoothDtmSchedulerRunning<BluetoothDtmReceiverEvent>,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> BluetoothDtmFirstRunningParts<'runtime, S, SCHEDULER_CAPACITY> {
        match self.running {
            BluetoothDtmFirstRunningPhase::Transmitter {
                command,
                task,
                running,
            } => BluetoothDtmFirstRunningParts::Transmitter {
                response: command.into_started_response(task),
                running,
            },
            BluetoothDtmFirstRunningPhase::Receiver {
                command,
                task,
                running,
            } => BluetoothDtmFirstRunningParts::Receiver {
                response: command.into_started_response(task),
                running,
            },
        }
    }
}

/// Finite reason a pre-`RUN` runner transition may be retried unchanged.
#[must_use = "the retry cause should be inspected before advancing the retained runner"]
pub enum BluetoothDtmFirstRunnerRetryCause<E> {
    /// The CPU-owned graph could not become the scheduler hardware-list head.
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    /// Dynamic scheduler interrupt preparation rejected the published head.
    SchedulerStart(E),
}

/// Opaque retry owner for a role-consistent pre-`RUN` state.
///
/// The private fields prevent safe code from pairing a TX command with an RX
/// graph or from pairing a task service with another published head.
#[must_use = "inspect the cause and retain, retry, or cancel the exact runner"]
pub struct BluetoothDtmFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmFirstRunnerRetryCause<S::Error>,
    role: crate::BluetoothDtmRole,
    runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role of the exact command and graph retained for retry.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.role
    }

    /// Recover the owning cause and the only runner which may retry it.
    pub fn into_parts(
        self,
    ) -> (
        BluetoothDtmFirstRunnerRetryCause<S::Error>,
        BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ) {
        (self.cause, self.runner)
    }
}

/// Opaque cleanup after an initial DTM preparation released its CPU graph.
///
/// The graph remains paired with the exact Controller while an abandoned
/// controller-time request is drained and the runtime idle slot is restored.
#[must_use = "drive cleanup until the exact DTM graph is restored"]
pub struct BluetoothDtmFirstPreparationCleanup<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmFirstPreparationCleanupPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothDtmFirstPreparationCleanupPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Drain {
        command: BluetoothDtmDeferredStart<'runtime>,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    },
    Restore {
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    },
}

/// Restored initial-preparation owner awaiting the chip's closed HCI policy.
///
/// The graph is back in the runtime idle slot, but the retained lower error
/// still decides whether command authority may reopen. Callers cannot separate
/// the task owner from that decision.
#[must_use = "consume the restored owner through its closed completion policy"]
pub struct BluetoothDtmFirstPreparationCleanTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    owner: BluetoothDtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothDtmControllerEventPreparationError,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstPreparationCleanTask<'_, S, SCHEDULER_CAPACITY>
{
    /// Exact preparation rejection retained for diagnostics.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Hardware role retained by the accepted command and restored graph.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.owner.role()
    }
}

/// Opaque restored owner whose preparation failure poisoned an invariant.
///
/// No decomposition or HCI-response edge exists: graph-slot restoration does
/// not make a role, identity, list or worker-ownership disagreement reusable.
#[must_use = "retain the fail-stop Controller owner for reset or teardown"]
pub struct BluetoothDtmFirstPreparationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    _owner: BluetoothDtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    error: BluetoothDtmControllerEventPreparationError,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstPreparationFailStop<'_, S, SCHEDULER_CAPACITY>
{
    /// Exact invariant, identity, list or ownership failure.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.error
    }

    /// Hardware role retained by the accepted command and recovered graph.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self._owner.role()
    }
}

/// Closed completion after an initial preparation restored its idle graph.
#[must_use = "publish the ordered rejection or retain the opaque fail-stop owner"]
pub enum BluetoothDtmFirstPreparationCompletion<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// A finite timing/resource rejection classified as Hardware Failure.
    ResponsePending(crate::BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>),
    /// Restoration succeeded, but the retained failure forbids reuse.
    FailStop(BluetoothDtmFirstPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstPreparationCleanTask<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Apply the chip-owned failure classification without caller-selected status.
    pub fn into_completion(
        self,
    ) -> BluetoothDtmFirstPreparationCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        match classify_dtm_first_preparation_completion(self.error) {
            BluetoothDtmFirstPreparationCompletionClass::HardwareFailure => {
                BluetoothDtmFirstPreparationCompletion::ResponsePending(
                    self.owner.into_hardware_failure_response(),
                )
            }
            BluetoothDtmFirstPreparationCompletionClass::FailStop => {
                BluetoothDtmFirstPreparationCompletion::FailStop(
                    BluetoothDtmFirstPreparationFailStop {
                        _owner: self.owner,
                        error: self.error,
                    },
                )
            }
        }
    }
}

/// One bounded graph-cleanup transition.
#[must_use = "retain cleanup ownership until the Controller can start another DTM session"]
pub enum BluetoothDtmFirstPreparationCleanupStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The abandoned controller-time latch still owns its result.
    Waiting(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// Time cleanup completed; graph restoration is the next bounded step.
    Continue(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact graph is idle again beside the command which did not run.
    CleanTask(BluetoothDtmFirstPreparationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// A time-worker fault retained the unchanged cleanup owner.
    Fault {
        /// Exact cleanup transaction which may be rechecked or fail-stopped.
        cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact orphan-drain failure.
        error: BluetoothControllerSchedulerCurrentError,
    },
    /// The runtime rejected restoration without separating graph and Controller.
    RestoreRejected(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn drain(
        command: BluetoothDtmDeferredStart<'runtime>,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
        error: BluetoothDtmControllerEventPreparationError,
    ) -> Self {
        Self {
            phase: BluetoothDtmFirstPreparationCleanupPhase::Drain {
                command,
                epoch,
                idle,
                error,
            },
        }
    }

    /// Exact lower preparation failure which made cleanup necessary.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        match &self.phase {
            BluetoothDtmFirstPreparationCleanupPhase::Drain { error, .. }
            | BluetoothDtmFirstPreparationCleanupPhase::Restore { error, .. } => *error,
        }
    }

    /// Execute one finite drain or restore operation.
    pub fn step(self) -> BluetoothDtmFirstPreparationCleanupStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstPreparationCleanupPhase::Drain {
                command,
                mut epoch,
                idle,
                error,
            } => match epoch.drain_abandoned_controller_time() {
                Ok(BluetoothControllerTimeOrphanDrainStep::Waiting) => {
                    BluetoothDtmFirstPreparationCleanupStep::Waiting(Self::drain(
                        command, epoch, idle, error,
                    ))
                }
                Ok(
                    BluetoothControllerTimeOrphanDrainStep::Idle
                    | BluetoothControllerTimeOrphanDrainStep::Drained,
                ) => BluetoothDtmFirstPreparationCleanupStep::Continue(Self {
                    phase: BluetoothDtmFirstPreparationCleanupPhase::Restore {
                        command,
                        task: epoch.into_task_service(),
                        idle,
                        error,
                    },
                }),
                Err(drain_error) => BluetoothDtmFirstPreparationCleanupStep::Fault {
                    cleanup: Self::drain(command, epoch, idle, error),
                    error: drain_error,
                },
            },
            BluetoothDtmFirstPreparationCleanupPhase::Restore {
                command,
                mut task,
                idle,
                error,
            } => match task.restore_dtm_session_idle(idle) {
                Ok(()) => BluetoothDtmFirstPreparationCleanupStep::CleanTask(
                    BluetoothDtmFirstPreparationCleanTask {
                        owner: BluetoothDtmFirstTaskOwner {
                            deferred: command,
                            task,
                        },
                        error,
                    },
                ),
                Err(idle) => BluetoothDtmFirstPreparationCleanupStep::RestoreRejected(Self {
                    phase: BluetoothDtmFirstPreparationCleanupPhase::Restore {
                        command,
                        task,
                        idle,
                        error,
                    },
                }),
            },
        }
    }
}

/// Neutral shutdown cancellation while a preparation cleanup is still pending.
///
/// This wrapper cannot be converted into the normal rejected-start completion;
/// it exists solely to recover or retain the exact Controller owner for
/// shutdown and teardown.
#[must_use = "drive neutral cleanup without manufacturing an HCI response"]
pub struct BluetoothDtmFirstCancellationPreparationCleanup<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded neutral preparation-cleanup transition.
#[must_use = "retain every neutral cancellation owner until cleanup is terminal"]
pub enum BluetoothDtmFirstCancellationPreparationCleanupStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Waiting(BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        cleanup: BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothControllerSchedulerCurrentError,
    },
    RestoreRejected(
        BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn new(cleanup: BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { cleanup }
    }

    /// Exact lower preparation outcome retained for shutdown diagnostics.
    pub const fn error(&self) -> BluetoothDtmControllerEventPreparationError {
        self.cleanup.error()
    }

    /// Execute one finite neutral drain or graph-restore operation.
    pub fn step(
        self,
    ) -> BluetoothDtmFirstCancellationPreparationCleanupStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.cleanup.step() {
            BluetoothDtmFirstPreparationCleanupStep::Waiting(cleanup) => {
                BluetoothDtmFirstCancellationPreparationCleanupStep::Waiting(Self::new(cleanup))
            }
            BluetoothDtmFirstPreparationCleanupStep::Continue(cleanup) => {
                BluetoothDtmFirstCancellationPreparationCleanupStep::Continue(Self::new(cleanup))
            }
            BluetoothDtmFirstPreparationCleanupStep::CleanTask(clean) => {
                BluetoothDtmFirstCancellationPreparationCleanupStep::CleanTask(
                    BluetoothDtmFirstCancellationCleanTask {
                        _owner: clean.owner,
                        preparation_error: Some(clean.error),
                    },
                )
            }
            BluetoothDtmFirstPreparationCleanupStep::Fault { cleanup, error } => {
                BluetoothDtmFirstCancellationPreparationCleanupStep::Fault {
                    cleanup: Self::new(cleanup),
                    error,
                }
            }
            BluetoothDtmFirstPreparationCleanupStep::RestoreRejected(cleanup) => {
                BluetoothDtmFirstCancellationPreparationCleanupStep::RestoreRejected(Self::new(
                    cleanup,
                ))
            }
        }
    }
}

/// Fail-stop owner for an impossible command/preparation role mismatch.
///
/// No decomposition API exists because separating the Controller from the raw
/// lower outcome would re-open role cross-wiring in safe code.
#[must_use = "an invariant fault retains the complete Controller and graph owner"]
pub struct BluetoothDtmFirstInvariantFault<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    expected: crate::BluetoothDtmRole,
    observed: crate::BluetoothDtmRole,
    _command: BluetoothDtmDeferredStart<'runtime>,
    _epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    _outcome: BluetoothDtmControllerPreparationOutcome,
}

impl<S, const SCHEDULER_CAPACITY: usize> BluetoothDtmFirstInvariantFault<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Role required by the retained semantic HCI command.
    pub const fn expected_role(&self) -> crate::BluetoothDtmRole {
        self.expected
    }

    /// Role carried by the impossible lower preparation outcome.
    pub const fn observed_role(&self) -> crate::BluetoothDtmRole {
        self.observed
    }
}

/// Opaque task/graph pair rejected by the runtime idle slot.
#[must_use = "retry restoration without separating the exact task and graph"]
pub struct BluetoothDtmFirstIdleRestore<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothDtmDeferredStart<'runtime>,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    idle: BluetoothDtmSessionIdle,
}

/// Result of retrying one exact idle-slot restoration.
#[must_use = "retain a rejected task/graph pair or consume the clean task"]
pub enum BluetoothDtmFirstIdleRestoreStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// The sole DTM graph is reusable without creating response authority.
    CleanTask(BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// The slot still rejected the unchanged task/graph pair.
    Rejected(BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn new(
        command: BluetoothDtmDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: BluetoothDtmSessionIdle,
    ) -> Self {
        Self {
            command,
            task,
            idle,
        }
    }

    /// Retry one finite restore operation on the same runtime identity.
    pub fn step(mut self) -> BluetoothDtmFirstIdleRestoreStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => BluetoothDtmFirstIdleRestoreStep::CleanTask(
                BluetoothDtmFirstCancellationCleanTask {
                    _owner: BluetoothDtmFirstTaskOwner {
                        deferred: self.command,
                        task: self.task,
                    },
                    preparation_error: None,
                },
            ),
            Err(idle) => {
                self.idle = idle;
                BluetoothDtmFirstIdleRestoreStep::Rejected(self)
            }
        }
    }
}

/// Lossless first-event runner failure.
#[must_use = "every failure retains command and Controller ownership"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain complete affine retry and cleanup owners"
)]
pub enum BluetoothDtmFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdBegin(
        BluetoothDtmFirstAcceptedFailure<
            'runtime,
            BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    ColdRecheck(
        BluetoothDtmFirstAcceptedFailure<
            'runtime,
            BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    WarmBegin(
        BluetoothDtmFirstAcceptedFailure<
            'runtime,
            BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    WarmRecheck(
        BluetoothDtmFirstAcceptedFailure<
            'runtime,
            BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    SessionActive(
        BluetoothDtmFirstAcceptedFailure<
            'runtime,
            BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    /// A failed initial preparation requires time drain and graph restoration.
    PreparationRejected(BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    InvariantFault(BluetoothDtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY>),
    /// A role-consistent graph retained at the exact failed transition.
    Retryable(BluetoothDtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of explicitly cancelling a runner before or after head publication.
#[must_use = "cancellation must retain the recovered or irreversible owner"]
pub enum BluetoothDtmFirstRunnerCancel<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    CleanTask(BluetoothDtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    CleanEpoch(BluetoothDtmFirstCancellationEpoch<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsColdTimeDrain(BluetoothDtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsWarmTimeDrain(BluetoothDtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsPreparationCleanup(
        BluetoothDtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    RestoreRejected(BluetoothDtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
    /// A lower cancel/recheck fault retained its exact owner.
    ///
    /// The `cancel()` implementation reaches only `ColdRecheck`, `WarmRecheck`
    /// or `InvariantFault`; it never manufactures a runnable `Retryable` owner.
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
    /// Cancellation was rejected or crossed published `HEAD`; no resume edge exists.
    FailStop(BluetoothDtmFirstCancellationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Closed reason neutral shutdown could not recover a reusable first-event owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmFirstCancellationFailStopReason {
    /// The prepared graph rejected cancellation before head publication.
    PreparedCancellationRejected,
    /// Scheduler `HEAD` was already visible and requires future powered quiescence.
    SchedulerHeadPublished,
}

/// Opaque shutdown owner after cancellation crossed an irreversible boundary.
///
/// Until powered quiescence exists this type deliberately exposes no runner,
/// step, response or decomposition edge. Resuming it could otherwise reach
/// scheduler `RUN` and manufacture a successful completion after shutdown.
#[must_use = "retain the fail-stop shutdown owner for powered teardown"]
pub struct BluetoothDtmFirstCancellationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    reason: BluetoothDtmFirstCancellationFailStopReason,
    role: crate::BluetoothDtmRole,
    _runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstCancellationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn new(
        runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
        reason: BluetoothDtmFirstCancellationFailStopReason,
    ) -> Self {
        let role = runner.role();
        BluetoothDtmFirstCancellationFailStop {
            reason,
            role,
            _runner: runner,
        }
    }

    /// Exact irreversible shutdown boundary.
    pub const fn reason(&self) -> BluetoothDtmFirstCancellationFailStopReason {
        self.reason
    }

    /// Hardware role retained by the opaque runner.
    pub const fn role(&self) -> crate::BluetoothDtmRole {
        self.role
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_phase(phase: BluetoothDtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { phase }
    }

    fn role(&self) -> crate::BluetoothDtmRole {
        match &self.phase {
            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, .. }
            | BluetoothDtmFirstRunnerPhase::WarmCurrent { command, .. }
            | BluetoothDtmFirstRunnerPhase::CurrentReady { command, .. }
            | BluetoothDtmFirstRunnerPhase::Preparation { command, .. }
            | BluetoothDtmFirstRunnerPhase::TransmitterPrepared { command, .. }
            | BluetoothDtmFirstRunnerPhase::ReceiverPrepared { command, .. }
            | BluetoothDtmFirstRunnerPhase::TransmitterHead { command, .. }
            | BluetoothDtmFirstRunnerPhase::ReceiverHead { command, .. } => command.role(),
        }
    }

    /// Begin either the cold first-live or warm fresh-current acquisition.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains exact command and Controller owners"
    )]
    pub(crate) fn begin(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: BluetoothDtmDeferredStart<'runtime>,
    ) -> Result<Self, BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending },
                )),
                Err(failure) => Err(BluetoothDtmFirstRunnerFailure::WarmBegin(
                    BluetoothDtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            Err(unavailable) => {
                match unavailable
                    .into_task_service()
                    .begin_always_awake_post_enable_time()
                {
                    Ok(pending) => Ok(Self::from_phase(
                        BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending },
                    )),
                    Err(failure) => Err(BluetoothDtmFirstRunnerFailure::ColdBegin(
                        BluetoothDtmFirstAcceptedFailure::new(command, failure),
                    )),
                }
            }
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending },
                        ))
                    }
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::ColdRecheck(
                            BluetoothDtmFirstAcceptedFailure::new(command, failure),
                        ),
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending },
                        ))
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current },
                        ))
                    }
                    Err(failure) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::WarmRecheck(
                            BluetoothDtmFirstAcceptedFailure::new(command, failure),
                        ),
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current } => {
                Self::begin_preparation(command, current)
            }
            BluetoothDtmFirstRunnerPhase::Preparation { command, pending } => {
                match pending.recheck() {
                    BluetoothDtmControllerPreparationStep::Pending(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    BluetoothDtmControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(command, terminal)
                    }
                }
            }
            BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner =
                        Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                            command,
                            task,
                            merged,
                        });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::BluetoothDtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothDtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                        command,
                        task,
                        merged,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::BluetoothDtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::TransmitterHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => BluetoothDtmFirstRunnerStep::Running(BluetoothDtmFirstRunning {
                    running: BluetoothDtmFirstRunningPhase::Transmitter {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::BluetoothDtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            BluetoothDtmFirstRunnerPhase::ReceiverHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => BluetoothDtmFirstRunnerStep::Running(BluetoothDtmFirstRunning {
                    running: BluetoothDtmFirstRunningPhase::Receiver {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    });
                    BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::Retryable(
                        BluetoothDtmFirstRunnerRetry {
                            cause: BluetoothDtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::BluetoothDtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
        }
    }

    fn begin_preparation(
        command: BluetoothDtmDeferredStart<'runtime>,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match &command.kind {
            BluetoothDtmDeferredStartKind::Transmitter(deferred) => {
                let program = crate::dtm_command::transmitter_program(deferred.command());
                let result = current.begin_dtm_transmitter_first_item(
                    program.pattern,
                    program.length,
                    program.channel,
                    program.phy,
                    program.requested_interval_micros,
                );
                match result {
                    Ok(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(
                        current,
                    )) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::SessionActive(
                            BluetoothDtmFirstAcceptedFailure::new(command, current),
                        ),
                    ),
                    Err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal(
                        terminal,
                    )) => Self::finish_preparation(command, terminal),
                }
            }
            BluetoothDtmDeferredStartKind::Receiver(deferred) => {
                let program = crate::dtm_command::receiver_program(deferred.command());
                let result = current.begin_dtm_receiver_first_item(program.channel, program.phy);
                match result {
                    Ok(pending) => {
                        BluetoothDtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothDtmFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    Err(BluetoothDtmControllerInitialPreparationFailure::SessionActive(
                        current,
                    )) => BluetoothDtmFirstRunnerStep::Failed(
                        BluetoothDtmFirstRunnerFailure::SessionActive(
                            BluetoothDtmFirstAcceptedFailure::new(command, current),
                        ),
                    ),
                    Err(BluetoothDtmControllerInitialPreparationFailure::PreparationTerminal(
                        terminal,
                    )) => Self::finish_preparation(command, terminal),
                }
            }
        }
    }

    fn finish_preparation(
        command: BluetoothDtmDeferredStart<'runtime>,
        terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command.kind, outcome) {
            (
                BluetoothDtmDeferredStartKind::Transmitter(deferred),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Ok(merged)),
            ) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                    command: BluetoothDtmDeferredStart::transmitter(deferred),
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                BluetoothDtmDeferredStartKind::Receiver(deferred),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Ok(merged)),
            ) => BluetoothDtmFirstRunnerStep::Continue(Self::from_phase(
                BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                    command: BluetoothDtmDeferredStart::receiver(deferred),
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                BluetoothDtmDeferredStartKind::Transmitter(deferred),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerStep::Failed(
                BluetoothDtmFirstRunnerFailure::PreparationRejected(
                    Self::transmitter_preparation_cleanup(deferred, epoch, failure),
                ),
            ),
            (
                BluetoothDtmDeferredStartKind::Receiver(deferred),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerStep::Failed(
                BluetoothDtmFirstRunnerFailure::PreparationRejected(
                    Self::receiver_preparation_cleanup(deferred, epoch, failure),
                ),
            ),
            (kind, outcome) => {
                BluetoothDtmFirstRunnerStep::Failed(BluetoothDtmFirstRunnerFailure::InvariantFault(
                    Self::invariant_fault(BluetoothDtmDeferredStart { kind }, epoch, outcome),
                ))
            }
        }
    }

    fn transmitter_preparation_cleanup(
        deferred: LeControllerDeferredTransmitterStart<'runtime, ()>,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: BluetoothDtmControllerTxPreparationFailure,
    ) -> BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _, _) = failure.into_parts();
        BluetoothDtmFirstPreparationCleanup::drain(
            BluetoothDtmDeferredStart::transmitter(deferred),
            epoch,
            BluetoothDtmSessionIdle::new(graph),
            error,
        )
    }

    fn receiver_preparation_cleanup(
        deferred: LeControllerDeferredReceiverStart<'runtime, ()>,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: BluetoothDtmControllerRxPreparationFailure,
    ) -> BluetoothDtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _) = failure.into_owner().into_memory_and_packet_count();
        BluetoothDtmFirstPreparationCleanup::drain(
            BluetoothDtmDeferredStart::receiver(deferred),
            epoch,
            BluetoothDtmSessionIdle::new(graph),
            error,
        )
    }

    fn cancelled_preparation(
        command: BluetoothDtmDeferredStart<'runtime>,
        terminal: BluetoothDtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command.kind, outcome) {
            (
                BluetoothDtmDeferredStartKind::Transmitter(deferred),
                BluetoothDtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerCancel::NeedsPreparationCleanup(
                BluetoothDtmFirstCancellationPreparationCleanup::new(
                    Self::transmitter_preparation_cleanup(deferred, epoch, failure),
                ),
            ),
            (
                BluetoothDtmDeferredStartKind::Receiver(deferred),
                BluetoothDtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => BluetoothDtmFirstRunnerCancel::NeedsPreparationCleanup(
                BluetoothDtmFirstCancellationPreparationCleanup::new(
                    Self::receiver_preparation_cleanup(deferred, epoch, failure),
                ),
            ),
            (kind, outcome) => BluetoothDtmFirstRunnerCancel::Failed(
                BluetoothDtmFirstRunnerFailure::InvariantFault(Self::invariant_fault(
                    BluetoothDtmDeferredStart { kind },
                    epoch,
                    outcome,
                )),
            ),
        }
    }

    fn invariant_fault(
        command: BluetoothDtmDeferredStart<'runtime>,
        epoch: BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: BluetoothDtmControllerPreparationOutcome,
    ) -> BluetoothDtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY> {
        let expected = command.role();
        let observed = match &outcome {
            BluetoothDtmControllerPreparationOutcome::TransmitterFirst(_)
            | BluetoothDtmControllerPreparationOutcome::TransmitterRecurring(_) => {
                crate::BluetoothDtmRole::Transmitter
            }
            BluetoothDtmControllerPreparationOutcome::ReceiverFirst(_)
            | BluetoothDtmControllerPreparationOutcome::ReceiverRecurring(_) => {
                crate::BluetoothDtmRole::Receiver
            }
        };
        BluetoothDtmFirstInvariantFault {
            expected,
            observed,
            _command: command,
            _epoch: epoch,
            _outcome: outcome,
        }
    }

    /// Neutrally cancel reversible work for Controller shutdown.
    ///
    /// Dropping or cancelling an executor await is not this transition: the
    /// caller retains the runner across await cancellation. This operation
    /// recovers ownership but deliberately creates no HCI response authority.
    pub fn cancel(self) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothDtmFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.cancel() {
                    Ok(task) => BluetoothDtmFirstRunnerCancel::NeedsColdTimeDrain(
                        BluetoothDtmFirstColdTimeDrain {
                            deferred: command,
                            task,
                        },
                    ),
                    Err(failure) => BluetoothDtmFirstRunnerCancel::Failed(
                        BluetoothDtmFirstRunnerFailure::ColdRecheck(
                            BluetoothDtmFirstAcceptedFailure::new(command, failure),
                        ),
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.cancel() {
                    Ok(epoch) => BluetoothDtmFirstRunnerCancel::NeedsWarmTimeDrain(
                        BluetoothDtmFirstWarmTimeDrain {
                            deferred: command,
                            epoch,
                        },
                    ),
                    Err(failure) => BluetoothDtmFirstRunnerCancel::Failed(
                        BluetoothDtmFirstRunnerFailure::WarmRecheck(
                            BluetoothDtmFirstAcceptedFailure::new(command, failure),
                        ),
                    ),
                }
            }
            BluetoothDtmFirstRunnerPhase::CurrentReady { command, current } => {
                BluetoothDtmFirstRunnerCancel::CleanEpoch(BluetoothDtmFirstCancellationEpoch {
                    deferred: command,
                    _epoch: current.into_retained_epoch(),
                })
            }
            BluetoothDtmFirstRunnerPhase::Preparation { command, pending } => {
                Self::cancelled_preparation(command, pending.cancel())
            }
            BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_transmitter_first_item(merged) {
                Ok((graph, _, _)) => {
                    let idle = BluetoothDtmSessionIdle::new(graph);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => BluetoothDtmFirstRunnerCancel::CleanTask(
                            BluetoothDtmFirstCancellationCleanTask {
                                _owner: BluetoothDtmFirstTaskOwner {
                                    deferred: command,
                                    task,
                                },
                                preparation_error: None,
                            },
                        ),
                        Err(idle) => BluetoothDtmFirstRunnerCancel::RestoreRejected(
                            BluetoothDtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => BluetoothDtmFirstRunnerCancel::FailStop(
                    BluetoothDtmFirstCancellationFailStop::new(
                        Self::from_phase(BluetoothDtmFirstRunnerPhase::TransmitterPrepared {
                            command,
                            task,
                            merged,
                        }),
                        BluetoothDtmFirstCancellationFailStopReason::PreparedCancellationRejected,
                    ),
                ),
            },
            BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_receiver_first_item(merged) {
                Ok(owner) => {
                    let (graph, _) = owner.into_memory_and_packet_count();
                    let idle = BluetoothDtmSessionIdle::new(graph);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => BluetoothDtmFirstRunnerCancel::CleanTask(
                            BluetoothDtmFirstCancellationCleanTask {
                                _owner: BluetoothDtmFirstTaskOwner {
                                    deferred: command,
                                    task,
                                },
                                preparation_error: None,
                            },
                        ),
                        Err(idle) => BluetoothDtmFirstRunnerCancel::RestoreRejected(
                            BluetoothDtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => BluetoothDtmFirstRunnerCancel::FailStop(
                    BluetoothDtmFirstCancellationFailStop::new(
                        Self::from_phase(BluetoothDtmFirstRunnerPhase::ReceiverPrepared {
                            command,
                            task,
                            merged,
                        }),
                        BluetoothDtmFirstCancellationFailStopReason::PreparedCancellationRejected,
                    ),
                ),
            },
            head @ (BluetoothDtmFirstRunnerPhase::TransmitterHead { .. }
            | BluetoothDtmFirstRunnerPhase::ReceiverHead { .. }) => {
                BluetoothDtmFirstRunnerCancel::FailStop(BluetoothDtmFirstCancellationFailStop::new(
                    Self::from_phase(head),
                    BluetoothDtmFirstCancellationFailStopReason::SchedulerHeadPublished,
                ))
            }
        }
    }
}
