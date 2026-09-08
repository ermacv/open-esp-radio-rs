//! Executor-neutral handoff from accepted advertising to first peripheral RUN.
//!
//! The connectable advertising completion already owns the sole peripheral
//! allocation and the copied `CONNECT_IND`. This runner normalizes that exact
//! packet capture, acquires fresh scheduler current, prepares the first event,
//! and crosses RX/head/RUN publication without checking out another graph.

#![forbid(unsafe_code)]

use core::ops::ControlFlow;

use crate::{
    controller::{
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginError,
        ControllerSchedulerCurrentError, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, LePacketStartTimingError,
        PeripheralConnectionCompletionStep, PeripheralConnectionControllerPreparationError,
        SchedulerRunInterruptStorage,
        boot::peripheral_connection::{
            PeripheralConnectionControllerPreparationFailStop,
            PeripheralConnectionControllerPreparationPending,
            PeripheralConnectionControllerPreparationStep,
            PeripheralConnectionControllerPreparationTerminal,
            PeripheralConnectionControllerPrepared,
        },
    },
    le::{
        advertising::{
            LegacyAdvertisingEventPhase, LegacyConnectableAdvertisingAwaitingPeripheralStart,
            connectable::LegacyConnectableAdvertisingConnectionTransfer,
        },
        peripheral::{
            completion::{
                PeripheralConnectionCompletionRole, PeripheralConnectionRecycleFailure,
                PeripheralConnectionRecycleFailureCause,
            },
            connection::PeripheralConnectionAcceptedRequest,
        },
    },
    scheduler::{
        PeripheralConnectionSchedulerCompleted, PeripheralConnectionSchedulerHeadPublished,
        PeripheralConnectionSchedulerRecycled, SchedulerHeadPublicationError,
        completion::{
            SingleItemCompletion, SingleItemCompletionFault, SingleItemCompletionFaultCause,
            SingleItemCompletionStep, SingleItemCompletionWaitKind,
        },
        core::SingleItemSchedulerSoftwareListRemovalReady,
    },
};

use oer_bluetooth_ll::{
    advertising_lifecycle::LegacyAdvertisingEventIdentity,
    connectable_advertising::LegacyConnectableAdvertisingSet,
};

use oer_esp32s31_bluetooth_memory::{
    LeReceivedPdu, LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    PeripheralConnectionMemoryGraphPublicationError,
};

struct LegacyConnectablePeripheralOrigin {
    advertising_set: LegacyConnectableAdvertisingSet<'static>,
    advertising_identity: LegacyAdvertisingEventIdentity,
    phase: LegacyAdvertisingEventPhase,
    scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

impl LegacyConnectablePeripheralOrigin {
    fn split(
        transfer: LegacyConnectableAdvertisingConnectionTransfer,
    ) -> (Self, PeripheralConnectionAcceptedRequest) {
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

struct LegacyConnectablePeripheralStartOwner {
    origin: LegacyConnectablePeripheralOrigin,
    accepted: PeripheralConnectionAcceptedRequest,
}

/// Fresh-controller-time wait for one accepted peripheral request.
#[must_use = "recheck the causal controller-time acquisition"]
pub struct LegacyConnectablePeripheralFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pending: ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    owner: LegacyConnectablePeripheralStartOwner,
    packet_start: crate::le::peripheral::Le1MPacketStartTiming,
}

/// First attempt to acquire fresh controller time for the accepted request.
#[must_use = "retain the time wait or sealed fail-stop owner"]
pub enum LegacyConnectablePeripheralFirstBeginStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(LegacyConnectablePeripheralFirstFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// One bounded fresh-current transition into controller preparation.
#[must_use = "retain the time wait, preparation transition, or fail-stop owner"]
pub enum LegacyConnectablePeripheralFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Preparation(LegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(LegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Controller preparation still waiting on captured hardware time.
#[must_use = "recheck preparation or retain its affine owners"]
pub struct LegacyConnectablePeripheralFirstPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    pending: PeripheralConnectionControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    origin: LegacyConnectablePeripheralOrigin,
}

/// One controller-preparation transition for the accepted request.
#[must_use = "retain pending, prepared, recovered, or fail-stop ownership"]
pub enum LegacyConnectablePeripheralFirstPreparationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(
        LegacyConnectablePeripheralFirstPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    Prepared(LegacyConnectablePeripheralFirstPrepared<'runtime, S, SCHEDULER_CAPACITY>),
    Recovered(LegacyConnectablePeripheralFirstRecovered<'runtime, S, SCHEDULER_CAPACITY>),
    FailStop(LegacyConnectablePeripheralFirstPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Fully prepared first event before scheduler-head publication.
#[must_use = "publish this exact prepared memory graph"]
pub struct LegacyConnectablePeripheralFirstPrepared<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    origin: LegacyConnectablePeripheralOrigin,
    prepared: PeripheralConnectionControllerPrepared,
}

/// Recoverable scheduler-head publication outcome for one prepared first event.
///
/// An irreversible publication fault is the `ControlFlow::Break` return from
/// [`LegacyConnectablePeripheralFirstPrepared::publish`], so this sum
/// contains only states from which normal execution can continue.
#[must_use = "retain the published head or retryable preparation"]
pub enum LegacyConnectablePeripheralFirstPublicationStep<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    HeadPublished(LegacyConnectablePeripheralFirstHeadPublished<'runtime, S, SCHEDULER_CAPACITY>),
    Retryable(BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Published first-event scheduler head before the RUN interrupt is armed.
#[must_use = "start RUN or retain the exact published head"]
pub struct LegacyConnectablePeripheralFirstHeadPublished<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    origin: LegacyConnectablePeripheralOrigin,
    packet: LeReceivedPdu,
    head: PeripheralConnectionSchedulerHeadPublished,
}

/// RUN publication of one already-published first-event head.
#[must_use = "retain the running connection or retryable published head"]
pub enum LegacyConnectablePeripheralFirstRunStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Running(LegacyConnectablePeripheralFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Retryable(BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First peripheral event which crossed the scheduler RUN publication edge.
#[must_use = "retain the running connection and its originating advertising evidence"]
pub struct LegacyConnectablePeripheralFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    phase: LegacyConnectablePeripheralFirstRunningPhase,
    event_counter: u16,
    evidence: LegacyConnectablePeripheralFirstRunningEvidence,
}

enum LegacyConnectablePeripheralFirstRunningPhase {
    Completion(SingleItemCompletion<PeripheralConnectionCompletionRole>),
    RemovalReady(SingleItemSchedulerSoftwareListRemovalReady<PeripheralConnectionCompletionRole>),
}

/// Borrowed wake source for the first peripheral-event completion lifecycle.
pub enum LegacyConnectablePeripheralFirstRunningWait<'a> {
    Scheduler(&'a crate::interrupt::SchedulerWakeCell),
    PostUnlink(&'a crate::le::dtm::DtmPostUnlinkWakeCell),
}

/// Seven phase-typed continuations for one bounded first-peripheral radio transition.
pub struct LegacyConnectablePeripheralFirstRunningContinuations<
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
    LegacyConnectablePeripheralFirstRunningContinuations<
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
pub struct LegacyConnectablePeripheralFirstNormalizationUnavailable<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    recycled: PeripheralConnectionSchedulerRecycled,
    evidence: LegacyConnectablePeripheralFirstRunningEvidence,
}

/// Completed first connection event with its originating advertising evidence.
#[must_use = "retain the completed connection for recurrence or teardown"]
pub struct LegacyConnectablePeripheralFirstCompleted<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    completed: PeripheralConnectionSchedulerCompleted,
    evidence: LegacyConnectablePeripheralFirstRunningEvidence,
}

/// Finite reason the first peripheral-event owner was sealed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstCompletionFailStopCause {
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
pub struct LegacyConnectablePeripheralFirstCompletionFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectablePeripheralFirstCompletionFailStopCause,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _event_counter: u16,
    _evidence: LegacyConnectablePeripheralFirstRunningEvidence,
    _fault: SingleItemCompletionFault<
        crate::controller::boot::SingleItemSchedulerCompletionFaultOwner<
            PeripheralConnectionCompletionRole,
        >,
    >,
}

/// Finite reason the peripheral-specific recycle tail sealed its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstRecycleFailStopCause {
    SchedulerIdentityMismatch,
    FinishedListDrainStillActive,
    MemoryIdentityMismatch,
    ReceiveInvalid,
    ReservationIdentityMismatch,
}

/// Sealed task and exact removal-ready graph after recycle rejection.
#[must_use = "retain every affine owner after the recycle fail-stop boundary"]
pub struct LegacyConnectablePeripheralFirstRecycleFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectablePeripheralFirstRecycleFailStopCause,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _event_counter: u16,
    _evidence: LegacyConnectablePeripheralFirstRunningEvidence,
    _failure: PeripheralConnectionRecycleFailure,
}

impl<S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstRecycleFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstRecycleFailStopCause {
        self.cause
    }
}

impl<S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstCompletionFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstCompletionFailStopCause {
        self.cause
    }
}

/// Advertising completion and causal packet retained beside first peripheral RUN.
#[must_use = "retain the accepted-packet evidence with the first peripheral event"]
pub struct LegacyConnectablePeripheralFirstRunningEvidence {
    origin: LegacyConnectablePeripheralOrigin,
    packet: LeReceivedPdu,
}

impl LegacyConnectablePeripheralFirstRunningEvidence {
    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.origin.advertising_set
    }

    pub const fn advertising_identity(&self) -> LegacyAdvertisingEventIdentity {
        self.origin.advertising_identity
    }

    pub const fn advertising_phase(&self) -> LegacyAdvertisingEventPhase {
        self.origin.phase
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.origin.scheduler_status
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.origin.rejected_packets
    }

    pub const fn accepted_packet(&self) -> &LeReceivedPdu {
        &self.packet
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Reuse the same completion/recycle lifecycle after a recurring RUN.
    pub(super) fn from_recurring(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        running: crate::scheduler::core::SingleItemSchedulerRunning<
            PeripheralConnectionCompletionRole,
        >,
        event_counter: u16,
        evidence: LegacyConnectablePeripheralFirstRunningEvidence,
    ) -> Self {
        Self {
            task,
            phase: LegacyConnectablePeripheralFirstRunningPhase::Completion(
                SingleItemCompletion::new(running),
            ),
            event_counter,
            evidence,
        }
    }

    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    pub const fn advertising_set(&self) -> LegacyConnectableAdvertisingSet<'static> {
        self.evidence.advertising_set()
    }

    pub const fn advertising_identity(&self) -> LegacyAdvertisingEventIdentity {
        self.evidence.advertising_identity()
    }

    pub const fn advertising_phase(&self) -> LegacyAdvertisingEventPhase {
        self.evidence.advertising_phase()
    }

    pub const fn advertising_scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.evidence.advertising_scheduler_status()
    }

    pub const fn rejected_advertising_packets(&self) -> usize {
        self.evidence.rejected_advertising_packets()
    }

    pub fn radio_wait(&self) -> Option<LegacyConnectablePeripheralFirstRunningWait<'_>> {
        let LegacyConnectablePeripheralFirstRunningPhase::Completion(completion) = &self.phase
        else {
            return None;
        };
        match completion.wait_kind() {
            Some(SingleItemCompletionWaitKind::Scheduler) => Some(
                LegacyConnectablePeripheralFirstRunningWait::Scheduler(self.task.scheduler_wake()),
            ),
            Some(SingleItemCompletionWaitKind::PostUnlink) => {
                Some(LegacyConnectablePeripheralFirstRunningWait::PostUnlink(
                    self.task.post_unlink_wake(),
                ))
            }
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
        continuations: LegacyConnectablePeripheralFirstRunningContinuations<
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
        Unrelated: FnOnce(
            Context,
            Self,
            crate::scheduler::BluetoothSchedulerFinishedHardwareListObserved,
        ) -> R,
        NormalizationUnavailable: FnOnce(
            Context,
            LegacyConnectablePeripheralFirstNormalizationUnavailable<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        Completed: FnOnce(
            Context,
            LegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>,
        ) -> R,
        CompletionFailStop: FnOnce(
            Context,
            LegacyConnectablePeripheralFirstCompletionFailStop<'runtime, S, SCHEDULER_CAPACITY>,
        ) -> R,
        RecycleFailStop: FnOnce(
            Context,
            LegacyConnectablePeripheralFirstRecycleFailStop<'runtime, S, SCHEDULER_CAPACITY>,
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
            LegacyConnectablePeripheralFirstRunningPhase::Completion(completion) => {
                match completion.step(&mut task) {
                    SingleItemCompletionStep::Continue(completion) => continuing(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectablePeripheralFirstRunningPhase::Completion(
                                completion,
                            ),
                            event_counter,
                            evidence,
                        },
                    ),
                    SingleItemCompletionStep::Waiting(completion) => waiting(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectablePeripheralFirstRunningPhase::Completion(
                                completion,
                            ),
                            event_counter,
                            evidence,
                        },
                    ),
                    SingleItemCompletionStep::UnrelatedList {
                        completion,
                        observed,
                    } => unrelated(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectablePeripheralFirstRunningPhase::Completion(
                                completion,
                            ),
                            event_counter,
                            evidence,
                        },
                        observed,
                    ),
                    SingleItemCompletionStep::RemovalReady(ready) => continuing(
                        context,
                        Self {
                            task,
                            phase: LegacyConnectablePeripheralFirstRunningPhase::RemovalReady(
                                ready,
                            ),
                            event_counter,
                            evidence,
                        },
                    ),
                    SingleItemCompletionStep::Fault(fault) => completion_fail_stop(
                        context,
                        LegacyConnectablePeripheralFirstCompletionFailStop {
                            cause: peripheral_completion_fault_cause(fault.cause),
                            _task: task,
                            _event_counter: event_counter,
                            _evidence: evidence,
                            _fault: fault,
                        },
                    ),
                }
            }
            LegacyConnectablePeripheralFirstRunningPhase::RemovalReady(ready) => {
                match task.recycle_peripheral_connection_completed(ready) {
                    ControlFlow::Continue(recycled) => {
                        match task.complete_peripheral_connection_event(recycled) {
                            PeripheralConnectionCompletionStep::SchedulerEpochUnavailable(
                                recycled,
                            ) => normalization_unavailable(
                                context,
                                LegacyConnectablePeripheralFirstNormalizationUnavailable {
                                    task,
                                    recycled,
                                    evidence,
                                },
                            ),
                            PeripheralConnectionCompletionStep::Completed(connection) => completed(
                                context,
                                LegacyConnectablePeripheralFirstCompleted {
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
    LegacyConnectablePeripheralFirstNormalizationUnavailable<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn evidence(&self) -> &LegacyConnectablePeripheralFirstRunningEvidence {
        &self.evidence
    }

    /// Retry only the scheduler-epoch-dependent capture normalization.
    pub fn retry_with<R>(
        self,
        unavailable: impl FnOnce(Self) -> R,
        completed: impl FnOnce(
            LegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            mut task,
            recycled,
            evidence,
        } = self;
        match task.complete_peripheral_connection_event(recycled) {
            PeripheralConnectionCompletionStep::SchedulerEpochUnavailable(recycled) => {
                unavailable(Self {
                    task,
                    recycled,
                    evidence,
                })
            }
            PeripheralConnectionCompletionStep::Completed(connection) => {
                completed(LegacyConnectablePeripheralFirstCompleted {
                    task,
                    completed: connection,
                    evidence,
                })
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstCompleted<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn connection(&self) -> &PeripheralConnectionSchedulerCompleted {
        &self.completed
    }

    pub const fn evidence(&self) -> &LegacyConnectablePeripheralFirstRunningEvidence {
        &self.evidence
    }

    pub fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        PeripheralConnectionSchedulerCompleted,
        LegacyConnectablePeripheralFirstRunningEvidence,
    ) {
        (self.task, self.completed, self.evidence)
    }
}

fn peripheral_completion_fault_cause(
    cause: SingleItemCompletionFaultCause,
) -> LegacyConnectablePeripheralFirstCompletionFailStopCause {
    match cause {
        SingleItemCompletionFaultCause::FinishedListDrainAlreadyActive => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainAlreadyActive
        }
        SingleItemCompletionFaultCause::SchedulerIdentityMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::SchedulerIdentityMismatch
        }
        SingleItemCompletionFaultCause::FinishedListDrainLost => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainLost
        }
        SingleItemCompletionFaultCause::RepeatedRoleList => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::RepeatedRoleList
        }
        SingleItemCompletionFaultCause::FinishedListDrainStillActive => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::FinishedListDrainStillActive
        }
        SingleItemCompletionFaultCause::ExpectedHardwareHeadStillPublished => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::ExpectedHardwareHeadStillPublished
        }
        SingleItemCompletionFaultCause::UnexpectedHardwareHeadChanged => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::UnexpectedHardwareHeadChanged
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxBusy => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxBusy
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxIdentityExhausted => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxIdentityExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxGenerationExhausted => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxGenerationExhausted
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxCommitMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxCommitMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkMailboxAffinityMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkMailboxAffinityMismatch
        }
        SingleItemCompletionFaultCause::PrimaryInterruptFault => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PrimaryInterruptFault
        }
        SingleItemCompletionFaultCause::PostUnlinkNoSchedulerWorkRearmMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkNoSchedulerWorkRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkPendingRearmMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkPendingRearmMismatch
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckUnavailable => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkRecheckUnavailable
        }
        SingleItemCompletionFaultCause::PostUnlinkRecheckRearmMismatch => {
            LegacyConnectablePeripheralFirstCompletionFailStopCause::PostUnlinkRecheckRearmMismatch
        }
    }
}

fn peripheral_recycle_fail_stop<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    event_counter: u16,
    evidence: LegacyConnectablePeripheralFirstRunningEvidence,
    failure: PeripheralConnectionRecycleFailure,
) -> LegacyConnectablePeripheralFirstRecycleFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let cause = match failure.cause() {
        PeripheralConnectionRecycleFailureCause::SchedulerIdentityMismatch => {
            LegacyConnectablePeripheralFirstRecycleFailStopCause::SchedulerIdentityMismatch
        }
        PeripheralConnectionRecycleFailureCause::FinishedListDrainStillActive => {
            LegacyConnectablePeripheralFirstRecycleFailStopCause::FinishedListDrainStillActive
        }
        PeripheralConnectionRecycleFailureCause::MemoryIdentityMismatch(_) => {
            LegacyConnectablePeripheralFirstRecycleFailStopCause::MemoryIdentityMismatch
        }
        PeripheralConnectionRecycleFailureCause::ReceiveInvalid(_) => {
            LegacyConnectablePeripheralFirstRecycleFailStopCause::ReceiveInvalid
        }
        PeripheralConnectionRecycleFailureCause::ReservationIdentityMismatch => {
            LegacyConnectablePeripheralFirstRecycleFailStopCause::ReservationIdentityMismatch
        }
    };
    LegacyConnectablePeripheralFirstRecycleFailStop {
        cause,
        _task: task,
        _event_counter: event_counter,
        _evidence: evidence,
        _failure: failure,
    }
}

/// Recoverable pre-publication rejection with the exact accepted request.
#[must_use = "retry the exact accepted request or retain every returned owner"]
pub struct LegacyConnectablePeripheralFirstRecovered<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: LegacyConnectablePeripheralStartOwner,
    error: PeripheralConnectionControllerPreparationError,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstRecovered<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn error(&self) -> PeripheralConnectionControllerPreparationError {
        self.error
    }

    pub fn retry(
        self,
    ) -> LegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY> {
        start_with_owner(self.task, self.owner)
    }
}

enum LegacyConnectablePeripheralFirstRetryPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    HeadPublication {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        origin: LegacyConnectablePeripheralOrigin,
        prepared: PeripheralConnectionControllerPrepared,
        error: SchedulerHeadPublicationError,
    },
    InterruptStorage {
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        origin: LegacyConnectablePeripheralOrigin,
        packet: LeReceivedPdu,
        head: PeripheralConnectionSchedulerHeadPublished,
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
    S: SchedulerRunInterruptStorage,
{
    phase: LegacyConnectablePeripheralFirstRetryPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Borrowed classification of one retryable first-peripheral-event failure.
pub enum LegacyConnectablePeripheralFirstRetryCause<'error, E> {
    HeadPublication(SchedulerHeadPublicationError),
    InterruptStorage(&'error E),
}

/// Retried operation selected by the exact publication edge that rejected it.
#[must_use = "drive the retried publication or RUN edge"]
pub enum LegacyConnectablePeripheralFirstRetryStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    HeadPublication(
        LegacyConnectablePeripheralFirstPublicationStep<'runtime, S, SCHEDULER_CAPACITY>,
    ),
    InterruptStorage(LegacyConnectablePeripheralFirstRunStep<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectablePeripheralFirstRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cause(&self) -> LegacyConnectablePeripheralFirstRetryCause<'_, S::Error> {
        match &self.phase {
            LegacyConnectablePeripheralFirstRetryPhase::HeadPublication { error, .. } => {
                LegacyConnectablePeripheralFirstRetryCause::HeadPublication(*error)
            }
            LegacyConnectablePeripheralFirstRetryPhase::InterruptStorage { error, .. } => {
                LegacyConnectablePeripheralFirstRetryCause::InterruptStorage(error)
            }
        }
    }

    pub fn retry(
        self,
    ) -> ControlFlow<
        LegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyConnectablePeripheralFirstRetryStep<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        match self.phase {
            LegacyConnectablePeripheralFirstRetryPhase::HeadPublication {
                task,
                origin,
                prepared,
                ..
            } => {
                let publication = LegacyConnectablePeripheralFirstPrepared {
                    task,
                    origin,
                    prepared,
                }
                .publish();
                match publication {
                    ControlFlow::Continue(step) => ControlFlow::Continue(
                        LegacyConnectablePeripheralFirstRetryStep::HeadPublication(step),
                    ),
                    ControlFlow::Break(failure) => ControlFlow::Break(failure),
                }
            }
            LegacyConnectablePeripheralFirstRetryPhase::InterruptStorage {
                task,
                origin,
                packet,
                head,
                ..
            } => {
                ControlFlow::Continue(LegacyConnectablePeripheralFirstRetryStep::InterruptStorage(
                    LegacyConnectablePeripheralFirstHeadPublished {
                        task,
                        origin,
                        packet,
                        head,
                    }
                    .start(),
                ))
            }
        }
    }
}

/// Permanent-fault class for the accepted first peripheral event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectablePeripheralFirstFailStopCause {
    PacketTiming(LePacketStartTimingError),
    SchedulerEpochUnavailable,
    CurrentBegin(ControllerSchedulerCurrentBeginError),
    Current(ControllerSchedulerCurrentError),
    PreparationControllerTime(crate::scheduler::ControllerTimeAcquisitionError),
    PreparationPhaseOwnership,
    SchedulerPublication(Option<PeripheralConnectionMemoryGraphPublicationError>),
}

enum LegacyConnectablePeripheralFirstFailStopOwner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Initial {
        _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _transfer: LegacyConnectableAdvertisingConnectionTransfer,
    },
    BeforeEpoch {
        _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _owner: LegacyConnectablePeripheralStartOwner,
    },
    BeforeCurrent {
        _owner: LegacyConnectablePeripheralStartOwner,
        _epoch:
            crate::controller::ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
    },
}

/// Sealed owner after a permanent controller-time or publication fault.
#[must_use = "the fail-stop owner cannot safely return to an active controller"]
pub struct LegacyConnectablePeripheralFirstFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectablePeripheralFirstFailStopCause,
    _owner: LegacyConnectablePeripheralFirstFailStopOwner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstFailStopCause {
        self.cause
    }
}

/// Sealed owner when fresh controller-time acquisition failed after it began.
#[must_use = "the failed current acquisition still owns the accepted request"]
pub struct LegacyConnectablePeripheralFirstCurrentFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: ControllerSchedulerCurrentError,
    _owner: LegacyConnectablePeripheralStartOwner,
    _failure: crate::controller::ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstCurrentFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstFailStopCause {
        LegacyConnectablePeripheralFirstFailStopCause::Current(self.cause)
    }
}

/// Sealed owner after permanent controller preparation failure.
#[must_use = "the failed preparation still owns its exact controller graph"]
pub struct LegacyConnectablePeripheralFirstPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyConnectablePeripheralFirstFailStopCause,
    _origin: LegacyConnectablePeripheralOrigin,
    _failure: PeripheralConnectionControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstFailStopCause {
        self.cause
    }
}

/// Sealed owner after the scheduler-head publication crossed an irreversible fault.
#[must_use = "the failed publication still owns every detached graph fragment"]
pub struct LegacyConnectablePeripheralFirstPublicationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    cause: Option<PeripheralConnectionMemoryGraphPublicationError>,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    _origin: LegacyConnectablePeripheralOrigin,
    _packet: LeReceivedPdu,
    _failure: crate::scheduler::PeripheralConnectionSchedulerHeadPublicationFailure,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectablePeripheralFirstFailStopCause {
        LegacyConnectablePeripheralFirstFailStopCause::SchedulerPublication(self.cause)
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn begin(
        awaiting: LegacyConnectableAdvertisingAwaitingPeripheralStart<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> LegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (task, transfer) = awaiting.into_parts();
        start_with_transfer(task, transfer)
    }

    pub fn step(
        self,
    ) -> LegacyConnectablePeripheralFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            pending,
            owner,
            packet_start,
        } = self;
        match pending.recheck() {
            Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                LegacyConnectablePeripheralFirstRunnerStep::WaitControllerTime(Self {
                    pending,
                    owner,
                    packet_start,
                })
            }
            Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                let LegacyConnectablePeripheralStartOwner { origin, accepted } = owner;
                LegacyConnectablePeripheralFirstRunnerStep::Preparation(preparation_step(
                    origin,
                    current.begin_peripheral_connection_first_event(accepted, packet_start),
                ))
            }
            Err(failure) => {
                let cause = failure.error();
                LegacyConnectablePeripheralFirstRunnerStep::FailStop(
                    LegacyConnectablePeripheralFirstCurrentFailStop {
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
    LegacyConnectablePeripheralFirstPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn recheck(
        self,
    ) -> LegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY> {
        preparation_step(self.origin, self.pending.recheck())
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstPrepared<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn publish(
        self,
    ) -> ControlFlow<
        LegacyConnectablePeripheralFirstPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyConnectablePeripheralFirstPublicationStep<'runtime, S, SCHEDULER_CAPACITY>,
    > {
        let Self {
            mut task,
            origin,
            prepared,
        } = self;
        let PeripheralConnectionControllerPrepared { merged, packet } = prepared;
        match task.publish_peripheral_connection_scheduler_head(merged) {
            Ok(head) => ControlFlow::Continue(
                LegacyConnectablePeripheralFirstPublicationStep::HeadPublished(
                    LegacyConnectablePeripheralFirstHeadPublished {
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
                            phase: LegacyConnectablePeripheralFirstRetryPhase::HeadPublication {
                                task,
                                origin,
                                prepared: PeripheralConnectionControllerPrepared { merged, packet },
                                error,
                            },
                        };
                        ControlFlow::Continue(
                            LegacyConnectablePeripheralFirstPublicationStep::Retryable(retry),
                        )
                    }
                    Err(failure) => {
                        ControlFlow::Break(LegacyConnectablePeripheralFirstPublicationFailStop {
                            cause,
                            _task: task,
                            _origin: origin,
                            _packet: packet,
                            _failure: failure,
                        })
                    }
                }
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectablePeripheralFirstHeadPublished<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn start(self) -> LegacyConnectablePeripheralFirstRunStep<'runtime, S, SCHEDULER_CAPACITY> {
        let Self {
            mut task,
            origin,
            packet,
            head,
        } = self;
        let event_counter = head.event_counter();
        match task.start_peripheral_connection_scheduler(head) {
            Ok(running) => LegacyConnectablePeripheralFirstRunStep::Running(
                LegacyConnectablePeripheralFirstRunning {
                    task,
                    phase: LegacyConnectablePeripheralFirstRunningPhase::Completion(
                        SingleItemCompletion::new(running),
                    ),
                    event_counter,
                    evidence: LegacyConnectablePeripheralFirstRunningEvidence { origin, packet },
                },
            ),
            Err(failure) => {
                let (error, head) = failure.into_parts();
                LegacyConnectablePeripheralFirstRunStep::Retryable(
                    BluetoothLegacyConnectablePeripheralFirstRetry {
                        phase: LegacyConnectablePeripheralFirstRetryPhase::InterruptStorage {
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
    mut task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    transfer: LegacyConnectableAdvertisingConnectionTransfer,
) -> LegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let packet_start = match task.normalize_le_1m_packet_start(transfer.peripheral().packet()) {
        Ok(packet_start) => packet_start,
        Err(error) => {
            return LegacyConnectablePeripheralFirstBeginStep::FailStop(
                LegacyConnectablePeripheralFirstFailStop {
                    cause: LegacyConnectablePeripheralFirstFailStopCause::PacketTiming(error),
                    _owner: LegacyConnectablePeripheralFirstFailStopOwner::Initial {
                        _task: task,
                        _transfer: transfer,
                    },
                },
            );
        }
    };
    let (origin, accepted) = LegacyConnectablePeripheralOrigin::split(transfer);
    start_with_normalized_owner(
        task,
        LegacyConnectablePeripheralStartOwner { origin, accepted },
        packet_start,
    )
}

fn start_with_owner<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: LegacyConnectablePeripheralStartOwner,
) -> LegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let packet_start = match task.normalize_le_1m_packet_start(owner.accepted.packet()) {
        Ok(packet_start) => packet_start,
        Err(error) => {
            return LegacyConnectablePeripheralFirstBeginStep::FailStop(
                LegacyConnectablePeripheralFirstFailStop {
                    cause: LegacyConnectablePeripheralFirstFailStopCause::PacketTiming(error),
                    _owner: LegacyConnectablePeripheralFirstFailStopOwner::BeforeEpoch {
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
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    owner: LegacyConnectablePeripheralStartOwner,
    packet_start: crate::le::peripheral::Le1MPacketStartTiming,
) -> LegacyConnectablePeripheralFirstBeginStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let epoch = match task.retain_scheduler_epoch() {
        Ok(epoch) => epoch,
        Err(unavailable) => {
            return LegacyConnectablePeripheralFirstBeginStep::FailStop(
                LegacyConnectablePeripheralFirstFailStop {
                    cause: LegacyConnectablePeripheralFirstFailStopCause::SchedulerEpochUnavailable,
                    _owner: LegacyConnectablePeripheralFirstFailStopOwner::BeforeEpoch {
                        _task: unavailable.into_task_service(),
                        _owner: owner,
                    },
                },
            );
        }
    };
    match epoch.begin_fresh_scheduler_current() {
        Ok(pending) => LegacyConnectablePeripheralFirstBeginStep::WaitControllerTime(
            LegacyConnectablePeripheralFirstRunner {
                pending,
                owner,
                packet_start,
            },
        ),
        Err(failure) => {
            let error = failure.error();
            let (epoch, _) = failure.into_parts();
            LegacyConnectablePeripheralFirstBeginStep::FailStop(
                LegacyConnectablePeripheralFirstFailStop {
                    cause: LegacyConnectablePeripheralFirstFailStopCause::CurrentBegin(error),
                    _owner: LegacyConnectablePeripheralFirstFailStopOwner::BeforeCurrent {
                        _owner: owner,
                        _epoch: epoch,
                    },
                },
            )
        }
    }
}

fn preparation_step<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    origin: LegacyConnectablePeripheralOrigin,
    step: PeripheralConnectionControllerPreparationStep<'runtime, S, SCHEDULER_CAPACITY>,
) -> LegacyConnectablePeripheralFirstPreparationStep<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match step {
        PeripheralConnectionControllerPreparationStep::Pending(pending) => {
            LegacyConnectablePeripheralFirstPreparationStep::WaitControllerTime(
                LegacyConnectablePeripheralFirstPreparationPending { pending, origin },
            )
        }
        PeripheralConnectionControllerPreparationStep::Terminal(terminal) => match terminal {
            PeripheralConnectionControllerPreparationTerminal::Prepared {
                controller,
                prepared,
            } => LegacyConnectablePeripheralFirstPreparationStep::Prepared(
                LegacyConnectablePeripheralFirstPrepared {
                    task: controller.into_task_service(),
                    origin,
                    prepared,
                },
            ),
            PeripheralConnectionControllerPreparationTerminal::Recovered {
                controller,
                error,
                accepted,
            } => LegacyConnectablePeripheralFirstPreparationStep::Recovered(
                LegacyConnectablePeripheralFirstRecovered {
                    task: controller.into_task_service(),
                    owner: LegacyConnectablePeripheralStartOwner { origin, accepted },
                    error,
                },
            ),
            PeripheralConnectionControllerPreparationTerminal::FailStop(failure) => {
                let cause = match failure.cause() {
                        crate::controller::boot::peripheral_connection::PeripheralConnectionControllerPreparationFailStopCause::ControllerTime(error) => {
                            LegacyConnectablePeripheralFirstFailStopCause::PreparationControllerTime(error)
                        }
                        crate::controller::boot::peripheral_connection::PeripheralConnectionControllerPreparationFailStopCause::PhaseOwnership => {
                            LegacyConnectablePeripheralFirstFailStopCause::PreparationPhaseOwnership
                        }
                    };
                LegacyConnectablePeripheralFirstPreparationStep::FailStop(
                    LegacyConnectablePeripheralFirstPreparationFailStop {
                        cause,
                        _origin: origin,
                        _failure: failure,
                    },
                )
            }
        },
    }
}
