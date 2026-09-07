//! Phase-typed forward preparation for one connectable advertising successor.

#![forbid(unsafe_code)]

use crate::{
    ControllerTimeSample,
    controller::{
        SchedulerRunInterruptStorage,
        boot::{
            LegacyConnectableAdvertisingSchedulerFailStopCause,
            LegacyConnectableAdvertisingSchedulerStartRetryError,
            LegacyConnectableAdvertisingSchedulerStartStep,
            timed_preparation::{
                TimedPreparationPending, TimedPreparationRollbackOutcome, TimedPreparationStep,
            },
        },
    },
    le::advertising::connectable::{
        LegacyConnectableAdvertisingEventCandidate, LegacyConnectableAdvertisingNextEventPortable,
        LegacyConnectableAdvertisingPrepared, active::LegacyConnectableAdvertisingActiveSession,
    },
    scheduler::core::{
        LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        LegacyConnectableAdvertisingEventPrepared, LegacyConnectableAdvertisingPreSequence,
    },
};

use super::{
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    LegacyConnectableAdvertisingRecurrenceContext,
    LegacyConnectableAdvertisingRecurrenceRollbackFailed,
    LegacyConnectableAdvertisingRecurrenceTimedController,
    LegacyConnectableAdvertisingRecurringFailStopCause,
    LegacyConnectableAdvertisingRecurringFailStopOwner, LegacyConnectableAdvertisingRecurringRetry,
    LegacyConnectableAdvertisingRecurringRetryCause, Task, event_retry_cause,
    runtime_fail_stop_parts, timed_fail_stop,
};

type SequencePendingCore<'runtime, S, const CAPACITY: usize> = TimedPreparationPending<
    LegacyConnectableAdvertisingRecurrenceTimedController<'runtime, S, CAPACITY>,
    LegacyConnectableAdvertisingPreSequence,
    LegacyConnectableAdvertisingRecurrenceRollbackFailed,
>;

/// Portable successor waiting for its static role graphs and timing projection.
#[must_use = "prepare or cancel the exact scheduled successor"]
pub struct LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) portable: LegacyConnectableAdvertisingNextEventPortable,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) const fn new(
        context: LegacyConnectableAdvertisingRecurrenceContext,
        task: Task<'runtime, S, CAPACITY>,
        portable: LegacyConnectableAdvertisingNextEventPortable,
    ) -> Self {
        Self {
            context,
            task,
            portable,
        }
    }

    /// Check out both restored role graphs and form the phase-locked window.
    pub fn prepare_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurringRetry<
                LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
                S::Error,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            context,
            mut task,
            portable,
        } = self;
        let event = match portable {
            LegacyConnectableAdvertisingNextEventPortable::Event(event) => event,
            portable @ LegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(_) => {
                return fail_stop(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                        cause: LegacyConnectableAdvertisingRecurringFailStopCause::EventSequenceExhausted,
                        context,
                        _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Scheduled {
                            _task: task,
                            _portable: portable,
                        },
                    },
                );
            }
        };
        let Some(timing) = task.legacy_connectable_advertising_recurring_timing() else {
            return fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause: LegacyConnectableAdvertisingRecurringFailStopCause::SchedulerEpochUnavailable,
                context,
                _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Scheduled {
                    _task: task,
                    _portable: LegacyConnectableAdvertisingNextEventPortable::Event(event),
                },
            });
        };
        let prepared = match task
            .begin_legacy_connectable_advertising_scheduled_event(context.definition, event)
        {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (cause, failure) = runtime_fail_stop_parts(failure);
                return fail_stop(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                        cause,
                        context,
                        _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Runtime {
                            _task: task,
                            _failure: failure,
                        },
                    },
                );
            }
        };
        match prepared.form_recurring_event_candidate(
            timing,
            context.previous_phase,
            context.start_offset_micros,
            task.legacy_connectable_advertising_scheduler_config(),
        ) {
            Ok(candidate) => ready(
                continuation,
                LegacyConnectableAdvertisingRecurrenceCandidate {
                    context,
                    task,
                    candidate,
                },
            ),
            Err(failure) => retry(
                continuation,
                LegacyConnectableAdvertisingRecurringRetry::new(
                    LegacyConnectableAdvertisingRecurringRetryCause::TimingWindow,
                    LegacyConnectableAdvertisingRecurrenceGraphPrepared {
                        context,
                        task,
                        prepared: failure.into_prepared(),
                    },
                ),
            ),
        }
    }
}

/// Restored role graphs waiting to retry the same timing projection.
#[must_use = "retry timing or cancel the exact graph"]
pub struct LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) prepared: LegacyConnectableAdvertisingPrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Retry only the pure timing projection; no role owner is recreated.
    pub fn retry_timing_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, LegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            context,
            task,
            prepared,
        } = self;
        let Some(timing) = task.legacy_connectable_advertising_recurring_timing() else {
            return fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause: LegacyConnectableAdvertisingRecurringFailStopCause::SchedulerEpochUnavailable,
                context,
                _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::GraphPrepared {
                    _task: task,
                    _prepared: prepared,
                },
            });
        };
        match prepared.form_recurring_event_candidate(
            timing,
            context.previous_phase,
            context.start_offset_micros,
            task.legacy_connectable_advertising_scheduler_config(),
        ) {
            Ok(candidate) => ready(
                continuation,
                LegacyConnectableAdvertisingRecurrenceCandidate {
                    context,
                    task,
                    candidate,
                },
            ),
            Err(failure) => retry(
                continuation,
                LegacyConnectableAdvertisingRecurringRetry::new(
                    LegacyConnectableAdvertisingRecurringRetryCause::TimingWindow,
                    Self {
                        context,
                        task,
                        prepared: failure.into_prepared(),
                    },
                ),
            ),
        }
    }
}

/// Complete event candidate waiting for exact recurring timeline admission.
#[must_use = "begin sequence acquisition or cancel the exact candidate"]
pub struct LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) candidate: LegacyConnectableAdvertisingEventCandidate,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Reserve the exact phase and publish one fresh Controller-time request.
    pub fn begin_sequence_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, LegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            context,
            mut task,
            candidate,
        } = self;
        let admitted = match task.admit_legacy_connectable_advertising_recurring_event(candidate) {
            Ok(admitted) => admitted,
            Err(failure) => {
                let cause = event_retry_cause(failure.error());
                return retry(
                    continuation,
                    LegacyConnectableAdvertisingRecurringRetry::new(
                        cause,
                        Self {
                            context,
                            task,
                            candidate: failure.into_candidate(),
                        },
                    ),
                );
            }
        };
        let controller = LegacyConnectableAdvertisingRecurrenceTimedController {
            task,
            rollback: None,
        };
        match SequencePendingCore::begin(controller, admitted, |controller, admitted| {
            let cancelled = controller
                .task
                .cancel_legacy_connectable_advertising_recurring_pre_sequence(admitted);
            match controller
                .task
                .restore_legacy_connectable_advertising_cancelled_in_place(cancelled)
            {
                TimedPreparationRollbackOutcome::Restored => {
                    TimedPreparationRollbackOutcome::Restored
                }
                TimedPreparationRollbackOutcome::FailStop(rollback) => {
                    controller.rollback = Some(rollback);
                    TimedPreparationRollbackOutcome::FailStop(
                        LegacyConnectableAdvertisingRecurrenceRollbackFailed,
                    )
                }
            }
        }) {
            Ok(pending) => waiting(
                continuation,
                LegacyConnectableAdvertisingRecurrenceSequencePending { context, pending },
            ),
            Err(failure) => fail_stop(continuation, timed_fail_stop(context, failure)),
        }
    }
}

/// One in-flight fresh sequence sample.
#[must_use = "recheck or cancel the exact Controller-time request"]
pub struct LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) pending: SequencePendingCore<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Observe exactly one bounded Controller-time completion edge.
    pub fn recheck_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(C, Self) -> R,
        ready: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self.pending.recheck() {
            TimedPreparationStep::Waiting(pending) => waiting(
                continuation,
                Self {
                    context: self.context,
                    pending,
                },
            ),
            TimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => match controller.rollback {
                None => ready(
                    continuation,
                    LegacyConnectableAdvertisingRecurrenceSequenceReady {
                        context: self.context,
                        task: controller.task,
                        admitted: phase,
                        sample,
                    },
                ),
                Some(rollback) => fail_stop(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                        cause: LegacyConnectableAdvertisingRecurringFailStopCause::PhaseOwnership,
                        context: self.context,
                        _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Rollback {
                            _task: controller.task,
                            _rollback: rollback,
                        },
                    },
                ),
            },
            TimedPreparationStep::FailStop(failure) => {
                fail_stop(continuation, timed_fail_stop(self.context, failure))
            }
        }
    }
}

/// Fresh sample paired with the exact admitted recurrence.
#[must_use = "authorize the event fields or cancel the admitted recurrence"]
pub struct LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) admitted: LegacyConnectableAdvertisingPreSequence,
    pub(super) sample: ControllerTimeSample,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Authorize the sample and apply the accepted event window to CPU-owned SRAM.
    pub fn prepare_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurringRetry<
                LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
                S::Error,
            >,
        ) -> R,
    ) -> R {
        let Self {
            context,
            mut task,
            admitted,
            sample,
        } = self;
        match task.prepare_legacy_connectable_advertising_recurring_event(admitted, sample) {
            Ok(prepared) => ready(
                continuation,
                LegacyConnectableAdvertisingRecurrencePrepared {
                    context,
                    task,
                    prepared,
                },
            ),
            Err(failure) => {
                let cause = event_retry_cause(failure.error());
                retry(
                    continuation,
                    LegacyConnectableAdvertisingRecurringRetry::new(
                        cause,
                        LegacyConnectableAdvertisingRecurrenceCandidate {
                            context,
                            task,
                            candidate: failure.into_candidate(),
                        },
                    ),
                )
            }
        }
    }
}

/// Sequence-ready event waiting for the exclusive empty-list join.
#[must_use = "merge, retry, or cancel the exact prepared recurrence"]
pub struct LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) prepared: LegacyConnectableAdvertisingEventPrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Join the sole event to the current exact empty scheduler list.
    pub fn merge_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(C, LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>) -> R,
        retry: impl FnOnce(C, LegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
    ) -> R {
        let Self {
            context,
            mut task,
            prepared,
        } = self;
        match task.merge_legacy_connectable_advertising_recurring_event(prepared) {
            Ok(merged) => ready(
                continuation,
                LegacyConnectableAdvertisingRecurrenceMerged {
                    context,
                    task,
                    merged,
                },
            ),
            Err(failure) => retry(
                continuation,
                LegacyConnectableAdvertisingRecurringRetry::new(
                    LegacyConnectableAdvertisingRecurringRetryCause::EmptyList(failure.error()),
                    Self {
                        context,
                        task,
                        prepared: failure.into_prepared(),
                    },
                ),
            ),
        }
    }
}

/// Exact unpublished list join immediately before the atomic MMIO suffix.
#[must_use = "start, retry, or cancel the exact merged recurrence"]
pub struct LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    pub(super) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) merged: LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Execute the existing atomic RX/HEAD/RUN suffix.
    pub fn start_with<C, R>(
        self,
        continuation: C,
        running: impl FnOnce(C, LegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>) -> R,
        retry: impl FnOnce(C, LegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let Self {
            context,
            task,
            merged,
        } = self;
        match task.start_legacy_connectable_advertising_scheduler(merged) {
            LegacyConnectableAdvertisingSchedulerStartStep::Running {
                controller,
                running: running_graph,
            } => running(
                continuation,
                LegacyConnectableAdvertisingActiveSession::new(controller, running_graph),
            ),
            LegacyConnectableAdvertisingSchedulerStartStep::Retryable { failure } => {
                let (task, merged, error) = failure.into_parts();
                let cause = match error {
                    LegacyConnectableAdvertisingSchedulerStartRetryError::Head(error) => {
                        LegacyConnectableAdvertisingRecurringRetryCause::SchedulerHead(error)
                    }
                    LegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(error) => {
                        LegacyConnectableAdvertisingRecurringRetryCause::SchedulerInterrupts(error)
                    }
                };
                retry(
                    continuation,
                    LegacyConnectableAdvertisingRecurringRetry::new(
                        cause,
                        Self {
                            context,
                            task,
                            merged,
                        },
                    ),
                )
            }
            LegacyConnectableAdvertisingSchedulerStartStep::FailStop(failure) => {
                let cause = match failure.cause() {
                    LegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(
                        error,
                    ) => LegacyConnectableAdvertisingRecurringFailStopCause::ReceivePublication(
                        error,
                    ),
                    LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(error) => {
                        LegacyConnectableAdvertisingRecurringFailStopCause::SchedulerHeadPublication(
                            error,
                        )
                    }
                    LegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(error) => {
                        LegacyConnectableAdvertisingRecurringFailStopCause::SchedulerRunPublication(
                            error,
                        )
                    }
                };
                fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                    cause,
                    context,
                    _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::SchedulerPublication {
                        _failure: failure,
                    },
                })
            }
        }
    }
}
