//! Bounded first-event runner for legacy LE Direct Test Mode.
//!
//! The runner owns one semantic HCI command together with every affine
//! Controller typestate needed to reach the first scheduler `RUN`. Each
//! [`DtmFirstRunner::step`] call performs exactly one finite lower
//! transition. In particular, a controller-time `Waiting` result is returned
//! to the executor instead of being polled in a hidden loop.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        AlwaysAwakePostEnableTimeBeginFailure, AlwaysAwakePostEnableTimeError,
        AlwaysAwakePostEnableTimeFailure, AlwaysAwakePostEnableTimeOrphanDrainStep,
        AlwaysAwakePostEnableTimePending, AlwaysAwakePostEnableTimeStep,
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginFailure,
        ControllerSchedulerCurrentError, ControllerSchedulerCurrentFailure,
        ControllerSchedulerCurrentPending, ControllerSchedulerCurrentStep,
        ControllerSchedulerEpochRetained, ControllerSchedulerNowReady,
        ControllerTimeOrphanDrainStep, DtmControllerInitialPreparationFailure,
        DtmControllerPreparationOutcome, DtmControllerPreparationPending,
        DtmControllerPreparationStep, DtmControllerPreparationTerminal,
        SchedulerRunInterruptStorage,
    },
    le::dtm::{DtmReceiverEvent, DtmSessionIdle, DtmTransmitterEvent},
    scheduler::{
        DtmControllerEventPreparationError, DtmControllerRxPreparationFailure,
        DtmControllerTxPreparationFailure, DtmEmptySchedulerMergePrepared,
        DtmInitialSchedulerItemPhase, DtmSchedulerHeadPublished, DtmSchedulerRunning,
        SchedulerHeadPublicationError,
        core::{DtmFirstPreparationCompletionClass, classify_dtm_first_preparation_completion},
    },
};

use oer_bluetooth_hci::{
    LeControllerClassifiedCommand, LeControllerDeferredReceiverStart,
    LeControllerDeferredTransmitterStart, LeControllerResponsePending,
};

enum DtmDeferredStartKind<'runtime> {
    Transmitter(LeControllerDeferredTransmitterStart<'runtime, ()>),
    Receiver(LeControllerDeferredReceiverStart<'runtime, ()>),
}

/// Opaque accepted DTM start retaining its semantic command and HCI order.
///
/// The portable role-specific deferred start is never decomposed through this
/// public API. Chip phases may move the hardware owner independently, while
/// the exact start response remains constructible only after scheduler `RUN`.
#[must_use = "retain the accepted start until hardware starts or is recovered"]
pub struct DtmDeferredStart<'runtime> {
    kind: DtmDeferredStartKind<'runtime>,
}

impl DtmDeferredStart<'_> {
    /// Hardware role selected by the accepted semantic command.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        match self.kind {
            DtmDeferredStartKind::Transmitter(_) => crate::le::dtm::DtmRole::Transmitter,
            DtmDeferredStartKind::Receiver(_) => crate::le::dtm::DtmRole::Receiver,
        }
    }
}

impl<'runtime> DtmDeferredStart<'runtime> {
    pub(crate) fn transmitter(start: LeControllerDeferredTransmitterStart<'runtime, ()>) -> Self {
        Self {
            kind: DtmDeferredStartKind::Transmitter(start),
        }
    }

    pub(crate) fn receiver(start: LeControllerDeferredReceiverStart<'runtime, ()>) -> Self {
        Self {
            kind: DtmDeferredStartKind::Receiver(start),
        }
    }

    fn into_started_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        match self.kind {
            DtmDeferredStartKind::Transmitter(start) => {
                start.map_owner(|()| owner).into_started_response()
            }
            DtmDeferredStartKind::Receiver(start) => {
                start.map_owner(|()| owner).into_started_response()
            }
        }
    }

    fn into_hardware_failure_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        match self.kind {
            DtmDeferredStartKind::Transmitter(start) => {
                start.map_owner(|()| owner).into_hardware_failure_response()
            }
            DtmDeferredStartKind::Receiver(start) => {
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
struct DtmFirstTaskOwner<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: DtmDeferredStart<'runtime>,
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Hardware role retained by this exact recovery owner.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.deferred.role()
    }

    fn into_hardware_failure_response(
        self,
    ) -> crate::controller::ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY> {
        crate::controller::ControllerIdleResponsePending::new(
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
pub struct DtmFirstCancellationCleanTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    _owner: DtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    preparation_error: Option<DtmControllerEventPreparationError>,
}

impl<S, const SCHEDULER_CAPACITY: usize> DtmFirstCancellationCleanTask<'_, S, SCHEDULER_CAPACITY> {
    /// Hardware role retained by the neutrally cancelled command.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self._owner.role()
    }

    /// Lower preparation outcome retained when cancellation interrupted that phase.
    pub const fn preparation_error(&self) -> Option<DtmControllerEventPreparationError> {
        self.preparation_error
    }
}

/// Neutral cancellation paired with a retained scheduler epoch.
#[must_use = "recover the idle task without manufacturing response authority"]
pub struct DtmFirstCancellationEpoch<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: DtmDeferredStart<'runtime>,
    _epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<S, const SCHEDULER_CAPACITY: usize> DtmFirstCancellationEpoch<'_, S, SCHEDULER_CAPACITY> {
    /// Hardware role retained by this exact recovery owner.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.deferred.role()
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstCancellationEpoch<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Discard an unused current sample and recover a neutral cleanup owner.
    pub fn into_clean_task(self) -> DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY> {
        DtmFirstCancellationCleanTask {
            _owner: DtmFirstTaskOwner {
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
pub struct DtmFirstColdTimeDrain<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: DtmDeferredStart<'runtime>,
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded cold-current cancellation drain result.
#[must_use = "retain Waiting or Fault; CleanTask remains neutral shutdown ownership"]
pub enum DtmFirstColdTimeDrainStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(DtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        drain: DtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>,
        error: AlwaysAwakePostEnableTimeError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform one bounded observation without manufacturing a failure response.
    pub fn step(mut self) -> DtmFirstColdTimeDrainStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.task.drain_abandoned_always_awake_post_enable_time() {
            Ok(AlwaysAwakePostEnableTimeOrphanDrainStep::Waiting) => {
                DtmFirstColdTimeDrainStep::Waiting(self)
            }
            Ok(
                AlwaysAwakePostEnableTimeOrphanDrainStep::Idle
                | AlwaysAwakePostEnableTimeOrphanDrainStep::Drained,
            ) => DtmFirstColdTimeDrainStep::CleanTask(DtmFirstCancellationCleanTask {
                _owner: DtmFirstTaskOwner {
                    deferred: self.deferred,
                    task: self.task,
                },
                preparation_error: None,
            }),
            Err(error) => DtmFirstColdTimeDrainStep::Fault { drain: self, error },
        }
    }
}

/// Cancelled warm-current request awaiting bounded orphan drain.
#[must_use = "drain the abandoned warm-current request before completing cancellation"]
pub struct DtmFirstWarmTimeDrain<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    deferred: DtmDeferredStart<'runtime>,
    epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded warm-current cancellation drain result.
#[must_use = "retain Waiting or Fault; CleanTask remains neutral shutdown ownership"]
pub enum DtmFirstWarmTimeDrainStep<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    Waiting(DtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        drain: DtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>,
        error: ControllerSchedulerCurrentError,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Perform one bounded observation without manufacturing a failure response.
    pub fn step(mut self) -> DtmFirstWarmTimeDrainStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.epoch.drain_abandoned_controller_time() {
            Ok(ControllerTimeOrphanDrainStep::Waiting) => DtmFirstWarmTimeDrainStep::Waiting(self),
            Ok(ControllerTimeOrphanDrainStep::Idle | ControllerTimeOrphanDrainStep::Drained) => {
                DtmFirstWarmTimeDrainStep::CleanTask(DtmFirstCancellationCleanTask {
                    _owner: DtmFirstTaskOwner {
                        deferred: self.deferred,
                        task: self.epoch.into_task_service(),
                    },
                    preparation_error: None,
                })
            }
            Err(error) => DtmFirstWarmTimeDrainStep::Fault { drain: self, error },
        }
    }
}

/// Opaque accepted start paired with one lower pre-`RUN` owner.
///
/// Private fields make it impossible to separate portable command order from
/// the lower failure/current owner at a public recovery boundary.
#[must_use = "retain the accepted start and lower owner as one transaction"]
pub struct DtmFirstAcceptedFailure<'runtime, Owner> {
    deferred: DtmDeferredStart<'runtime>,
    owner: Owner,
}

impl<'runtime, Owner> DtmFirstAcceptedFailure<'runtime, Owner> {
    fn new(deferred: DtmDeferredStart<'runtime>, owner: Owner) -> Self {
        Self { deferred, owner }
    }

    /// Hardware role retained by this exact failed transition.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.deferred.role()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    DtmFirstAcceptedFailure<'_, AlwaysAwakePostEnableTimeBeginFailure<'_, S, SCHEDULER_CAPACITY>>
{
    /// Exact cold-acquisition rejection retained by this owner.
    pub const fn cold_begin_error(&self) -> crate::controller::AlwaysAwakePostEnableTimeBeginError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    DtmFirstAcceptedFailure<'_, AlwaysAwakePostEnableTimeFailure<'_, S, SCHEDULER_CAPACITY>>
{
    /// Exact cold-acquisition fail-stop observation retained by this owner.
    pub const fn cold_recheck_error(&self) -> crate::controller::AlwaysAwakePostEnableTimeError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    DtmFirstAcceptedFailure<'_, ControllerSchedulerCurrentBeginFailure<'_, S, SCHEDULER_CAPACITY>>
{
    /// Exact warm-acquisition rejection retained by this owner.
    pub const fn warm_begin_error(
        &self,
    ) -> crate::controller::ControllerSchedulerCurrentBeginError {
        self.owner.error()
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    DtmFirstAcceptedFailure<'_, ControllerSchedulerCurrentFailure<'_, S, SCHEDULER_CAPACITY>>
{
    /// Exact warm-acquisition fail-stop observation retained by this owner.
    pub const fn warm_recheck_error(&self) -> crate::controller::ControllerSchedulerCurrentError {
        self.owner.error()
    }
}

/// Opaque fail-stop owner for an impossible mismatch after combined intake.
///
/// Task ownership, classification and next-command authority cannot be
/// decomposed through this chip API.
#[must_use = "retain the complete post-intake mismatch owner"]
pub struct ControllerIdleCommandMismatch<'runtime, 'command, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _command: LeControllerClassifiedCommand<
        'runtime,
        'command,
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    >,
}

impl<'runtime, 'command, S, const SCHEDULER_CAPACITY: usize>
    ControllerIdleCommandMismatch<'runtime, 'command, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        command: LeControllerClassifiedCommand<
            'runtime,
            'command,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
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
#[expect(
    clippy::large_enum_variant,
    reason = "each variant retains its exact command, radio continuation, or sealed failure owners inline"
)]
pub enum ControllerIdleCommandRoute<'runtime, 'command, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// A validated RX/TX command entered the sole first-event runner.
    Start(DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// A validated non-connectable Enable entered the advertising runner.
    StartLegacyNonconnectableAdvertising(
        crate::le::advertising::LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// A validated connectable Enable entered the response-capable runner.
    StartLegacyConnectableAdvertising(
        crate::le::advertising::LegacyConnectableAdvertisingFirstRunner<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ),
    /// A validated passive scanner Enable entered the HCI-composed first runner.
    StartPassiveScanning(
        crate::le::scanning::PassiveScanHciFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Initial Controller-time acquisition failed without losing any owner.
    StartFailed(DtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
    /// Advertising current acquisition failed without losing HCI order.
    LegacyAdvertisingStartFailed(
        crate::le::advertising::LegacyAdvertisingFirstRunnerFailure<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ),
    /// Connectable advertising failed before the bounded runner was created.
    LegacyConnectableAdvertisingStartFailed(
        crate::le::advertising::LegacyConnectableAdvertisingFirstRunnerFailure<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ),
    /// Passive scanner start failed without losing HCI order or lower ownership.
    PassiveScanStartFailed(
        crate::le::scanning::PassiveScanHciFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Idle Test End became an ordered standard zero-count response.
    ResponsePending(
        crate::controller::ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Idle Reset retains its exact command/order until lifecycle completion.
    ResetBarrier(crate::controller::ControllerIdleResetBarrier<'runtime, S, SCHEDULER_CAPACITY>),
    /// Defensive fail-stop owner for an impossible post-intake epoch mismatch.
    ///
    /// Classification, task ownership and command authority remain inseparable.
    EndpointMismatch(ControllerIdleCommandMismatch<'runtime, 'command, S, SCHEDULER_CAPACITY>),
}

/// Bounded first-event Controller runner.
///
/// Every variant retains the sole task service either directly or inside one
/// lower affine state. The private role-specific variants prevent a TX command
/// from being paired with an RX descriptor graph.
#[must_use = "step or explicitly cancel the affine DTM runner"]
pub struct DtmFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain complete affine Controller owners"
)]
enum DtmFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: DtmDeferredStart<'runtime>,
        pending: AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: DtmDeferredStart<'runtime>,
        pending: ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: DtmDeferredStart<'runtime>,
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: DtmDeferredStart<'runtime>,
        pending: DtmControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    TransmitterPrepared {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: DtmEmptySchedulerMergePrepared<DtmTransmitterEvent, DtmInitialSchedulerItemPhase>,
    },
    ReceiverPrepared {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: DtmEmptySchedulerMergePrepared<DtmReceiverEvent, DtmInitialSchedulerItemPhase>,
    },
    TransmitterHead {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: DtmSchedulerHeadPublished<DtmTransmitterEvent>,
    },
    ReceiverHead {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: DtmSchedulerHeadPublished<DtmReceiverEvent>,
    },
}

/// Result of one bounded runner step.
#[must_use = "retain pending ownership or handle the terminal first-event result"]
pub enum DtmFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// A self-clearing controller-time latch still needs a later observation.
    WaitControllerTime(DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The previous bounded transition completed; another can run immediately.
    Continue(DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact first graph reached hardware scheduler `RUN`.
    Running(DtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite transition failed while retaining its exact owner.
    Failed(DtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First DTM event admitted to the hardware scheduler.
///
/// The semantic command remains owned until its Command Complete response is
/// durably published. Hardware progress therefore does not depend on HCI
/// Controller-to-Host queue capacity.
#[must_use = "the running graph and pending HCI response authority must remain owned"]
pub struct DtmFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    running: DtmFirstRunningPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum DtmFirstRunningPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Transmitter {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: DtmSchedulerRunning<DtmTransmitterEvent>,
    },
    Receiver {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: DtmSchedulerRunning<DtmReceiverEvent>,
    },
}

pub(crate) enum DtmFirstRunningParts<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Transmitter {
        response: LeControllerResponsePending<
            'runtime,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
        running: DtmSchedulerRunning<DtmTransmitterEvent>,
    },
    Receiver {
        response: LeControllerResponsePending<
            'runtime,
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        >,
        running: DtmSchedulerRunning<DtmReceiverEvent>,
    },
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize> DtmFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(self) -> DtmFirstRunningParts<'runtime, S, SCHEDULER_CAPACITY> {
        match self.running {
            DtmFirstRunningPhase::Transmitter {
                command,
                task,
                running,
            } => DtmFirstRunningParts::Transmitter {
                response: command.into_started_response(task),
                running,
            },
            DtmFirstRunningPhase::Receiver {
                command,
                task,
                running,
            } => DtmFirstRunningParts::Receiver {
                response: command.into_started_response(task),
                running,
            },
        }
    }
}

/// Finite reason a pre-`RUN` runner transition may be retried unchanged.
#[must_use = "the retry cause should be inspected before advancing the retained runner"]
pub enum DtmFirstRunnerRetryCause<E> {
    /// The CPU-owned graph could not become the scheduler hardware-list head.
    HeadPublication(SchedulerHeadPublicationError),
    /// Dynamic scheduler interrupt preparation rejected the published head.
    SchedulerStart(E),
}

/// Opaque retry owner for a role-consistent pre-`RUN` state.
///
/// The private fields prevent safe code from pairing a TX command with an RX
/// graph or from pairing a task service with another published head.
#[must_use = "inspect the cause and retain, retry, or cancel the exact runner"]
pub struct DtmFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: DtmFirstRunnerRetryCause<S::Error>,
    role: crate::le::dtm::DtmRole,
    runner: DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Role of the exact command and graph retained for retry.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.role
    }

    /// Recover the owning cause and the only runner which may retry it.
    pub fn into_parts(
        self,
    ) -> (
        DtmFirstRunnerRetryCause<S::Error>,
        DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ) {
        (self.cause, self.runner)
    }
}

/// Opaque cleanup after an initial DTM preparation released its CPU graph.
///
/// The graph remains paired with the exact Controller while an abandoned
/// controller-time request is drained and the runtime idle slot is restored.
#[must_use = "drive cleanup until the exact DTM graph is restored"]
pub struct DtmFirstPreparationCleanup<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmFirstPreparationCleanupPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum DtmFirstPreparationCleanupPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Drain {
        command: DtmDeferredStart<'runtime>,
        epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: DtmSessionIdle,
        error: DtmControllerEventPreparationError,
    },
    Restore {
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: DtmSessionIdle,
        error: DtmControllerEventPreparationError,
    },
}

/// Restored initial-preparation owner awaiting the chip's closed HCI policy.
///
/// The graph is back in the runtime idle slot, but the retained lower error
/// still decides whether command authority may reopen. Callers cannot separate
/// the task owner from that decision.
#[must_use = "consume the restored owner through its closed completion policy"]
pub struct DtmFirstPreparationCleanTask<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    owner: DtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    error: DtmControllerEventPreparationError,
}

impl<S, const SCHEDULER_CAPACITY: usize> DtmFirstPreparationCleanTask<'_, S, SCHEDULER_CAPACITY> {
    /// Exact preparation rejection retained for diagnostics.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Hardware role retained by the accepted command and restored graph.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.owner.role()
    }
}

/// Opaque restored owner whose preparation failure poisoned an invariant.
///
/// No decomposition or HCI-response edge exists: graph-slot restoration does
/// not make a role, identity, list or worker-ownership disagreement reusable.
#[must_use = "retain the fail-stop Controller owner for reset or teardown"]
pub struct DtmFirstPreparationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    _owner: DtmFirstTaskOwner<'runtime, S, SCHEDULER_CAPACITY>,
    error: DtmControllerEventPreparationError,
}

impl<S, const SCHEDULER_CAPACITY: usize> DtmFirstPreparationFailStop<'_, S, SCHEDULER_CAPACITY> {
    /// Exact invariant, identity, list or ownership failure.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.error
    }

    /// Hardware role retained by the accepted command and recovered graph.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self._owner.role()
    }
}

/// Closed completion after an initial preparation restored its idle graph.
#[must_use = "publish the ordered rejection or retain the opaque fail-stop owner"]
pub enum DtmFirstPreparationCompletion<'runtime, S, const SCHEDULER_CAPACITY: usize> {
    /// A finite timing/resource rejection classified as Hardware Failure.
    ResponsePending(
        crate::controller::ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    /// Restoration succeeded, but the retained failure forbids reuse.
    FailStop(DtmFirstPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstPreparationCleanTask<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Apply the chip-owned failure classification without caller-selected status.
    pub fn into_completion(self) -> DtmFirstPreparationCompletion<'runtime, S, SCHEDULER_CAPACITY> {
        match classify_dtm_first_preparation_completion(self.error) {
            DtmFirstPreparationCompletionClass::HardwareFailure => {
                DtmFirstPreparationCompletion::ResponsePending(
                    self.owner.into_hardware_failure_response(),
                )
            }
            DtmFirstPreparationCompletionClass::FailStop => {
                DtmFirstPreparationCompletion::FailStop(DtmFirstPreparationFailStop {
                    _owner: self.owner,
                    error: self.error,
                })
            }
        }
    }
}

/// One bounded graph-cleanup transition.
#[must_use = "retain cleanup ownership until the Controller can start another DTM session"]
pub enum DtmFirstPreparationCleanupStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The abandoned controller-time latch still owns its result.
    Waiting(DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// Time cleanup completed; graph restoration is the next bounded step.
    Continue(DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    /// The exact graph is idle again beside the command which did not run.
    CleanTask(DtmFirstPreparationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// A time-worker fault retained the unchanged cleanup owner.
    Fault {
        /// Exact cleanup transaction which may be rechecked or fail-stopped.
        cleanup: DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
        /// Exact orphan-drain failure.
        error: ControllerSchedulerCurrentError,
    },
    /// The runtime rejected restoration without separating graph and Controller.
    RestoreRejected(DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn drain(
        command: DtmDeferredStart<'runtime>,
        epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        idle: DtmSessionIdle,
        error: DtmControllerEventPreparationError,
    ) -> Self {
        Self {
            phase: DtmFirstPreparationCleanupPhase::Drain {
                command,
                epoch,
                idle,
                error,
            },
        }
    }

    /// Exact lower preparation failure which made cleanup necessary.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        match &self.phase {
            DtmFirstPreparationCleanupPhase::Drain { error, .. }
            | DtmFirstPreparationCleanupPhase::Restore { error, .. } => *error,
        }
    }

    /// Execute one finite drain or restore operation.
    pub fn step(self) -> DtmFirstPreparationCleanupStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            DtmFirstPreparationCleanupPhase::Drain {
                command,
                mut epoch,
                idle,
                error,
            } => match epoch.drain_abandoned_controller_time() {
                Ok(ControllerTimeOrphanDrainStep::Waiting) => {
                    DtmFirstPreparationCleanupStep::Waiting(Self::drain(
                        command, epoch, idle, error,
                    ))
                }
                Ok(
                    ControllerTimeOrphanDrainStep::Idle | ControllerTimeOrphanDrainStep::Drained,
                ) => DtmFirstPreparationCleanupStep::Continue(Self {
                    phase: DtmFirstPreparationCleanupPhase::Restore {
                        command,
                        task: epoch.into_task_service(),
                        idle,
                        error,
                    },
                }),
                Err(drain_error) => DtmFirstPreparationCleanupStep::Fault {
                    cleanup: Self::drain(command, epoch, idle, error),
                    error: drain_error,
                },
            },
            DtmFirstPreparationCleanupPhase::Restore {
                command,
                mut task,
                idle,
                error,
            } => match task.restore_dtm_session_idle(idle) {
                Ok(()) => DtmFirstPreparationCleanupStep::CleanTask(DtmFirstPreparationCleanTask {
                    owner: DtmFirstTaskOwner {
                        deferred: command,
                        task,
                    },
                    error,
                }),
                Err(idle) => DtmFirstPreparationCleanupStep::RestoreRejected(Self {
                    phase: DtmFirstPreparationCleanupPhase::Restore {
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
pub struct DtmFirstCancellationPreparationCleanup<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cleanup: DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
}

/// One bounded neutral preparation-cleanup transition.
#[must_use = "retain every neutral cancellation owner until cleanup is terminal"]
pub enum DtmFirstCancellationPreparationCleanupStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Waiting(DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    CleanTask(DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    Fault {
        cleanup: DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
        error: ControllerSchedulerCurrentError,
    },
    RestoreRejected(DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn new(cleanup: DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { cleanup }
    }

    /// Exact lower preparation outcome retained for shutdown diagnostics.
    pub const fn error(&self) -> DtmControllerEventPreparationError {
        self.cleanup.error()
    }

    /// Execute one finite neutral drain or graph-restore operation.
    pub fn step(
        self,
    ) -> DtmFirstCancellationPreparationCleanupStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.cleanup.step() {
            DtmFirstPreparationCleanupStep::Waiting(cleanup) => {
                DtmFirstCancellationPreparationCleanupStep::Waiting(Self::new(cleanup))
            }
            DtmFirstPreparationCleanupStep::Continue(cleanup) => {
                DtmFirstCancellationPreparationCleanupStep::Continue(Self::new(cleanup))
            }
            DtmFirstPreparationCleanupStep::CleanTask(clean) => {
                DtmFirstCancellationPreparationCleanupStep::CleanTask(
                    DtmFirstCancellationCleanTask {
                        _owner: clean.owner,
                        preparation_error: Some(clean.error),
                    },
                )
            }
            DtmFirstPreparationCleanupStep::Fault { cleanup, error } => {
                DtmFirstCancellationPreparationCleanupStep::Fault {
                    cleanup: Self::new(cleanup),
                    error,
                }
            }
            DtmFirstPreparationCleanupStep::RestoreRejected(cleanup) => {
                DtmFirstCancellationPreparationCleanupStep::RestoreRejected(Self::new(cleanup))
            }
        }
    }
}

/// Fail-stop owner for an impossible command/preparation role mismatch.
///
/// No decomposition API exists because separating the Controller from the raw
/// lower outcome would re-open role cross-wiring in safe code.
#[must_use = "an invariant fault retains the complete Controller and graph owner"]
pub struct DtmFirstInvariantFault<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    expected: crate::le::dtm::DtmRole,
    observed: crate::le::dtm::DtmRole,
    _command: DtmDeferredStart<'runtime>,
    _epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    _outcome: DtmControllerPreparationOutcome,
}

impl<S, const SCHEDULER_CAPACITY: usize> DtmFirstInvariantFault<'_, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Role required by the retained semantic HCI command.
    pub const fn expected_role(&self) -> crate::le::dtm::DtmRole {
        self.expected
    }

    /// Role carried by the impossible lower preparation outcome.
    pub const fn observed_role(&self) -> crate::le::dtm::DtmRole {
        self.observed
    }
}

/// Opaque task/graph pair rejected by the runtime idle slot.
#[must_use = "retry restoration without separating the exact task and graph"]
pub struct DtmFirstIdleRestore<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    command: DtmDeferredStart<'runtime>,
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    idle: DtmSessionIdle,
}

/// Result of retrying one exact idle-slot restoration.
#[must_use = "retain a rejected task/graph pair or consume the clean task"]
pub enum DtmFirstIdleRestoreStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// The sole DTM graph is reusable without creating response authority.
    CleanTask(DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    /// The slot still rejected the unchanged task/graph pair.
    Rejected(DtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn new(
        command: DtmDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        idle: DtmSessionIdle,
    ) -> Self {
        Self {
            command,
            task,
            idle,
        }
    }

    /// Retry one finite restore operation on the same runtime identity.
    pub fn step(mut self) -> DtmFirstIdleRestoreStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.task.restore_dtm_session_idle(self.idle) {
            Ok(()) => DtmFirstIdleRestoreStep::CleanTask(DtmFirstCancellationCleanTask {
                _owner: DtmFirstTaskOwner {
                    deferred: self.command,
                    task: self.task,
                },
                preparation_error: None,
            }),
            Err(idle) => {
                self.idle = idle;
                DtmFirstIdleRestoreStep::Rejected(self)
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
pub enum DtmFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdBegin(
        DtmFirstAcceptedFailure<
            'runtime,
            AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    ColdRecheck(
        DtmFirstAcceptedFailure<
            'runtime,
            AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    WarmBegin(
        DtmFirstAcceptedFailure<
            'runtime,
            ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    WarmRecheck(
        DtmFirstAcceptedFailure<
            'runtime,
            ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    SessionActive(
        DtmFirstAcceptedFailure<
            'runtime,
            ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        >,
    ),
    /// A failed initial preparation requires time drain and graph restoration.
    PreparationRejected(DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>),
    InvariantFault(DtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY>),
    /// A role-consistent graph retained at the exact failed transition.
    Retryable(DtmFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Result of explicitly cancelling a runner before or after head publication.
#[must_use = "cancellation must retain the recovered or irreversible owner"]
pub enum DtmFirstRunnerCancel<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    CleanTask(DtmFirstCancellationCleanTask<'runtime, S, SCHEDULER_CAPACITY>),
    CleanEpoch(DtmFirstCancellationEpoch<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsColdTimeDrain(DtmFirstColdTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsWarmTimeDrain(DtmFirstWarmTimeDrain<'runtime, S, SCHEDULER_CAPACITY>),
    NeedsPreparationCleanup(
        DtmFirstCancellationPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    RestoreRejected(DtmFirstIdleRestore<'runtime, S, SCHEDULER_CAPACITY>),
    /// A lower cancel/recheck fault retained its exact owner.
    ///
    /// The `cancel()` implementation reaches only `ColdRecheck`, `WarmRecheck`
    /// or `InvariantFault`; it never manufactures a runnable `Retryable` owner.
    Failed(DtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
    /// Cancellation was rejected or crossed published `HEAD`; no resume edge exists.
    FailStop(DtmFirstCancellationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Closed reason neutral shutdown could not recover a reusable first-event owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmFirstCancellationFailStopReason {
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
pub struct DtmFirstCancellationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    reason: DtmFirstCancellationFailStopReason,
    role: crate::le::dtm::DtmRole,
    _runner: DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstCancellationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn new(
        runner: DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
        reason: DtmFirstCancellationFailStopReason,
    ) -> Self {
        let role = runner.role();
        DtmFirstCancellationFailStop {
            reason,
            role,
            _runner: runner,
        }
    }

    /// Exact irreversible shutdown boundary.
    pub const fn reason(&self) -> DtmFirstCancellationFailStopReason {
        self.reason
    }

    /// Hardware role retained by the opaque runner.
    pub const fn role(&self) -> crate::le::dtm::DtmRole {
        self.role
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize> DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_phase(phase: DtmFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { phase }
    }

    fn role(&self) -> crate::le::dtm::DtmRole {
        match &self.phase {
            DtmFirstRunnerPhase::ColdCurrent { command, .. }
            | DtmFirstRunnerPhase::WarmCurrent { command, .. }
            | DtmFirstRunnerPhase::CurrentReady { command, .. }
            | DtmFirstRunnerPhase::Preparation { command, .. }
            | DtmFirstRunnerPhase::TransmitterPrepared { command, .. }
            | DtmFirstRunnerPhase::ReceiverPrepared { command, .. }
            | DtmFirstRunnerPhase::TransmitterHead { command, .. }
            | DtmFirstRunnerPhase::ReceiverHead { command, .. } => command.role(),
        }
    }

    /// Begin either the cold first-live or warm fresh-current acquisition.
    #[expect(
        clippy::result_large_err,
        reason = "no-alloc begin failure retains exact command and Controller owners"
    )]
    pub(crate) fn begin(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: DtmDeferredStart<'runtime>,
    ) -> Result<Self, DtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(DtmFirstRunnerPhase::WarmCurrent {
                    command,
                    pending,
                })),
                Err(failure) => Err(DtmFirstRunnerFailure::WarmBegin(
                    DtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            Err(unavailable) => {
                match unavailable
                    .into_task_service()
                    .begin_always_awake_post_enable_time()
                {
                    Ok(pending) => Ok(Self::from_phase(DtmFirstRunnerPhase::ColdCurrent {
                        command,
                        pending,
                    })),
                    Err(failure) => Err(DtmFirstRunnerFailure::ColdBegin(
                        DtmFirstAcceptedFailure::new(command, failure),
                    )),
                }
            }
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> DtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            DtmFirstRunnerPhase::ColdCurrent { command, pending } => match pending.recheck() {
                Ok(AlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                    DtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                        DtmFirstRunnerPhase::ColdCurrent { command, pending },
                    ))
                }
                Ok(AlwaysAwakePostEnableTimeStep::Ready(ready)) => DtmFirstRunnerStep::Continue(
                    Self::from_phase(DtmFirstRunnerPhase::CurrentReady {
                        command,
                        current: ready.initialize_scheduler_epoch(),
                    }),
                ),
                Err(failure) => DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::ColdRecheck(
                    DtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            DtmFirstRunnerPhase::WarmCurrent { command, pending } => match pending.recheck() {
                Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                    DtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                        DtmFirstRunnerPhase::WarmCurrent { command, pending },
                    ))
                }
                Ok(ControllerSchedulerCurrentStep::Ready(current)) => DtmFirstRunnerStep::Continue(
                    Self::from_phase(DtmFirstRunnerPhase::CurrentReady { command, current }),
                ),
                Err(failure) => DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::WarmRecheck(
                    DtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            DtmFirstRunnerPhase::CurrentReady { command, current } => {
                Self::begin_preparation(command, current)
            }
            DtmFirstRunnerPhase::Preparation { command, pending } => match pending.recheck() {
                DtmControllerPreparationStep::Continue(pending) => DtmFirstRunnerStep::Continue(
                    Self::from_phase(DtmFirstRunnerPhase::Preparation { command, pending }),
                ),
                DtmControllerPreparationStep::Pending(pending) => {
                    DtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                        DtmFirstRunnerPhase::Preparation { command, pending },
                    ))
                }
                DtmControllerPreparationStep::Terminal(terminal) => {
                    Self::finish_preparation(command, terminal)
                }
            },
            DtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => DtmFirstRunnerStep::Continue(Self::from_phase(
                    DtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner = Self::from_phase(DtmFirstRunnerPhase::TransmitterPrepared {
                        command,
                        task,
                        merged,
                    });
                    DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::Retryable(
                        DtmFirstRunnerRetry {
                            cause: DtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::le::dtm::DtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            DtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.publish_dtm_scheduler_head(merged) {
                Ok(head) => DtmFirstRunnerStep::Continue(Self::from_phase(
                    DtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    let merged = failure.into_merged();
                    let runner = Self::from_phase(DtmFirstRunnerPhase::ReceiverPrepared {
                        command,
                        task,
                        merged,
                    });
                    DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::Retryable(
                        DtmFirstRunnerRetry {
                            cause: DtmFirstRunnerRetryCause::HeadPublication(error),
                            role: crate::le::dtm::DtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
            DtmFirstRunnerPhase::TransmitterHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => DtmFirstRunnerStep::Running(DtmFirstRunning {
                    running: DtmFirstRunningPhase::Transmitter {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(DtmFirstRunnerPhase::TransmitterHead {
                        command,
                        task,
                        head,
                    });
                    DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::Retryable(
                        DtmFirstRunnerRetry {
                            cause: DtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::le::dtm::DtmRole::Transmitter,
                            runner,
                        },
                    ))
                }
            },
            DtmFirstRunnerPhase::ReceiverHead {
                command,
                mut task,
                head,
            } => match task.start_dtm_scheduler(head) {
                Ok(running) => DtmFirstRunnerStep::Running(DtmFirstRunning {
                    running: DtmFirstRunningPhase::Receiver {
                        command,
                        task,
                        running,
                    },
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    let runner = Self::from_phase(DtmFirstRunnerPhase::ReceiverHead {
                        command,
                        task,
                        head,
                    });
                    DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::Retryable(
                        DtmFirstRunnerRetry {
                            cause: DtmFirstRunnerRetryCause::SchedulerStart(error),
                            role: crate::le::dtm::DtmRole::Receiver,
                            runner,
                        },
                    ))
                }
            },
        }
    }

    fn begin_preparation(
        command: DtmDeferredStart<'runtime>,
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> DtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match &command.kind {
            DtmDeferredStartKind::Transmitter(deferred) => {
                let program = crate::le::dtm::command::transmitter_program(deferred.command());
                let result = current.begin_dtm_transmitter_first_item(
                    program.pattern,
                    program.length,
                    program.channel,
                    program.phy,
                    program.requested_interval_micros,
                );
                match result {
                    Ok(pending) => DtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                        DtmFirstRunnerPhase::Preparation { command, pending },
                    )),
                    Err(DtmControllerInitialPreparationFailure::SessionActive(current)) => {
                        DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::SessionActive(
                            DtmFirstAcceptedFailure::new(command, current),
                        ))
                    }
                    Err(DtmControllerInitialPreparationFailure::PreparationTerminal(terminal)) => {
                        Self::finish_preparation(command, terminal)
                    }
                }
            }
            DtmDeferredStartKind::Receiver(deferred) => {
                let program = crate::le::dtm::command::receiver_program(deferred.command());
                let result = current.begin_dtm_receiver_first_item(program.channel, program.phy);
                match result {
                    Ok(pending) => DtmFirstRunnerStep::WaitControllerTime(Self::from_phase(
                        DtmFirstRunnerPhase::Preparation { command, pending },
                    )),
                    Err(DtmControllerInitialPreparationFailure::SessionActive(current)) => {
                        DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::SessionActive(
                            DtmFirstAcceptedFailure::new(command, current),
                        ))
                    }
                    Err(DtmControllerInitialPreparationFailure::PreparationTerminal(terminal)) => {
                        Self::finish_preparation(command, terminal)
                    }
                }
            }
        }
    }

    fn finish_preparation(
        command: DtmDeferredStart<'runtime>,
        terminal: DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> DtmFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command.kind, outcome) {
            (
                DtmDeferredStartKind::Transmitter(deferred),
                DtmControllerPreparationOutcome::TransmitterFirst(Ok(merged)),
            ) => DtmFirstRunnerStep::Continue(Self::from_phase(
                DtmFirstRunnerPhase::TransmitterPrepared {
                    command: DtmDeferredStart::transmitter(deferred),
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                DtmDeferredStartKind::Receiver(deferred),
                DtmControllerPreparationOutcome::ReceiverFirst(Ok(merged)),
            ) => DtmFirstRunnerStep::Continue(Self::from_phase(
                DtmFirstRunnerPhase::ReceiverPrepared {
                    command: DtmDeferredStart::receiver(deferred),
                    task: epoch.into_task_service(),
                    merged,
                },
            )),
            (
                DtmDeferredStartKind::Transmitter(deferred),
                DtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::PreparationRejected(
                Self::transmitter_preparation_cleanup(deferred, epoch, failure),
            )),
            (
                DtmDeferredStartKind::Receiver(deferred),
                DtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::PreparationRejected(
                Self::receiver_preparation_cleanup(deferred, epoch, failure),
            )),
            (kind, outcome) => DtmFirstRunnerStep::Failed(DtmFirstRunnerFailure::InvariantFault(
                Self::invariant_fault(DtmDeferredStart { kind }, epoch, outcome),
            )),
        }
    }

    fn transmitter_preparation_cleanup(
        deferred: LeControllerDeferredTransmitterStart<'runtime, ()>,
        epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: DtmControllerTxPreparationFailure,
    ) -> DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _, _) = failure.into_parts();
        DtmFirstPreparationCleanup::drain(
            DtmDeferredStart::transmitter(deferred),
            epoch,
            DtmSessionIdle::new(graph),
            error,
        )
    }

    fn receiver_preparation_cleanup(
        deferred: LeControllerDeferredReceiverStart<'runtime, ()>,
        epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        failure: DtmControllerRxPreparationFailure,
    ) -> DtmFirstPreparationCleanup<'runtime, S, SCHEDULER_CAPACITY> {
        let error = failure.error();
        let (graph, _) = failure.into_owner().into_memory_and_packet_count();
        DtmFirstPreparationCleanup::drain(
            DtmDeferredStart::receiver(deferred),
            epoch,
            DtmSessionIdle::new(graph),
            error,
        )
    }

    fn cancelled_preparation(
        command: DtmDeferredStart<'runtime>,
        terminal: DtmControllerPreparationTerminal<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> DtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        match (command.kind, outcome) {
            (
                DtmDeferredStartKind::Transmitter(deferred),
                DtmControllerPreparationOutcome::TransmitterFirst(Err(failure)),
            ) => DtmFirstRunnerCancel::NeedsPreparationCleanup(
                DtmFirstCancellationPreparationCleanup::new(Self::transmitter_preparation_cleanup(
                    deferred, epoch, failure,
                )),
            ),
            (
                DtmDeferredStartKind::Receiver(deferred),
                DtmControllerPreparationOutcome::ReceiverFirst(Err(failure)),
            ) => DtmFirstRunnerCancel::NeedsPreparationCleanup(
                DtmFirstCancellationPreparationCleanup::new(Self::receiver_preparation_cleanup(
                    deferred, epoch, failure,
                )),
            ),
            (kind, outcome) => DtmFirstRunnerCancel::Failed(DtmFirstRunnerFailure::InvariantFault(
                Self::invariant_fault(DtmDeferredStart { kind }, epoch, outcome),
            )),
        }
    }

    fn invariant_fault(
        command: DtmDeferredStart<'runtime>,
        epoch: ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
        outcome: DtmControllerPreparationOutcome,
    ) -> DtmFirstInvariantFault<'runtime, S, SCHEDULER_CAPACITY> {
        let expected = command.role();
        let observed = match &outcome {
            DtmControllerPreparationOutcome::TransmitterFirst(_)
            | DtmControllerPreparationOutcome::TransmitterRecurring(_) => {
                crate::le::dtm::DtmRole::Transmitter
            }
            DtmControllerPreparationOutcome::ReceiverFirst(_)
            | DtmControllerPreparationOutcome::ReceiverRecurring(_) => {
                crate::le::dtm::DtmRole::Receiver
            }
        };
        DtmFirstInvariantFault {
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
    pub fn cancel(self) -> DtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            DtmFirstRunnerPhase::ColdCurrent { command, pending } => match pending.cancel() {
                Ok(task) => DtmFirstRunnerCancel::NeedsColdTimeDrain(DtmFirstColdTimeDrain {
                    deferred: command,
                    task,
                }),
                Err(failure) => DtmFirstRunnerCancel::Failed(DtmFirstRunnerFailure::ColdRecheck(
                    DtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            DtmFirstRunnerPhase::WarmCurrent { command, pending } => match pending.cancel() {
                Ok(epoch) => DtmFirstRunnerCancel::NeedsWarmTimeDrain(DtmFirstWarmTimeDrain {
                    deferred: command,
                    epoch,
                }),
                Err(failure) => DtmFirstRunnerCancel::Failed(DtmFirstRunnerFailure::WarmRecheck(
                    DtmFirstAcceptedFailure::new(command, failure),
                )),
            },
            DtmFirstRunnerPhase::CurrentReady { command, current } => {
                DtmFirstRunnerCancel::CleanEpoch(DtmFirstCancellationEpoch {
                    deferred: command,
                    _epoch: current.into_retained_epoch(),
                })
            }
            DtmFirstRunnerPhase::Preparation { command, pending } => {
                Self::cancelled_preparation(command, pending.cancel())
            }
            DtmFirstRunnerPhase::TransmitterPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_transmitter_first_item(merged) {
                Ok((graph, _, _)) => {
                    let idle = DtmSessionIdle::new(graph);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => DtmFirstRunnerCancel::CleanTask(DtmFirstCancellationCleanTask {
                            _owner: DtmFirstTaskOwner {
                                deferred: command,
                                task,
                            },
                            preparation_error: None,
                        }),
                        Err(idle) => DtmFirstRunnerCancel::RestoreRejected(
                            DtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => DtmFirstRunnerCancel::FailStop(DtmFirstCancellationFailStop::new(
                    Self::from_phase(DtmFirstRunnerPhase::TransmitterPrepared {
                        command,
                        task,
                        merged,
                    }),
                    DtmFirstCancellationFailStopReason::PreparedCancellationRejected,
                )),
            },
            DtmFirstRunnerPhase::ReceiverPrepared {
                command,
                mut task,
                merged,
            } => match task.cancel_dtm_receiver_first_item(merged) {
                Ok(owner) => {
                    let (graph, _) = owner.into_memory_and_packet_count();
                    let idle = DtmSessionIdle::new(graph);
                    match task.restore_dtm_session_idle(idle) {
                        Ok(()) => DtmFirstRunnerCancel::CleanTask(DtmFirstCancellationCleanTask {
                            _owner: DtmFirstTaskOwner {
                                deferred: command,
                                task,
                            },
                            preparation_error: None,
                        }),
                        Err(idle) => DtmFirstRunnerCancel::RestoreRejected(
                            DtmFirstIdleRestore::new(command, task, idle),
                        ),
                    }
                }
                Err(merged) => DtmFirstRunnerCancel::FailStop(DtmFirstCancellationFailStop::new(
                    Self::from_phase(DtmFirstRunnerPhase::ReceiverPrepared {
                        command,
                        task,
                        merged,
                    }),
                    DtmFirstCancellationFailStopReason::PreparedCancellationRejected,
                )),
            },
            head @ (DtmFirstRunnerPhase::TransmitterHead { .. }
            | DtmFirstRunnerPhase::ReceiverHead { .. }) => {
                DtmFirstRunnerCancel::FailStop(DtmFirstCancellationFailStop::new(
                    Self::from_phase(head),
                    DtmFirstCancellationFailStopReason::SchedulerHeadPublished,
                ))
            }
        }
    }
}
