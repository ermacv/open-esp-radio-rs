//! Executor-neutral handoff from accepted advertising to first peripheral RUN.
//!
//! The connectable advertising completion already owns the sole peripheral
//! allocation and the copied `CONNECT_IND`. This runner normalizes that exact
//! packet capture, acquires fresh scheduler current, prepares the first event,
//! and crosses RX/head/RUN publication without checking out another graph.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity;
use open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLeReceivedPdu, BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    BluetoothPeripheralConnectionMemoryGraphPublicationError,
};

use crate::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginError,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothLePacketStartTimingError,
    BluetoothLegacyAdvertisingEventPhase,
    BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart,
    BluetoothPeripheralConnectionCompletionStep,
    BluetoothPeripheralConnectionControllerPreparationError,
    BluetoothPeripheralConnectionSchedulerCompleted,
    BluetoothPeripheralConnectionSchedulerHeadPublished,
    BluetoothPeripheralConnectionSchedulerRecycled, BluetoothSchedulerHeadPublicationError,
    BluetoothSchedulerRunInterruptStorage,
    controller::boot::peripheral_connection::{
        BluetoothPeripheralConnectionControllerPreparationFailStop,
        BluetoothPeripheralConnectionControllerPreparationPending,
        BluetoothPeripheralConnectionControllerPreparationStep,
        BluetoothPeripheralConnectionControllerPreparationTerminal,
        BluetoothPeripheralConnectionControllerPrepared,
    },
    le::advertising::connectable::BluetoothLegacyConnectableAdvertisingConnectionTransfer,
    le::peripheral::completion::{
        BluetoothPeripheralConnectionCompletionRole, BluetoothPeripheralConnectionRecycleFailure,
        BluetoothPeripheralConnectionRecycleFailureCause,
    },
    le::peripheral::connection::BluetoothPeripheralConnectionAcceptedRequest,
    scheduler::completion::{
        BluetoothSingleItemCompletion, BluetoothSingleItemCompletionFault,
        BluetoothSingleItemCompletionFaultCause, BluetoothSingleItemCompletionStep,
        BluetoothSingleItemCompletionWaitKind,
    },
    scheduler::core::BluetoothSingleItemSchedulerSoftwareListRemovalReady,
};

struct BluetoothLegacyConnectablePeripheralOrigin {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    advertising_identity: LegacyAdvertisingEventIdentity,
    phase: BluetoothLegacyAdvertisingEventPhase,
    scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

impl BluetoothLegacyConnectablePeripheralOrigin {
    fn split(
        transfer: BluetoothLegacyConnectableAdvertisingConnectionTransfer,
    ) -> (Self, BluetoothPeripheralConnectionAcceptedRequest) {
        let (
            advertising_set,
            advertising_identity,
            accepted,
            phase,
            scheduler_status,
            rejected_packets,
        ) = transfer.into_parts();
        (
            Self {
                advertising_set,
                advertising_identity,
                phase,
                scheduler_status,
                rejected_packets,
            },
            accepted,
        )
    }
}

struct BluetoothLegacyConnectablePeripheralStartOwner {
    origin: BluetoothLegacyConnectablePeripheralOrigin,
    accepted: BluetoothPeripheralConnectionAcceptedRequest,
}

/// Fresh-controller-time wait for one accepted peripheral request.
#[must_use = "recheck the causal controller-time acquisition"]
pub struct BluetoothLegacyConnectablePeripheralFirstRunner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    owner: BluetoothLegacyConnectablePeripheralStartOwner,
    packet_start: crate::BluetoothLe1MPacketStartTiming,
}

/// First attempt to acquire fresh controller time for the accepted request.
#[must_use = "retain the time wait or sealed fail-stop owner"]
pub enum BluetoothLegacyConnectablePeripheralFirstBeginStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(
        BluetoothLegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    FailStop(BluetoothLegacyConnectablePeripheralFirstFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// One bounded fresh-current transition into controller preparation.
#[must_use = "retain the time wait, preparation transition, or fail-stop owner"]
pub enum BluetoothLegacyConnectablePeripheralFirstRunnerStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(
        BluetoothLegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Preparation(
        BluetoothLegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    FailStop(
        BluetoothLegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

/// Controller preparation still waiting on captured hardware time.
#[must_use = "recheck preparation or retain its affine owners"]
pub struct BluetoothLegacyConnectablePeripheralFirstPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pending:
        BluetoothPeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    origin: BluetoothLegacyConnectablePeripheralOrigin,
}

/// One controller-preparation transition for the accepted request.
#[must_use = "retain pending, prepared, recovered, or fail-stop ownership"]
pub enum BluetoothLegacyConnectablePeripheralFirstPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(
        BluetoothLegacyConnectablePeripheralFirstPreparationPending<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ),
    Prepared(BluetoothLegacyConnectablePeripheralFirstPrepared<'runtime, S, SCHEDULER_CAPACITY>),
    Recovered(BluetoothLegacyConnectablePeripheralFirstRecovered<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(
        BluetoothLegacyConnectablePeripheralFirstPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ),
}

/// Fully prepared first event before scheduler-head publication.
#[must_use = "publish this exact prepared memory graph"]
pub struct BluetoothLegacyConnectablePeripheralFirstPrepared<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    origin: BluetoothLegacyConnectablePeripheralOrigin,
    prepared: BluetoothPeripheralConnectionControllerPrepared,
}

/// Recoverable scheduler-head publication outcome for one prepared first event.
///
/// An irreversible publication fault is the `ControlFlow::Break` return from
/// [`BluetoothLegacyConnectablePeripheralFirstPrepared::publish`], so this sum
/// contains only states from which normal execution can continue.
#[must_use = "retain the published head or retryable preparation"]
pub enum BluetoothLegacyConnectablePeripheralFirstPublicationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    HeadPublished(
        BluetoothLegacyConnectablePeripheralFirstHeadPublished<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Retryable(BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Published first-event scheduler head before the RUN interrupt is armed.
#[must_use = "start RUN or retain the exact published head"]
pub struct BluetoothLegacyConnectablePeripheralFirstHeadPublished<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    origin: BluetoothLegacyConnectablePeripheralOrigin,
    packet: BluetoothLeReceivedPdu,
    head: BluetoothPeripheralConnectionSchedulerHeadPublished,
}

/// RUN publication of one already-published first-event head.
#[must_use = "retain the running connection or retryable published head"]
pub enum BluetoothLegacyConnectablePeripheralFirstRunStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Running(BluetoothLegacyConnectablePeripheralFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Retryable(BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First peripheral event which crossed the scheduler RUN publication edge.
#[must_use = "retain the running connection and its originating advertising evidence"]
pub struct BluetoothLegacyConnectablePeripheralFirstRunning<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: BluetoothLegacyConnectablePeripheralFirstRunningPhase,
    event_counter: u16,
    evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
}

enum BluetoothLegacyConnectablePeripheralFirstRunningPhase {
    Completion(BluetoothSingleItemCompletion<BluetoothPeripheralConnectionCompletionRole>),
    RemovalReady(
        BluetoothSingleItemSchedulerSoftwareListRemovalReady<
            BluetoothPeripheralConnectionCompletionRole,
        >,
    ),
}

/// Borrowed wake source for the first peripheral-event completion lifecycle.
pub enum BluetoothLegacyConnectablePeripheralFirstRunningWait<'a> {
    Scheduler(&'a crate::BluetoothSchedulerWakeCell),
    PostUnlink(&'a crate::BluetoothDtmPostUnlinkWakeCell),
}

/// Seven phase-typed continuations for one bounded first-peripheral radio transition.
pub struct BluetoothLegacyConnectablePeripheralFirstRunningContinuations<
    Continuing,
    Waiting,
    Unrelated,
    NormalizationUnavailable,
    Completed,
    CompletionFailStop,
    RecycleFailStop,
> {
    continuing: Continuing,
    waiting: Waiting,
    unrelated: Unrelated,
    normalization_unavailable: NormalizationUnavailable,
    completed: Completed,
    completion_fail_stop: CompletionFailStop,
    recycle_fail_stop: RecycleFailStop,
}

impl<
    Continuing,
    Waiting,
    Unrelated,
    NormalizationUnavailable,
    Completed,
    CompletionFailStop,
    RecycleFailStop,
>
    BluetoothLegacyConnectablePeripheralFirstRunningContinuations<
        Continuing,
        Waiting,
        Unrelated,
        NormalizationUnavailable,
        Completed,
        CompletionFailStop,
        RecycleFailStop,
    >
{
    pub const fn new(
        continuing: Continuing,
        waiting: Waiting,
        unrelated: Unrelated,
        normalization_unavailable: NormalizationUnavailable,
        completed: Completed,
        completion_fail_stop: CompletionFailStop,
        recycle_fail_stop: RecycleFailStop,
    ) -> Self {
        Self {
            continuing,
            waiting,
            unrelated,
            normalization_unavailable,
            completed,
            completion_fail_stop,
            recycle_fail_stop,
        }
    }

    fn into_parts(
        self,
    ) -> (
        Continuing,
        Waiting,
        Unrelated,
        NormalizationUnavailable,
        Completed,
        CompletionFailStop,
        RecycleFailStop,
    ) {
        (
            self.continuing,
            self.waiting,
            self.unrelated,
            self.normalization_unavailable,
            self.completed,
            self.completion_fail_stop,
            self.recycle_fail_stop,
        )
    }
}

/// Recycled event whose captured time cannot yet be normalized.
#[must_use = "retain the exact task, event and advertising evidence for retry"]
pub struct BluetoothLegacyConnectablePeripheralFirstNormalizationUnavailable<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    recycled: BluetoothPeripheralConnectionSchedulerRecycled,
    evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
}

/// Completed first connection event with its originating advertising evidence.
#[must_use = "retain the completed connection for recurrence or teardown"]
pub struct BluetoothLegacyConnectablePeripheralFirstCompleted<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    completed: BluetoothPeripheralConnectionSchedulerCompleted,
    evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
}

/// Finite reason the first peripheral-event owner was sealed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause {
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
}

/// Sealed task, connection graph and origin after an ownership mismatch.
#[must_use = "retain every affine owner after the fail-stop boundary"]
pub struct BluetoothLegacyConnectablePeripheralFirstCompletionFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _event_counter: u16,
    _evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
    _fault: BluetoothSingleItemCompletionFault<
        crate::controller::boot::BluetoothSingleItemSchedulerCompletionFaultOwner<
            BluetoothPeripheralConnectionCompletionRole,
        >,
    >,
}

/// Finite reason the peripheral-specific recycle tail sealed its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause {
    SchedulerIdentityMismatch,
    FinishedListDrainStillActive,
    MemoryIdentityMismatch,
    ReceiveInvalid,
    ReservationIdentityMismatch,
}

/// Sealed task and exact removal-ready graph after recycle rejection.
#[must_use = "retain every affine owner after the recycle fail-stop boundary"]
pub struct BluetoothLegacyConnectablePeripheralFirstRecycleFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _event_counter: u16,
    _evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
    _failure: BluetoothPeripheralConnectionRecycleFailure,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRecycleFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause {
        self.cause
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstCompletionFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause {
        self.cause
    }
}

/// Advertising completion and causal packet retained beside first peripheral RUN.
#[must_use = "retain the accepted-packet evidence with the first peripheral event"]
pub struct BluetoothLegacyConnectablePeripheralFirstRunningEvidence {
    origin: BluetoothLegacyConnectablePeripheralOrigin,
    packet: BluetoothLeReceivedPdu,
}

impl BluetoothLegacyConnectablePeripheralFirstRunningEvidence {
    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.origin.advertising_set
    }

    pub const fn advertising_identity(&self) -> LegacyAdvertisingEventIdentity {
        self.origin.advertising_identity
    }

    pub const fn advertising_phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.origin.phase
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.origin.scheduler_status
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.origin.rejected_packets
    }

    pub const fn accepted_packet(&self) -> &BluetoothLeReceivedPdu {
        &self.packet
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.evidence.advertising_set()
    }

    pub const fn advertising_identity(&self) -> LegacyAdvertisingEventIdentity {
        self.evidence.advertising_identity()
    }

    pub const fn advertising_phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.evidence.advertising_phase()
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.evidence.advertising_scheduler_status()
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.evidence.rejected_advertising_packets()
    }

    pub fn radio_wait(&self) -> Option<BluetoothLegacyConnectablePeripheralFirstRunningWait<'_>> {
        let BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(completion) =
            &self.phase
        else {
            return None;
        };
        match completion.wait_kind() {
            Some(BluetoothSingleItemCompletionWaitKind::Scheduler) => Some(
                BluetoothLegacyConnectablePeripheralFirstRunningWait::Scheduler(
                    self.task.scheduler_wake(),
                ),
            ),
            Some(BluetoothSingleItemCompletionWaitKind::PostUnlink) => Some(
                BluetoothLegacyConnectablePeripheralFirstRunningWait::PostUnlink(
                    self.task.post_unlink_wake(),
                ),
            ),
            None => None,
        }
    }

    /// Advance one bounded completion or peripheral-specific recycle transition.
    pub fn step_radio_with<
        R,
        Context,
        Continuing,
        Waiting,
        Unrelated,
        NormalizationUnavailable,
        Completed,
        CompletionFailStop,
        RecycleFailStop,
    >(
        self,
        context: Context,
        continuations: BluetoothLegacyConnectablePeripheralFirstRunningContinuations<
            Continuing,
            Waiting,
            Unrelated,
            NormalizationUnavailable,
            Completed,
            CompletionFailStop,
            RecycleFailStop,
        >,
    ) -> R
    where
        Continuing: FnOnce(Context, Self) -> R,
        Waiting: FnOnce(Context, Self) -> R,
        Unrelated:
            FnOnce(Context, Self, crate::BluetoothSchedulerFinishedHardwareListObserved) -> R,
        NormalizationUnavailable: FnOnce(
            Context,
            BluetoothLegacyConnectablePeripheralFirstNormalizationUnavailable<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        Completed: FnOnce(
            Context,
            BluetoothLegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>,
        ) -> R,
        CompletionFailStop: FnOnce(
            Context,
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        RecycleFailStop: FnOnce(
            Context,
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    {
        let (
            continuing,
            waiting,
            unrelated,
            normalization_unavailable,
            completed,
            completion_fail_stop,
            recycle_fail_stop,
        ) = continuations.into_parts();
        let Self {
            mut task,
            phase,
            event_counter,
            evidence,
        } = self;
        match phase {
            BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(completion) => {
                match completion.step(&mut task) {
                    BluetoothSingleItemCompletionStep::Continue(completion) => continuing(
                        context,
                        Self {
                            task,
                            phase:
                                BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(
                                    completion,
                                ),
                            event_counter,
                            evidence,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::Waiting(completion) => waiting(
                        context,
                        Self {
                            task,
                            phase:
                                BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(
                                    completion,
                                ),
                            event_counter,
                            evidence,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => unrelated(
                        context,
                        Self {
                            task,
                            phase:
                                BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(
                                    completion,
                                ),
                            event_counter,
                            evidence,
                        },
                        observed,
                    ),
                    BluetoothSingleItemCompletionStep::RemovalReady(ready) => continuing(
                        context,
                        Self {
                            task,
                            phase:
                                BluetoothLegacyConnectablePeripheralFirstRunningPhase::RemovalReady(
                                    ready,
                                ),
                            event_counter,
                            evidence,
                        },
                    ),
                    BluetoothSingleItemCompletionStep::Fault(fault) => completion_fail_stop(
                        context,
                        BluetoothLegacyConnectablePeripheralFirstCompletionFailStop {
                            cause: peripheral_completion_fault_cause(fault.cause),
                            _task: task,
                            _event_counter: event_counter,
                            _evidence: evidence,
                            _fault: fault,
                        },
                    ),
                }
            }
            BluetoothLegacyConnectablePeripheralFirstRunningPhase::RemovalReady(ready) => {
                match task.recycle_peripheral_connection_completed(ready) {
                    ControlFlow::Continue(recycled) => {
                        match task.complete_peripheral_connection_event(recycled) {
                            BluetoothPeripheralConnectionCompletionStep::SchedulerEpochUnavailable(
                                recycled,
                            ) => normalization_unavailable(
                                context,
                                BluetoothLegacyConnectablePeripheralFirstNormalizationUnavailable {
                                    task,
                                    recycled,
                                    evidence,
                                },
                            ),
                            BluetoothPeripheralConnectionCompletionStep::Completed(
                                connection,
                            ) => completed(
                                context,
                                BluetoothLegacyConnectablePeripheralFirstCompleted {
                                    task,
                                    completed: connection,
                                    evidence,
                                },
                            ),
                        }
                    }
                    ControlFlow::Break(failure) => recycle_fail_stop(
                        context,
                        peripheral_recycle_fail_stop(task, event_counter, evidence, failure),
                    ),
                }
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstNormalizationUnavailable<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    >
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn evidence(&self) -> &BluetoothLegacyConnectablePeripheralFirstRunningEvidence {
        &self.evidence
    }

    /// Retry only the scheduler-epoch-dependent capture normalization.
    pub fn retry_with<R>(
        self,
        unavailable: impl FnOnce(Self) -> R,
        completed: impl FnOnce(
            BluetoothLegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            mut task,
            recycled,
            evidence,
        } = self;
        match task.complete_peripheral_connection_event(recycled) {
            BluetoothPeripheralConnectionCompletionStep::SchedulerEpochUnavailable(recycled) => {
                unavailable(Self {
                    task,
                    recycled,
                    evidence,
                })
            }
            BluetoothPeripheralConnectionCompletionStep::Completed(connection) => {
                completed(BluetoothLegacyConnectablePeripheralFirstCompleted {
                    task,
                    completed: connection,
                    evidence,
                })
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn connection(&self) -> &BluetoothPeripheralConnectionSchedulerCompleted {
        &self.completed
    }

    pub const fn evidence(&self) -> &BluetoothLegacyConnectablePeripheralFirstRunningEvidence {
        &self.evidence
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothPeripheralConnectionSchedulerCompleted,
        BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
    ) {
        (self.task, self.completed, self.evidence)
    }
}

fn peripheral_completion_fault_cause(
    cause: BluetoothSingleItemCompletionFaultCause,
) -> BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause {
    match cause {
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainAlreadyActive
        }
        BluetoothSingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::SchedulerIdentityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainLost => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainLost
        }
        BluetoothSingleItemCompletionFaultCause::RepeatedRoleList => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::RepeatedRoleList
        }
        BluetoothSingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainStillActive
        }
        BluetoothSingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::ExpectedHardwareHeadStillPublished
        }
        BluetoothSingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::UnexpectedHardwareHeadChanged
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxBusy
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxIdentityExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxGenerationExhausted
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxCommitMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxAffinityMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PrimaryInterruptFault => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PrimaryInterruptFault
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkPendingRearmMismatch
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkRecheckUnavailable
        }
        BluetoothSingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            BluetoothLegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn peripheral_recycle_fail_stop<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    event_counter: u16,
    evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence,
    failure: BluetoothPeripheralConnectionRecycleFailure,
) -> BluetoothLegacyConnectablePeripheralFirstRecycleFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let cause = match failure.cause() {
        BluetoothPeripheralConnectionRecycleFailureCause::SchedulerIdentityMismatch => {
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause::SchedulerIdentityMismatch
        }
        BluetoothPeripheralConnectionRecycleFailureCause::FinishedListDrainStillActive => {
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause::FinishedListDrainStillActive
        }
        BluetoothPeripheralConnectionRecycleFailureCause::MemoryIdentityMismatch(_) => {
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause::MemoryIdentityMismatch
        }
        BluetoothPeripheralConnectionRecycleFailureCause::ReceiveInvalid(_) => {
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause::ReceiveInvalid
        }
        BluetoothPeripheralConnectionRecycleFailureCause::ReservationIdentityMismatch => {
            BluetoothLegacyConnectablePeripheralFirstRecycleFailStopCause::ReservationIdentityMismatch
        }
    };
    BluetoothLegacyConnectablePeripheralFirstRecycleFailStop {
        cause,
        _task: task,
        _event_counter: event_counter,
        _evidence: evidence,
        _failure: failure,
    }
}

/// Recoverable pre-publication rejection with the exact accepted request.
#[must_use = "retry the exact accepted request or retain every returned owner"]
pub struct BluetoothLegacyConnectablePeripheralFirstRecovered<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: BluetoothLegacyConnectablePeripheralStartOwner,
    error: BluetoothPeripheralConnectionControllerPreparationError,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRecovered<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> BluetoothPeripheralConnectionControllerPreparationError {
        self.error
    }

    pub fn retry(
        self,
    ) -> BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY> {
        start_with_owner(self.task, self.owner)
    }
}

enum BluetoothLegacyConnectablePeripheralFirstRetryPhase<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    HeadPublication {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        origin: BluetoothLegacyConnectablePeripheralOrigin,
        prepared: BluetoothPeripheralConnectionControllerPrepared,
        error: BluetoothSchedulerHeadPublicationError,
    },
    InterruptStorage {
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        origin: BluetoothLegacyConnectablePeripheralOrigin,
        packet: BluetoothLeReceivedPdu,
        head: BluetoothPeripheralConnectionSchedulerHeadPublished,
        error: S::Error,
    },
}

/// Exact retryable rejection before scheduler RUN.
#[must_use = "retry without rebuilding the accepted connection owner"]
pub struct BluetoothLegacyConnectablePeripheralFirstRetry<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothLegacyConnectablePeripheralFirstRetryPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Borrowed classification of one retryable first-peripheral-event failure.
pub enum BluetoothLegacyConnectablePeripheralFirstRetryCause<'error, E> {
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    InterruptStorage(&'error E),
}

/// Retried operation selected by the exact publication edge that rejected it.
#[must_use = "drive the retried publication or RUN edge"]
pub enum BluetoothLegacyConnectablePeripheralFirstRetryStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    HeadPublication(
        BluetoothLegacyConnectablePeripheralFirstPublicationStep<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    InterruptStorage(
        BluetoothLegacyConnectablePeripheralFirstRunStep<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstRetryCause<'_, S::Error> {
        match &self.phase {
            BluetoothLegacyConnectablePeripheralFirstRetryPhase::HeadPublication {
                error, ..
            } => BluetoothLegacyConnectablePeripheralFirstRetryCause::HeadPublication(*error),
            BluetoothLegacyConnectablePeripheralFirstRetryPhase::InterruptStorage {
                error, ..
            } => BluetoothLegacyConnectablePeripheralFirstRetryCause::InterruptStorage(error),
        }
    }

    pub fn retry(
        self,
    ) -> ControlFlow<
        BluetoothLegacyConnectablePeripheralFirstPublicationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
        BluetoothLegacyConnectablePeripheralFirstRetryStep<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.phase {
            BluetoothLegacyConnectablePeripheralFirstRetryPhase::HeadPublication {
                task,
                origin,
                prepared,
                ..
            } => {
                let publication = BluetoothLegacyConnectablePeripheralFirstPrepared {
                    task,
                    origin,
                    prepared,
                }
                .publish();
                match publication {
                    ControlFlow::Continue(step) => ControlFlow::Continue(
                        BluetoothLegacyConnectablePeripheralFirstRetryStep::HeadPublication(step),
                    ),
                    ControlFlow::Break(failure) => ControlFlow::Break(failure),
                }
            }
            BluetoothLegacyConnectablePeripheralFirstRetryPhase::InterruptStorage {
                task,
                origin,
                packet,
                head,
                ..
            } => ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstRetryStep::InterruptStorage(
                    BluetoothLegacyConnectablePeripheralFirstHeadPublished {
                        task,
                        origin,
                        packet,
                        head,
                    }
                    .start(),
                ),
            ),
        }
    }
}

/// Permanent-fault class for the accepted first peripheral event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectablePeripheralFirstFailStopCause {
    PacketTiming(BluetoothLePacketStartTimingError),
    SchedulerEpochUnavailable,
    CurrentBegin(BluetoothControllerSchedulerCurrentBeginError),
    Current(BluetoothControllerSchedulerCurrentError),
    PreparationControllerTime(crate::BluetoothControllerTimeAcquisitionError),
    PreparationPhaseOwnership,
    SchedulerPublication(Option<BluetoothPeripheralConnectionMemoryGraphPublicationError>),
}

enum BluetoothLegacyConnectablePeripheralFirstFailStopOwner<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Initial {
        _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _transfer: BluetoothLegacyConnectableAdvertisingConnectionTransfer,
    },
    BeforeEpoch {
        _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _owner: BluetoothLegacyConnectablePeripheralStartOwner,
    },
    BeforeCurrent {
        _owner: BluetoothLegacyConnectablePeripheralStartOwner,
        _epoch: crate::BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    },
}

/// Sealed owner after a permanent controller-time or publication fault.
#[must_use = "the fail-stop owner cannot safely return to an active controller"]
pub struct BluetoothLegacyConnectablePeripheralFirstFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause,
    _owner: BluetoothLegacyConnectablePeripheralFirstFailStopOwner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause {
        self.cause
    }
}

/// Sealed owner when fresh controller-time acquisition failed after it began.
#[must_use = "the failed current acquisition still owns the accepted request"]
pub struct BluetoothLegacyConnectablePeripheralFirstCurrentFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothControllerSchedulerCurrentError,
    _owner: BluetoothLegacyConnectablePeripheralStartOwner,
    _failure: crate::BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause {
        BluetoothLegacyConnectablePeripheralFirstFailStopCause::Current(self.cause)
    }
}

/// Sealed owner after permanent controller preparation failure.
#[must_use = "the failed preparation still owns its exact controller graph"]
pub struct BluetoothLegacyConnectablePeripheralFirstPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause,
    _origin: BluetoothLegacyConnectablePeripheralOrigin,
    _failure:
        BluetoothPeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause {
        self.cause
    }
}

/// Sealed owner after the scheduler-head publication crossed an irreversible fault.
#[must_use = "the failed publication still owns every detached graph fragment"]
pub struct BluetoothLegacyConnectablePeripheralFirstPublicationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: Option<BluetoothPeripheralConnectionMemoryGraphPublicationError>,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _origin: BluetoothLegacyConnectablePeripheralOrigin,
    _packet: BluetoothLeReceivedPdu,
    _failure: crate::BluetoothPeripheralConnectionSchedulerHeadPublicationFailure,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectablePeripheralFirstFailStopCause {
        BluetoothLegacyConnectablePeripheralFirstFailStopCause::SchedulerPublication(self.cause)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn begin(
        awaiting: BluetoothLegacyConnectableAdvertisingAwaitingPeripheralStart<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (task, transfer) = awaiting.into_parts();
        start_with_transfer(task, transfer)
    }

    pub fn step(
        self,
    ) -> BluetoothLegacyConnectablePeripheralFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            pending,
            owner,
            packet_start,
        } = self;
        match pending.recheck() {
            Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::WaitControllerTime(Self {
                    pending,
                    owner,
                    packet_start,
                })
            }
            Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                let BluetoothLegacyConnectablePeripheralStartOwner { origin, accepted } = owner;
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::Preparation(preparation_step(
                    origin,
                    current.begin_peripheral_connection_first_event(accepted, packet_start),
                ))
            }
            Err(failure) => {
                let cause = failure.error();
                BluetoothLegacyConnectablePeripheralFirstRunnerStep::FailStop(
                    BluetoothLegacyConnectablePeripheralFirstCurrentFailStop {
                        cause,
                        _owner: owner,
                        _failure: failure,
                    },
                )
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn recheck(
        self,
    ) -> BluetoothLegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
    {
        preparation_step(self.origin, self.pending.recheck())
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstPrepared<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn publish(
        self,
    ) -> ControlFlow<
        BluetoothLegacyConnectablePeripheralFirstPublicationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
        BluetoothLegacyConnectablePeripheralFirstPublicationStep<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let Self {
            mut task,
            origin,
            prepared,
        } = self;
        let BluetoothPeripheralConnectionControllerPrepared { merged, packet } = prepared;
        match task.publish_peripheral_connection_scheduler_head(merged) {
            Ok(head) => ControlFlow::Continue(
                BluetoothLegacyConnectablePeripheralFirstPublicationStep::HeadPublished(
                    BluetoothLegacyConnectablePeripheralFirstHeadPublished {
                        task,
                        origin,
                        packet,
                        head,
                    },
                ),
            ),
            Err(failure) => {
                let cause = failure.rx_publication_error();
                match failure.into_retryable_parts() {
                    Ok((error, merged)) => {
                        let retry = BluetoothLegacyConnectablePeripheralFirstRetry {
                            phase:
                                BluetoothLegacyConnectablePeripheralFirstRetryPhase::HeadPublication {
                                    task,
                                    origin,
                                    prepared: BluetoothPeripheralConnectionControllerPrepared {
                                        merged,
                                        packet,
                                    },
                                    error,
                                },
                        };
                        ControlFlow::Continue(
                            BluetoothLegacyConnectablePeripheralFirstPublicationStep::Retryable(
                                retry,
                            ),
                        )
                    }
                    Err(failure) => ControlFlow::Break(
                        BluetoothLegacyConnectablePeripheralFirstPublicationFailStop {
                            cause,
                            _task: task,
                            _origin: origin,
                            _packet: packet,
                            _failure: failure,
                        },
                    ),
                }
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstHeadPublished<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn start(
        self,
    ) -> BluetoothLegacyConnectablePeripheralFirstRunStep<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            mut task,
            origin,
            packet,
            head,
        } = self;
        let event_counter = head.event_counter();
        match task.start_peripheral_connection_scheduler(head) {
            Ok(running) => BluetoothLegacyConnectablePeripheralFirstRunStep::Running(
                BluetoothLegacyConnectablePeripheralFirstRunning {
                    task,
                    phase: BluetoothLegacyConnectablePeripheralFirstRunningPhase::Completion(
                        BluetoothSingleItemCompletion::new(running),
                    ),
                    event_counter,
                    evidence: BluetoothLegacyConnectablePeripheralFirstRunningEvidence {
                        origin,
                        packet,
                    },
                },
            ),
            Err(failure) => {
                let (error, head) = failure.into_parts();
                BluetoothLegacyConnectablePeripheralFirstRunStep::Retryable(
                    BluetoothLegacyConnectablePeripheralFirstRetry {
                        phase:
                            BluetoothLegacyConnectablePeripheralFirstRetryPhase::InterruptStorage {
                                task,
                                origin,
                                packet,
                                head,
                                error,
                            },
                    },
                )
            }
        }
    }
}

fn start_with_transfer<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    transfer: BluetoothLegacyConnectableAdvertisingConnectionTransfer,
) -> BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let packet_start = match task.normalize_le_1m_packet_start(transfer.peripheral().packet()) {
        Ok(packet_start) => packet_start,
        Err(error) => {
            return BluetoothLegacyConnectablePeripheralFirstBeginStep::FailStop(
                BluetoothLegacyConnectablePeripheralFirstFailStop {
                    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause::PacketTiming(
                        error,
                    ),
                    _owner: BluetoothLegacyConnectablePeripheralFirstFailStopOwner::Initial {
                        _task: task,
                        _transfer: transfer,
                    },
                },
            );
        }
    };
    let (origin, accepted) = BluetoothLegacyConnectablePeripheralOrigin::split(transfer);
    start_with_normalized_owner(
        task,
        BluetoothLegacyConnectablePeripheralStartOwner { origin, accepted },
        packet_start,
    )
}

fn start_with_owner<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: BluetoothLegacyConnectablePeripheralStartOwner,
) -> BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let packet_start = match task.normalize_le_1m_packet_start(owner.accepted.packet()) {
        Ok(packet_start) => packet_start,
        Err(error) => {
            return BluetoothLegacyConnectablePeripheralFirstBeginStep::FailStop(
                BluetoothLegacyConnectablePeripheralFirstFailStop {
                    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause::PacketTiming(
                        error,
                    ),
                    _owner: BluetoothLegacyConnectablePeripheralFirstFailStopOwner::BeforeEpoch {
                        _task: task,
                        _owner: owner,
                    },
                },
            );
        }
    };
    start_with_normalized_owner(task, owner, packet_start)
}

fn start_with_normalized_owner<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: BluetoothLegacyConnectablePeripheralStartOwner,
    packet_start: crate::BluetoothLe1MPacketStartTiming,
) -> BluetoothLegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let epoch = match task.retain_scheduler_epoch() {
        Ok(epoch) => epoch,
        Err(unavailable) => {
            return BluetoothLegacyConnectablePeripheralFirstBeginStep::FailStop(
                BluetoothLegacyConnectablePeripheralFirstFailStop {
                    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause::SchedulerEpochUnavailable,
                    _owner: BluetoothLegacyConnectablePeripheralFirstFailStopOwner::BeforeEpoch {
                        _task: unavailable.into_task_service(),
                        _owner: owner,
                    },
                },
            );
        }
    };
    match epoch.begin_fresh_scheduler_current() {
        Ok(pending) => BluetoothLegacyConnectablePeripheralFirstBeginStep::WaitControllerTime(
            BluetoothLegacyConnectablePeripheralFirstRunner {
                pending,
                owner,
                packet_start,
            },
        ),
        Err(failure) => {
            let error = failure.error();
            let (epoch, _) = failure.into_parts();
            BluetoothLegacyConnectablePeripheralFirstBeginStep::FailStop(
                BluetoothLegacyConnectablePeripheralFirstFailStop {
                    cause: BluetoothLegacyConnectablePeripheralFirstFailStopCause::CurrentBegin(
                        error,
                    ),
                    _owner: BluetoothLegacyConnectablePeripheralFirstFailStopOwner::BeforeCurrent {
                        _owner: owner,
                        _epoch: epoch,
                    },
                },
            )
        }
    }
}

fn preparation_step<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    origin: BluetoothLegacyConnectablePeripheralOrigin,
    step: BluetoothPeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>,
) -> BluetoothLegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match step {
        BluetoothPeripheralConnectionControllerPreparationStep::Pending(pending) => {
            BluetoothLegacyConnectablePeripheralFirstPreparationStep::WaitControllerTime(
                BluetoothLegacyConnectablePeripheralFirstPreparationPending { pending, origin },
            )
        }
        BluetoothPeripheralConnectionControllerPreparationStep::Terminal(terminal) => {
            match terminal {
                BluetoothPeripheralConnectionControllerPreparationTerminal::Prepared {
                    controller,
                    prepared,
                } => BluetoothLegacyConnectablePeripheralFirstPreparationStep::Prepared(
                    BluetoothLegacyConnectablePeripheralFirstPrepared {
                        task: controller.into_task_service(),
                        origin,
                        prepared,
                    },
                ),
                BluetoothPeripheralConnectionControllerPreparationTerminal::Recovered {
                    controller,
                    error,
                    accepted,
                } => BluetoothLegacyConnectablePeripheralFirstPreparationStep::Recovered(
                    BluetoothLegacyConnectablePeripheralFirstRecovered {
                        task: controller.into_task_service(),
                        owner: BluetoothLegacyConnectablePeripheralStartOwner { origin, accepted },
                        error,
                    },
                ),
                BluetoothPeripheralConnectionControllerPreparationTerminal::FailStop(failure) => {
                    let cause = match failure.cause() {
                        crate::controller::boot::peripheral_connection::BluetoothPeripheralConnectionControllerPreparationFailStopCause::ControllerTime(error) => {
                            BluetoothLegacyConnectablePeripheralFirstFailStopCause::PreparationControllerTime(error)
                        }
                        crate::controller::boot::peripheral_connection::BluetoothPeripheralConnectionControllerPreparationFailStopCause::PhaseOwnership => {
                            BluetoothLegacyConnectablePeripheralFirstFailStopCause::PreparationPhaseOwnership
                        }
                    };
                    BluetoothLegacyConnectablePeripheralFirstPreparationStep::FailStop(
                        BluetoothLegacyConnectablePeripheralFirstPreparationFailStop {
                            cause,
                            _origin: origin,
                            _failure: failure,
                        },
                    )
                }
            }
        }
    }
}
