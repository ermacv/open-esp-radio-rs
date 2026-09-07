//! Executor-neutral completion of one response-capable legacy advertising event.
//!
//! HCI ordering remains outside this owner. The session retains the sole task
//! service with the generic scheduler completion spine, then performs only the
//! connectable role's memory recycle, RX classification and runtime restoration.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        ControllerPublishedTaskService, SchedulerRunInterruptStorage,
        boot::SingleItemSchedulerCompletionFaultOwner,
    },
    interrupt::SchedulerWakeCell,
    le::{
        advertising::connectable::{
            LegacyConnectableAdvertisingConnectionTransfer,
            LegacyConnectableAdvertisingNoConnectionRestored,
            LegacyConnectableAdvertisingPeripheralResetCancellationFailure,
            LegacyConnectableAdvertisingPostRunFailStop,
            LegacyConnectableAdvertisingPostRunFailStopCause,
            LegacyConnectableAdvertisingPostRunOutcome,
            completion::{
                LegacyConnectableAdvertisingCompletionRole, LegacyConnectableAdvertisingRecycleStep,
            },
        },
        dtm::DtmPostUnlinkWakeCell,
        peripheral::connection::PeripheralConnectionAcceptedResetCancellationError,
    },
    scheduler::{
        BluetoothSchedulerFinishedHardwareListObserved,
        completion::{
            SingleItemCompletion, SingleItemCompletionFault, SingleItemCompletionFaultCause,
            SingleItemCompletionStep, SingleItemCompletionWaitKind,
        },
        core::{SingleItemSchedulerRunning, SingleItemSchedulerSoftwareListRemovalReady},
    },
};

pub(crate) use crate::le::advertising::connectable::LegacyConnectableAdvertisingPeripheralResetEvidence;

type CompletionRole = LegacyConnectableAdvertisingCompletionRole;
type SchedulerRunning = SingleItemSchedulerRunning<CompletionRole>;
type RemovalReady = SingleItemSchedulerSoftwareListRemovalReady<CompletionRole>;

/// Six semantic continuations for one bounded connectable-radio transition.
///
/// The bundle is behavior-only: it stores no radio owner and exactly one
/// callback receives the caller's affine context.
pub struct LegacyConnectableAdvertisingRadioContinuations<
    Continuing,
    Waiting,
    Unrelated,
    NoConnection,
    ConnectionAccepted,
    FailStop,
> {
    continuing: Continuing,
    waiting: Waiting,
    unrelated: Unrelated,
    no_connection: NoConnection,
    connection_accepted: ConnectionAccepted,
    fail_stop: FailStop,
}

impl<Continuing, Waiting, Unrelated, NoConnection, ConnectionAccepted, FailStop>
    LegacyConnectableAdvertisingRadioContinuations<
        Continuing,
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    >
{
    pub const fn new(
        continuing: Continuing,
        waiting: Waiting,
        unrelated: Unrelated,
        no_connection: NoConnection,
        connection_accepted: ConnectionAccepted,
        fail_stop: FailStop,
    ) -> Self {
        Self {
            continuing,
            waiting,
            unrelated,
            no_connection,
            connection_accepted,
            fail_stop,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Continuing,
        Waiting,
        Unrelated,
        NoConnection,
        ConnectionAccepted,
        FailStop,
    ) {
        (
            self.continuing,
            self.waiting,
            self.unrelated,
            self.no_connection,
            self.connection_accepted,
            self.fail_stop,
        )
    }
}

enum LegacyConnectableAdvertisingActivePhase {
    Completion(SingleItemCompletion<CompletionRole>),
    RemovalReady(RemovalReady),
}

/// One running connectable advertising event with no executor or HCI policy.
#[must_use = "drive the exact event to a recurrence, peripheral-transfer, or fail-stop boundary"]
pub struct LegacyConnectableAdvertisingActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    phase: LegacyConnectableAdvertisingActivePhase,
    identity: oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity,
}

/// Borrowed wake source for the current connectable completion phase.
pub enum LegacyConnectableAdvertisingActiveWait<'a> {
    Scheduler(&'a SchedulerWakeCell),
    PostUnlink(&'a DtmPostUnlinkWakeCell),
}

/// Reclaimed event with both reusable runtimes restored, awaiting recurrence policy.
#[must_use = "retain the controller and completed portable event until recurrence or stop"]
pub struct LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    completed: LegacyConnectableAdvertisingNoConnectionRestored,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.completed.identity()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, CAPACITY>,
        LegacyConnectableAdvertisingNoConnectionRestored,
    ) {
        (self.task, self.completed)
    }
}

/// Reclaimed advertising graph retaining the accepted peripheral allocation.
#[must_use = "retain the controller and accepted connection until peripheral first-event start"]
pub struct LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    transfer: LegacyConnectableAdvertisingConnectionTransfer,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn advertising_event_identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.transfer.identity()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, CAPACITY>,
        LegacyConnectableAdvertisingConnectionTransfer,
    ) {
        (self.task, self.transfer)
    }

    /// Cancel the accepted connection only for a Reset before peripheral publication.
    pub(crate) fn cancel_connection_for_reset(
        self,
    ) -> LegacyConnectableAdvertisingPeripheralResetCancellation<'runtime, S, CAPACITY> {
        let Self { mut task, transfer } = self;
        match task.cancel_legacy_connectable_advertising_connection_for_reset(transfer) {
            Ok(evidence) => LegacyConnectableAdvertisingPeripheralResetCancellation::Cancelled(
                LegacyConnectableAdvertisingPeripheralResetCancelled { task, evidence },
            ),
            Err(failure) => {
                let cause = failure.cause();
                LegacyConnectableAdvertisingPeripheralResetCancellation::FailStop(
                    LegacyConnectableAdvertisingPeripheralResetCancellationFailStop {
                        cause,
                        _task: task,
                        _failure: failure,
                    },
                )
            }
        }
    }
}

/// Explicit pre-publication Reset cancellation of an accepted connection.
#[must_use = "complete Reset with the clean task or retain the sealed owner"]
pub(crate) enum LegacyConnectableAdvertisingPeripheralResetCancellation<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Cancelled(LegacyConnectableAdvertisingPeripheralResetCancelled<'runtime, S, CAPACITY>),
    FailStop(
        LegacyConnectableAdvertisingPeripheralResetCancellationFailStop<'runtime, S, CAPACITY>,
    ),
}

/// Clean task and immutable advertising evidence after accepted-request retirement.
#[must_use = "carry the exact task and evidence through Reset completion"]
pub(crate) struct LegacyConnectableAdvertisingPeripheralResetCancelled<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    evidence: LegacyConnectableAdvertisingPeripheralResetEvidence,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingPeripheralResetCancelled<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, CAPACITY>,
        LegacyConnectableAdvertisingPeripheralResetEvidence,
    ) {
        (self.task, self.evidence)
    }
}

/// Sealed runtime mismatch retaining task, accepted connection, packet and allocation.
#[must_use = "retain every owner because Reset cancellation was not proven"]
pub(crate) struct LegacyConnectableAdvertisingPeripheralResetCancellationFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: PeripheralConnectionAcceptedResetCancellationError,
    _task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    _failure: LegacyConnectableAdvertisingPeripheralResetCancellationFailure,
}

impl<S, const CAPACITY: usize>
    LegacyConnectableAdvertisingPeripheralResetCancellationFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn cause(&self) -> PeripheralConnectionAcceptedResetCancellationError {
        self.cause
    }
}

/// Finite diagnostic for a sealed connectable post-RUN owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingActiveFailStopCause {
    FinishedListDrainAlreadyActive,
    SchedulerIdentityMismatch,
    FinishedListDrainLost,
    RepeatedRoleList,
    FinishedListDrainStillActive,
    ExpectedHardwareHeadStillPublished,
    UnexpectedHardwareHeadChanged,
    PostUnlinkMailboxBusy,
    PostUnlinkMailboxIdentityExhausted,
    PostUnlinkMailboxGenerationExhausted,
    PostUnlinkMailboxCommitMismatch,
    PostUnlinkMailboxAffinityMismatch,
    PrimaryInterruptFault,
    PostUnlinkNoSchedulerWorkRearmMismatch,
    PostUnlinkPendingRearmMismatch,
    PostUnlinkRecheckUnavailable,
    PostUnlinkRecheckRearmMismatch,
    MemoryIdentityMismatch,
    ReceiveInvalid,
    ReservationIdentityMismatch,
    ReceivePduUnavailable { discarded: usize },
    ReceivePoolIdentity,
    PacketAfterConnection,
    RuntimeGraphMismatch,
}

enum LegacyConnectableAdvertisingActiveFailStopOwner {
    Completion {
        _fault: SingleItemCompletionFault<SingleItemSchedulerCompletionFaultOwner<CompletionRole>>,
    },
    Recycle {
        _step: LegacyConnectableAdvertisingRecycleStep,
    },
    PostRun {
        _failure: LegacyConnectableAdvertisingPostRunFailStop,
    },
    NoConnectionRestore {
        _outcome: crate::le::advertising::connectable::LegacyConnectableAdvertisingNoConnection,
    },
    ConnectionRestore {
        _outcome:
            crate::le::advertising::connectable::LegacyConnectableAdvertisingConnectionAccepted,
    },
}

/// Opaque fail-stop owner retaining the exact controller and lower role state.
#[must_use = "retain the sealed owner for diagnostic shutdown"]
pub struct LegacyConnectableAdvertisingActiveFailStop<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectableAdvertisingActiveFailStopCause,
    _task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    _owner: LegacyConnectableAdvertisingActiveFailStopOwner,
}

impl<S, const CAPACITY: usize> LegacyConnectableAdvertisingActiveFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectableAdvertisingActiveFailStopCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
        running: SchedulerRunning,
    ) -> Self {
        let identity = running.item().identity();
        Self {
            task,
            phase: LegacyConnectableAdvertisingActivePhase::Completion(SingleItemCompletion::new(
                running,
            )),
            identity,
        }
    }

    pub const fn identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub fn radio_wait(&self) -> Option<LegacyConnectableAdvertisingActiveWait<'_>> {
        let LegacyConnectableAdvertisingActivePhase::Completion(completion) = &self.phase else {
            return None;
        };
        match completion.wait_kind() {
            Some(SingleItemCompletionWaitKind::Scheduler) => Some(
                LegacyConnectableAdvertisingActiveWait::Scheduler(self.task.scheduler_wake()),
            ),
            Some(SingleItemCompletionWaitKind::PostUnlink) => Some(
                LegacyConnectableAdvertisingActiveWait::PostUnlink(self.task.post_unlink_wake()),
            ),
            None => None,
        }
    }

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
        Unrelated: FnOnce(Context, Self, BluetoothSchedulerFinishedHardwareListObserved) -> R,
        NoConnection: FnOnce(
            Context,
            LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            LegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
        ) -> R,
        FailStop:
            FnOnce(Context, LegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>) -> R,
    {
        let (continuing, waiting, unrelated, no_connection, connection_accepted, fail_stop) =
            continuations.into_parts();
        let Self {
            mut task,
            phase,
            identity,
        } = self;
        match phase {
            LegacyConnectableAdvertisingActivePhase::Completion(completion) => {
                match completion.step(&mut task) {
                    SingleItemCompletionStep::Continue(completion) => continuing(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectableAdvertisingActivePhase::Completion(completion),
                            identity,
                        },
                    ),
                    SingleItemCompletionStep::Waiting(completion) => waiting(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectableAdvertisingActivePhase::Completion(completion),
                            identity,
                        },
                    ),
                    SingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => unrelated(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectableAdvertisingActivePhase::Completion(completion),
                            identity,
                        },
                        observed,
                    ),
                    SingleItemCompletionStep::RemovalReady(ready) => continuing(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectableAdvertisingActivePhase::RemovalReady(ready),
                            identity,
                        },
                    ),
                    SingleItemCompletionStep::Fault(fault) => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            completion_fault_cause(fault.cause),
                            LegacyConnectableAdvertisingActiveFailStopOwner::Completion {
                                _fault: fault,
                            },
                        ),
                    ),
                }
            }
            LegacyConnectableAdvertisingActivePhase::RemovalReady(ready) => {
                match task.recycle_legacy_connectable_advertising_completed(ready) {
                    LegacyConnectableAdvertisingRecycleStep::Classified(outcome) => {
                        match outcome {
                            LegacyConnectableAdvertisingPostRunOutcome::NoConnection(
                                outcome,
                            ) => match task
                                .restore_legacy_connectable_advertising_no_connection(outcome)
                            {
                                Ok(completed) => no_connection(
                                    context,
                                    LegacyConnectableAdvertisingAwaitingRecurrence {
                                        task,
                                        completed,
                                    },
                                ),
                                Err(outcome) => fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        LegacyConnectableAdvertisingActiveFailStopCause::RuntimeGraphMismatch,
                                        LegacyConnectableAdvertisingActiveFailStopOwner::NoConnectionRestore {
                                            _outcome: outcome,
                                        },
                                    ),
                                ),
                            },
                            LegacyConnectableAdvertisingPostRunOutcome::ConnectionAccepted(
                                outcome,
                            ) => match task.restore_legacy_connectable_advertising_connection(outcome) {
                                Ok(transfer) => connection_accepted(
                                    context,
                                    LegacyConnectableAdvertisingAwaitingPeripheralStart {
                                        task,
                                        transfer,
                                    },
                                ),
                                Err(outcome) => fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        LegacyConnectableAdvertisingActiveFailStopCause::RuntimeGraphMismatch,
                                        LegacyConnectableAdvertisingActiveFailStopOwner::ConnectionRestore {
                                            _outcome: outcome,
                                        },
                                    ),
                                ),
                            },
                            LegacyConnectableAdvertisingPostRunOutcome::FailStop(failure) => {
                                let cause = post_run_fail_stop_cause(failure.cause());
                                fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        cause,
                                        LegacyConnectableAdvertisingActiveFailStopOwner::PostRun {
                                            _failure: failure,
                                        },
                                    ),
                                )
                            }
                        }
                    }
                    step @ LegacyConnectableAdvertisingRecycleStep::SchedulerIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            LegacyConnectableAdvertisingActiveFailStopCause::SchedulerIdentityMismatch,
                            LegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ LegacyConnectableAdvertisingRecycleStep::FinishedListDrainStillActive { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            LegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainStillActive,
                            LegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ LegacyConnectableAdvertisingRecycleStep::MemoryIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            LegacyConnectableAdvertisingActiveFailStopCause::MemoryIdentityMismatch,
                            LegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ LegacyConnectableAdvertisingRecycleStep::ReceiveInvalid { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            LegacyConnectableAdvertisingActiveFailStopCause::ReceiveInvalid,
                            LegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ LegacyConnectableAdvertisingRecycleStep::ReservationIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            LegacyConnectableAdvertisingActiveFailStopCause::ReservationIdentityMismatch,
                            LegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                }
            }
        }
    }
}

fn completion_fault_cause(
    cause: SingleItemCompletionFaultCause,
) -> LegacyConnectableAdvertisingActiveFailStopCause {
    match cause {
        SingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            LegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainAlreadyActive
        }
        SingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::SchedulerIdentityMismatch
        }
        SingleItemCompletionFaultCause::FinishedListDrainLost => {
            LegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainLost
        }
        SingleItemCompletionFaultCause::RepeatedRoleList => {
            LegacyConnectableAdvertisingActiveFailStopCause::RepeatedRoleList
        }
        SingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            LegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainStillActive
        }
        SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            LegacyConnectableAdvertisingActiveFailStopCause::ExpectedHardwareHeadStillPublished
        }
        SingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            LegacyConnectableAdvertisingActiveFailStopCause::UnexpectedHardwareHeadChanged
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxBusy
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxIdentityExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxGenerationExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxCommitMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxAffinityMismatch
        }
        SingleItemCompletionFaultCause::PrimaryInterruptFault => {
            LegacyConnectableAdvertisingActiveFailStopCause::PrimaryInterruptFault
        }
        SingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkPendingRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkRecheckUnavailable
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            LegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn post_run_fail_stop_cause(
    cause: LegacyConnectableAdvertisingPostRunFailStopCause,
) -> LegacyConnectableAdvertisingActiveFailStopCause {
    match cause {
        LegacyConnectableAdvertisingPostRunFailStopCause::MemoryIdentity => {
            LegacyConnectableAdvertisingActiveFailStopCause::MemoryIdentityMismatch
        }
        LegacyConnectableAdvertisingPostRunFailStopCause::ReceivePduUnavailable { discarded } => {
            LegacyConnectableAdvertisingActiveFailStopCause::ReceivePduUnavailable { discarded }
        }
        LegacyConnectableAdvertisingPostRunFailStopCause::ReceivePoolIdentity => {
            LegacyConnectableAdvertisingActiveFailStopCause::ReceivePoolIdentity
        }
        LegacyConnectableAdvertisingPostRunFailStopCause::PacketAfterConnection => {
            LegacyConnectableAdvertisingActiveFailStopCause::PacketAfterConnection
        }
    }
}

fn active_fail_stop<'runtime, S, const CAPACITY: usize>(
    task: ControllerPublishedTaskService<'runtime, S, CAPACITY>,
    cause: LegacyConnectableAdvertisingActiveFailStopCause,
    owner: LegacyConnectableAdvertisingActiveFailStopOwner,
) -> LegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    LegacyConnectableAdvertisingActiveFailStop {
        cause,
        _task: task,
        _owner: owner,
    }
}
