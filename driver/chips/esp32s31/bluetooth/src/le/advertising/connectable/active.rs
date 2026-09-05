//! Executor-neutral completion of one response-capable legacy advertising event.
//!
//! HCI ordering remains outside this owner. The session retains the sole task
//! service with the generic scheduler completion spine, then performs only the
//! connectable role's memory recycle, RX classification and runtime restoration.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerPublishedTaskService, BluetoothDtmPostUnlinkWakeCell,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerWakeCell,
    controller::boot::BluetoothSingleItemSchedulerCompletionFaultOwner,
    le::advertising::connectable::completion::{
        BluetoothLegacyConnectableAdvertisingCompletionRole,
        BluetoothLegacyConnectableAdvertisingRecycleStep,
    },
    le::advertising::connectable::{
        BluetoothLegacyConnectableAdvertisingConnectionTransfer,
        BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
        BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure,
        BluetoothLegacyConnectableAdvertisingPostRunFailStop,
        BluetoothLegacyConnectableAdvertisingPostRunFailStopCause,
        BluetoothLegacyConnectableAdvertisingPostRunOutcome,
    },
    le::peripheral::connection::BluetoothPeripheralConnectionAcceptedResetCancellationError,
    scheduler::completion::{
        BluetoothSingleItemCompletion, BluetoothSingleItemCompletionFault,
        BluetoothSingleItemCompletionFaultCause, BluetoothSingleItemCompletionStep,
        BluetoothSingleItemCompletionWaitKind,
    },
    scheduler::core::{
        BluetoothSingleItemSchedulerRunning, BluetoothSingleItemSchedulerSoftwareListRemovalReady,
    },
};

pub(crate) use crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence;

type CompletionRole = BluetoothLegacyConnectableAdvertisingCompletionRole;
type SchedulerRunning = BluetoothSingleItemSchedulerRunning<CompletionRole>;
type RemovalReady = BluetoothSingleItemSchedulerSoftwareListRemovalReady<CompletionRole>;

/// Six semantic continuations for one bounded connectable-radio transition.
///
/// The bundle is behavior-only: it stores no radio owner and exactly one
/// callback receives the caller's affine context.
pub struct BluetoothLegacyConnectableAdvertisingRadioContinuations<
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
    BluetoothLegacyConnectableAdvertisingRadioContinuations<
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

enum BluetoothLegacyConnectableAdvertisingActivePhase {
    Completion(BluetoothSingleItemCompletion<CompletionRole>),
    RemovalReady(RemovalReady),
}

/// One running connectable advertising event with no executor or HCI policy.
#[must_use = "drive the exact event to a recurrence, peripheral-transfer, or fail-stop boundary"]
pub struct BluetoothLegacyConnectableAdvertisingActiveSession<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    phase: BluetoothLegacyConnectableAdvertisingActivePhase,
    identity: open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity,
}

/// Borrowed wake source for the current connectable completion phase.
pub enum BluetoothLegacyConnectableAdvertisingActiveWait<'a> {
    Scheduler(&'a BluetoothSchedulerWakeCell),
    PostUnlink(&'a BluetoothDtmPostUnlinkWakeCell),
}

/// Reclaimed event with both reusable runtimes restored, awaiting recurrence policy.
#[must_use = "retain the controller and completed portable event until recurrence or stop"]
pub struct BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    completed: BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.completed.identity()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
    ) {
        (self.task, self.completed)
    }
}

/// Reclaimed advertising graph retaining the accepted peripheral allocation.
#[must_use = "retain the controller and accepted connection until peripheral first-event start"]
pub struct BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    transfer: BluetoothLegacyConnectableAdvertisingConnectionTransfer,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn advertising_event_identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.transfer.identity()
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingConnectionTransfer,
    ) {
        (self.task, self.transfer)
    }

    /// Cancel the accepted connection only for a Reset before peripheral publication.
    pub(crate) fn cancel_connection_for_reset(
        self,
    ) -> BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation<'runtime, S, CAPACITY>
    {
        let Self { mut task, transfer } = self;
        match task.cancel_legacy_connectable_advertising_connection_for_reset(transfer) {
            Ok(evidence) => {
                BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation::Cancelled(
                    BluetoothLegacyConnectableAdvertisingPeripheralResetCancelled {
                        task,
                        evidence,
                    },
                )
            }
            Err(failure) => {
                let cause = failure.cause();
                BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation::FailStop(
                    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop {
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
pub(crate) enum BluetoothLegacyConnectableAdvertisingPeripheralResetCancellation<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Cancelled(BluetoothLegacyConnectableAdvertisingPeripheralResetCancelled<'runtime, S, CAPACITY>),
    FailStop(
        BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop<
            'runtime,
            S,
            CAPACITY,
        >,
    ),
}

/// Clean task and immutable advertising evidence after accepted-request retirement.
#[must_use = "carry the exact task and evidence through Reset completion"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPeripheralResetCancelled<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    evidence: BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingPeripheralResetCancelled<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingPeripheralResetEvidence,
    ) {
        (self.task, self.evidence)
    }
}

/// Sealed runtime mismatch retaining task, accepted connection, packet and allocation.
#[must_use = "retain every owner because Reset cancellation was not proven"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothPeripheralConnectionAcceptedResetCancellationError,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    _failure: BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailure,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingPeripheralResetCancellationFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn cause(
        &self,
    ) -> BluetoothPeripheralConnectionAcceptedResetCancellationError {
        self.cause
    }
}

/// Finite diagnostic for a sealed connectable post-RUN owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
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

enum BluetoothLegacyConnectableAdvertisingActiveFailStopOwner {
    Completion {
        _fault: BluetoothSingleItemCompletionFault<
            BluetoothSingleItemSchedulerCompletionFaultOwner<CompletionRole>,
        >,
    },
    Recycle {
        _step: BluetoothLegacyConnectableAdvertisingRecycleStep,
    },
    PostRun {
        _failure: BluetoothLegacyConnectableAdvertisingPostRunFailStop,
    },
    NoConnectionRestore {
        _outcome: crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingNoConnection,
    },
    ConnectionRestore {
        _outcome:
            crate::le::advertising::connectable::BluetoothLegacyConnectableAdvertisingConnectionAccepted,
    },
}

/// Opaque fail-stop owner retaining the exact controller and lower role state.
#[must_use = "retain the sealed owner for diagnostic shutdown"]
pub struct BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectableAdvertisingActiveFailStopCause,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    _owner: BluetoothLegacyConnectableAdvertisingActiveFailStopOwner,
}

impl<S, const CAPACITY: usize> BluetoothLegacyConnectableAdvertisingActiveFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn new(
        task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
        running: SchedulerRunning,
    ) -> Self {
        let identity = running.item().identity();
        Self {
            task,
            phase: BluetoothLegacyConnectableAdvertisingActivePhase::Completion(
                BluetoothSingleItemCompletion::new(running),
            ),
            identity,
        }
    }

    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.identity
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyConnectableAdvertisingActiveWait<'_>> {
        let BluetoothLegacyConnectableAdvertisingActivePhase::Completion(completion) = &self.phase
        else {
            return None;
        };
        match completion.wait_kind() {
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler) => {
                Some(BluetoothLegacyConnectableAdvertisingActiveWait::Scheduler(
                    self.task.scheduler_wake(),
                ))
            }
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink) => {
                Some(BluetoothLegacyConnectableAdvertisingActiveWait::PostUnlink(
                    self.task.post_unlink_wake(),
                ))
            }
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
        Unrelated: FnOnce(Context, Self, BluetoothSchedulerFinishedHardwareListObserved) -> R,
        NoConnection: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>,
        ) -> R,
        ConnectionAccepted: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<'runtime, S, CAPACITY>,
        ) -> R,
        FailStop: FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    {
        let (continuing, waiting, unrelated, no_connection, connection_accepted, fail_stop) =
            continuations.into_parts();
        let Self {
            mut task,
            phase,
            identity,
        } = self;
        match phase {
            BluetoothLegacyConnectableAdvertisingActivePhase::Completion(completion) => {
                match completion.step(&mut task) {
                    BluetoothSingleItemCompletionStep::Continue(completion) => continuing(
                        context,
                        Self {
                            task,
                            phase: BluetoothLegacyConnectableAdvertisingActivePhase::Completion(
                                completion,
                            ),
                            identity,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::Waiting(completion) => waiting(
                        context,
                        Self {
                            task,
                            phase: BluetoothLegacyConnectableAdvertisingActivePhase::Completion(
                                completion,
                            ),
                            identity,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => unrelated(
                        context,
                        Self {
                            task,
                            phase: BluetoothLegacyConnectableAdvertisingActivePhase::Completion(
                                completion,
                            ),
                            identity,
                        },
                        observed,
                    ),
                    BluetoothSingleItemCompletionStep::RemovalReady(ready) => continuing(
                        context,
                        Self {
                            task,
                            phase: BluetoothLegacyConnectableAdvertisingActivePhase::RemovalReady(
                                ready,
                            ),
                            identity,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::Fault(fault) => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            completion_fault_cause(fault.cause),
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Completion {
                                _fault: fault,
                            },
                        ),
                    ),
                }
            }
            BluetoothLegacyConnectableAdvertisingActivePhase::RemovalReady(ready) => {
                match task.recycle_legacy_connectable_advertising_completed(ready) {
                    BluetoothLegacyConnectableAdvertisingRecycleStep::Classified(outcome) => {
                        match outcome {
                            BluetoothLegacyConnectableAdvertisingPostRunOutcome::NoConnection(
                                outcome,
                            ) => match task
                                .restore_legacy_connectable_advertising_no_connection(outcome)
                            {
                                Ok(completed) => no_connection(
                                    context,
                                    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence {
                                        task,
                                        completed,
                                    },
                                ),
                                Err(outcome) => fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        BluetoothLegacyConnectableAdvertisingActiveFailStopCause::RuntimeGraphMismatch,
                                        BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::NoConnectionRestore {
                                            _outcome: outcome,
                                        },
                                    ),
                                ),
                            },
                            BluetoothLegacyConnectableAdvertisingPostRunOutcome::ConnectionAccepted(
                                outcome,
                            ) => match task.restore_legacy_connectable_advertising_connection(outcome) {
                                Ok(transfer) => connection_accepted(
                                    context,
                                    BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart {
                                        task,
                                        transfer,
                                    },
                                ),
                                Err(outcome) => fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        BluetoothLegacyConnectableAdvertisingActiveFailStopCause::RuntimeGraphMismatch,
                                        BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::ConnectionRestore {
                                            _outcome: outcome,
                                        },
                                    ),
                                ),
                            },
                            BluetoothLegacyConnectableAdvertisingPostRunOutcome::FailStop(failure) => {
                                let cause = post_run_fail_stop_cause(failure.cause());
                                fail_stop(
                                    context,
                                    active_fail_stop(
                                        task,
                                        cause,
                                        BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::PostRun {
                                            _failure: failure,
                                        },
                                    ),
                                )
                            }
                        }
                    }
                    step @ BluetoothLegacyConnectableAdvertisingRecycleStep::SchedulerIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::SchedulerIdentityMismatch,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ BluetoothLegacyConnectableAdvertisingRecycleStep::FinishedListDrainStillActive { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainStillActive,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ BluetoothLegacyConnectableAdvertisingRecycleStep::MemoryIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::MemoryIdentityMismatch,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ BluetoothLegacyConnectableAdvertisingRecycleStep::ReceiveInvalid { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::ReceiveInvalid,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                    step @ BluetoothLegacyConnectableAdvertisingRecycleStep::ReservationIdentityMismatch { .. } => fail_stop(
                        context,
                        active_fail_stop(
                            task,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::ReservationIdentityMismatch,
                            BluetoothLegacyConnectableAdvertisingActiveFailStopOwner::Recycle { _step: step },
                        ),
                    ),
                }
            }
        }
    }
}

fn completion_fault_cause(
    cause: BluetoothSingleItemCompletionFaultCause,
) -> BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
    match cause {
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainAlreadyActive
        }
        BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::SchedulerIdentityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainLost
        }
        BluetoothSingleItemCompletionFaultCause::RepeatedRoleList => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::RepeatedRoleList
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::FinishedListDrainStillActive
        }
        BluetoothSingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::ExpectedHardwareHeadStillPublished
        }
        BluetoothSingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::UnexpectedHardwareHeadChanged
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxBusy
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxIdentityExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxGenerationExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxCommitMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkMailboxAffinityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PrimaryInterruptFault => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PrimaryInterruptFault
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkPendingRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkRecheckUnavailable
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn post_run_fail_stop_cause(
    cause: BluetoothLegacyConnectableAdvertisingPostRunFailStopCause,
) -> BluetoothLegacyConnectableAdvertisingActiveFailStopCause {
    match cause {
        BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::MemoryIdentity => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::MemoryIdentityMismatch
        }
        BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::ReceivePduUnavailable {
            discarded,
        } => BluetoothLegacyConnectableAdvertisingActiveFailStopCause::ReceivePduUnavailable {
            discarded,
        },
        BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::ReceivePoolIdentity => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::ReceivePoolIdentity
        }
        BluetoothLegacyConnectableAdvertisingPostRunFailStopCause::PacketAfterConnection => {
            BluetoothLegacyConnectableAdvertisingActiveFailStopCause::PacketAfterConnection
        }
    }
}

fn active_fail_stop<'runtime, S, const CAPACITY: usize>(
    task: BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>,
    cause: BluetoothLegacyConnectableAdvertisingActiveFailStopCause,
    owner: BluetoothLegacyConnectableAdvertisingActiveFailStopOwner,
) -> BluetoothLegacyConnectableAdvertisingActiveFailStop<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    BluetoothLegacyConnectableAdvertisingActiveFailStop {
        cause,
        _task: task,
        _owner: owner,
    }
}
