//! Phase-typed forward preparation for one connectable advertising successor.

#![forbid(unsafe_code)]

use crate::{
    BluetoothControllerTimeSample, BluetoothSchedulerRunInterruptStorage,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingNextEventPortable,
        BluetoothLegacyConnectableAdvertisingPrepared,
    },
    controller_start::{
        BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause,
        BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError,
        BluetoothLegacyConnectableAdvertisingSchedulerStartStep,
        timed_preparation::{
            BluetoothTimedPreparationPending, BluetoothTimedPreparationRollbackOutcome,
            BluetoothTimedPreparationStep,
        },
    },
    legacy_connectable_advertising_active::BluetoothLegacyConnectableAdvertisingActiveSession,
    scheduler::{
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothLegacyConnectableAdvertisingEventPrepared,
        BluetoothLegacyConnectableAdvertisingPreSequence,
    },
};

use super::{
    BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed,
    BluetoothLegacyConnectableAdvertisingRecurrenceTimedController,
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner,
    BluetoothLegacyConnectableAdvertisingRecurringRetry,
    BluetoothLegacyConnectableAdvertisingRecurringRetryCause, Task, event_retry_cause,
    runtime_fail_stop_parts, timed_fail_stop,
};

type SequencePendingCore<'runtime, S, const CAPACITY: usize> = BluetoothTimedPreparationPending<
    BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<'runtime, S, CAPACITY>,
    BluetoothLegacyConnectableAdvertisingPreSequence,
    BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed,
>;

/// Portable successor waiting for its static role graphs and timing projection.
#[must_use = "prepare or cancel the exact scheduled successor"]
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) portable: BluetoothLegacyConnectableAdvertisingNextEventPortable,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) const fn new(
        context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
        task: Task<'runtime, S, CAPACITY>,
        portable: BluetoothLegacyConnectableAdvertisingNextEventPortable,
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
            BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>,
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
            BluetoothLegacyConnectableAdvertisingNextEventPortable::Event(event) => event,
            portable
            @ BluetoothLegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(
                _,
            ) => {
                return fail_stop(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                        cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::EventSequenceExhausted,
                        context,
                        _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Scheduled {
                            _task: task,
                            _portable: portable,
                        },
                    },
                );
            }
        };
        let Some(timing) = task.legacy_connectable_advertising_recurring_timing() else {
            return fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::SchedulerEpochUnavailable,
                context,
                _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Scheduled {
                    _task: task,
                    _portable: BluetoothLegacyConnectableAdvertisingNextEventPortable::Event(event),
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
                        _owner:
                            BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Runtime {
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
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate {
                    context,
                    task,
                    candidate,
                },
            ),
            Err(failure) => retry(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
                    BluetoothLegacyConnectableAdvertisingRecurringRetryCause::TimingWindow,
                    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared {
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) prepared: BluetoothLegacyConnectableAdvertisingPrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Retry only the pure timing projection; no role owner is recreated.
    pub fn retry_timing_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, BluetoothLegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
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
                cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::SchedulerEpochUnavailable,
                context,
                _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::GraphPrepared {
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
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate {
                    context,
                    task,
                    candidate,
                },
            ),
            Err(failure) => retry(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
                    BluetoothLegacyConnectableAdvertisingRecurringRetryCause::TimingWindow,
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) candidate: BluetoothLegacyConnectableAdvertisingEventCandidate,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Reserve the exact phase and publish one fresh Controller-time request.
    pub fn begin_sequence_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, BluetoothLegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
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
                    BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
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
        let controller = BluetoothLegacyConnectableAdvertisingRecurrenceTimedController {
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
                BluetoothTimedPreparationRollbackOutcome::Restored => {
                    BluetoothTimedPreparationRollbackOutcome::Restored
                }
                BluetoothTimedPreparationRollbackOutcome::FailStop(rollback) => {
                    controller.rollback = Some(rollback);
                    BluetoothTimedPreparationRollbackOutcome::FailStop(
                        BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed,
                    )
                }
            }
        }) {
            Ok(pending) => waiting(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending { context, pending },
            ),
            Err(failure) => fail_stop(continuation, timed_fail_stop(context, failure)),
        }
    }
}

/// One in-flight fresh sequence sample.
#[must_use = "recheck or cancel the exact Controller-time request"]
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) pending: SequencePendingCore<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Observe exactly one bounded Controller-time completion edge.
    pub fn recheck_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(C, Self) -> R,
        ready: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self.pending.recheck() {
            BluetoothTimedPreparationStep::Waiting(pending) => waiting(continuation, Self {
                context: self.context,
                pending,
            }),
            BluetoothTimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => match controller.rollback {
                None => ready(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady {
                        context: self.context,
                        task: controller.task,
                        admitted: phase,
                        sample,
                    },
                ),
                Some(rollback) => fail_stop(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                        cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::PhaseOwnership,
                        context: self.context,
                        _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Rollback {
                            _task: controller.task,
                            _rollback: rollback,
                        },
                    },
                ),
            },
            BluetoothTimedPreparationStep::FailStop(failure) => {
                fail_stop(continuation, timed_fail_stop(self.context, failure))
            }
        }
    }
}

/// Fresh sample paired with the exact admitted recurrence.
#[must_use = "authorize the event fields or cancel the admitted recurrence"]
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) admitted: BluetoothLegacyConnectableAdvertisingPreSequence,
    pub(super) sample: BluetoothControllerTimeSample,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Authorize the sample and apply the accepted event window to CPU-owned SRAM.
    pub fn prepare_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringRetry<
                BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>,
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
                BluetoothLegacyConnectableAdvertisingRecurrencePrepared {
                    context,
                    task,
                    prepared,
                },
            ),
            Err(failure) => {
                let cause = event_retry_cause(failure.error());
                retry(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
                        cause,
                        BluetoothLegacyConnectableAdvertisingRecurrenceCandidate {
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrencePrepared<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) prepared: BluetoothLegacyConnectableAdvertisingEventPrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Join the sole event to the current exact empty scheduler list.
    pub fn merge_with<C, R>(
        self,
        continuation: C,
        ready: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, BluetoothLegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
    ) -> R {
        let Self {
            context,
            mut task,
            prepared,
        } = self;
        match task.merge_legacy_connectable_advertising_recurring_event(prepared) {
            Ok(merged) => ready(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurrenceMerged {
                    context,
                    task,
                    merged,
                },
            ),
            Err(failure) => retry(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
                    BluetoothLegacyConnectableAdvertisingRecurringRetryCause::EmptyList(
                        failure.error(),
                    ),
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) merged: BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Execute the existing atomic RX/HEAD/RUN suffix.
    pub fn start_with<C, R>(
        self,
        continuation: C,
        running: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingActiveSession<'runtime, S, CAPACITY>,
        ) -> R,
        retry: impl FnOnce(C, BluetoothLegacyConnectableAdvertisingRecurringRetry<Self, S::Error>) -> R,
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
            BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Running {
                controller,
                running: running_graph,
            } => running(
                continuation,
                BluetoothLegacyConnectableAdvertisingActiveSession::new(controller, running_graph),
            ),
            BluetoothLegacyConnectableAdvertisingSchedulerStartStep::Retryable { failure } => {
                let (task, merged, error) = failure.into_parts();
                let cause = match error {
                    BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Head(error) => {
                        BluetoothLegacyConnectableAdvertisingRecurringRetryCause::SchedulerHead(
                            error,
                        )
                    }
                    BluetoothLegacyConnectableAdvertisingSchedulerStartRetryError::Interrupts(
                        error,
                    ) => BluetoothLegacyConnectableAdvertisingRecurringRetryCause::SchedulerInterrupts(error),
                };
                retry(
                    continuation,
                    BluetoothLegacyConnectableAdvertisingRecurringRetry::new(
                        cause,
                        Self {
                            context,
                            task,
                            merged,
                        },
                    ),
                )
            }
            BluetoothLegacyConnectableAdvertisingSchedulerStartStep::FailStop(failure) => {
                let cause = match failure.cause() {
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::ReceivePublication(error) => {
                        BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::ReceivePublication(error)
                    }
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerHead(error) => {
                        BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::SchedulerHeadPublication(error)
                    }
                    BluetoothLegacyConnectableAdvertisingSchedulerFailStopCause::SchedulerRun(error) => {
                        BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::SchedulerRunPublication(error)
                    }
                };
                fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                    cause,
                    context,
                    _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::SchedulerPublication {
                        _failure: failure,
                    },
                })
            }
        }
    }
}
