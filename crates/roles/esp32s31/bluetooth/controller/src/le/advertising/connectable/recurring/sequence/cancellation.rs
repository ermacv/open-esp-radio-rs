//! Lossless cancellation of each unpublished recurrence phase.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        SchedulerRunInterruptStorage,
        boot::timed_preparation::{
            TimedPreparationCancellationPending, TimedPreparationCancellationStep,
        },
    },
    le::advertising::connectable::{
        LegacyConnectableAdvertisingCancellationInvariant, LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingNextEventPortable,
        LegacyConnectableAdvertisingRecurrenceStopped,
        active::LegacyConnectableAdvertisingAwaitingRecurrence,
    },
};

use oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertiserConfigured;

use super::{
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    LegacyConnectableAdvertisingRecurrenceContext,
    LegacyConnectableAdvertisingRecurrenceRollbackFailed,
    LegacyConnectableAdvertisingRecurrenceTimedController,
    LegacyConnectableAdvertisingRecurringFailStopCause,
    LegacyConnectableAdvertisingRecurringFailStopOwner, Task, phases::*, timed_fail_stop,
};

type SequenceCancellationPending<'runtime, S, const CAPACITY: usize> =
    TimedPreparationCancellationPending<
        LegacyConnectableAdvertisingRecurrenceTimedController<'runtime, S, CAPACITY>,
    >;

fn restore_disabled_with<'runtime, S, const CAPACITY: usize, C, R>(
    context: LegacyConnectableAdvertisingRecurrenceContext,
    mut task: Task<'runtime, S, CAPACITY>,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    stopped: LegacyConnectableAdvertisingRecurrenceStopped,
    continuation: C,
    cancelled: impl FnOnce(
        C,
        LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    ) -> R,
    fail_stop: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    ) -> R,
) -> R
where
    S: SchedulerRunInterruptStorage,
{
    match task.restore_legacy_connectable_advertising_disabled(configured) {
        Ok(()) => cancelled(
            continuation,
            LegacyConnectableAdvertisingRecurrenceCancelled { task, stopped },
        ),
        Err(failure) => fail_stop(
            continuation,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause: LegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
                context,
                _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::DisabledRestore {
                    _task: task,
                    _failure: failure,
                },
            },
        ),
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn stop_recurrence_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let (task, completed) = self.into_parts();
        let context = LegacyConnectableAdvertisingRecurrenceContext::from_completed(&completed);
        let (configured, stopped) = completed.prepare_recurrence_stop();
        restore_disabled_with(
            context,
            task,
            configured,
            stopped,
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

/// Clean CPU-only recurrence cancellation with every reusable value retained.
#[must_use = "recover the controller task, portable set, and diagnostics together"]
pub struct LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    stopped: LegacyConnectableAdvertisingRecurrenceStopped,
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn identity(
        &self,
    ) -> oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.stopped.identity()
    }

    pub const fn previous_phase(&self) -> crate::le::advertising::LegacyAdvertisingEventPhase {
        self.stopped.previous_phase()
    }

    pub const fn previous_scheduler_status(
        &self,
    ) -> oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus
    {
        self.stopped.previous_scheduler_status()
    }

    pub const fn rejected_packets(&self) -> usize {
        self.stopped.rejected_packets()
    }

    pub const fn portable_set(
        &self,
    ) -> oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<'static> {
        self.stopped.portable_set()
    }

    pub fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<'static>,
        crate::le::advertising::LegacyAdvertisingEventPhase,
        oer_esp32s31_bluetooth_memory::LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        usize,
    ) {
        (
            self.task,
            self.stopped.portable_set(),
            self.stopped.previous_phase(),
            self.stopped.previous_scheduler_status(),
            self.stopped.rejected_packets(),
        )
    }
}

/// Restored role graph whose abandoned Controller-time request is still draining.
#[must_use = "recheck until the exact abandoned request is drained"]
pub struct LegacyConnectableAdvertisingRecurrenceCancellationPending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    context: LegacyConnectableAdvertisingRecurrenceContext,
    pending: SequenceCancellationPending<'runtime, S, CAPACITY>,
}

fn restore_with<'runtime, S, const CAPACITY: usize, C, R>(
    context: LegacyConnectableAdvertisingRecurrenceContext,
    task: Task<'runtime, S, CAPACITY>,
    restored: Result<
        LegacyConnectableAdvertisingCancelled,
        LegacyConnectableAdvertisingCancellationInvariant,
    >,
    continuation: C,
    cancelled: impl FnOnce(
        C,
        LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    ) -> R,
    fail_stop: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    ) -> R,
) -> R
where
    S: SchedulerRunInterruptStorage,
{
    task.restore_legacy_connectable_advertising_cancelled_with(
        restored,
        (context, continuation),
        |(context, continuation), task| {
            cancelled(
                continuation,
                LegacyConnectableAdvertisingRecurrenceCancelled {
                    task,
                    stopped: context.stopped(),
                },
            )
        },
        |(context, continuation), task, rollback| {
            fail_stop(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                    cause: LegacyConnectableAdvertisingRecurringFailStopCause::Rollback,
                    context,
                    _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Rollback {
                        _task: task,
                        _rollback: rollback,
                    },
                },
            )
        },
    )
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Cancel the untouched portable successor without a fallible ownership join.
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let configured = match self.portable {
            LegacyConnectableAdvertisingNextEventPortable::Event(event) => event.disable(),
            LegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(complete) => {
                complete.disable()
            }
        };
        let stopped = self.context.stopped_with_portable_set(configured.set());
        restore_disabled_with(
            self.context,
            self.task,
            configured,
            stopped,
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        restore_with(
            self.context,
            self.task,
            self.prepared.cancel(),
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        restore_with(
            self.context,
            self.task,
            self.candidate.cancel(),
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Cancel and roll back now, but do not expose the task before orphan drain.
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        draining: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancellationPending<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self.pending.cancel() {
            Ok(pending) => draining(
                continuation,
                LegacyConnectableAdvertisingRecurrenceCancellationPending {
                    context: self.context,
                    pending,
                },
            ),
            Err(failure) => fail_stop(continuation, timed_fail_stop(self.context, failure)),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let mut task = self.task;
        let restored =
            task.cancel_legacy_connectable_advertising_recurring_pre_sequence(self.admitted);
        restore_with(
            self.context,
            task,
            restored,
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let mut task = self.task;
        let restored = task.cancel_legacy_connectable_advertising_recurring_event(self.prepared);
        restore_with(
            self.context,
            task,
            restored,
            continuation,
            cancelled,
            fail_stop,
        )
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let mut task = self.task;
        match task.cancel_legacy_connectable_advertising_recurring_merge(self.merged) {
            Ok(restored) => restore_with(
                self.context,
                task,
                Ok(restored),
                continuation,
                cancelled,
                fail_stop,
            ),
            Err(failure) => fail_stop(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                    cause:
                        LegacyConnectableAdvertisingRecurringFailStopCause::EmptyListCancellation,
                    context: self.context,
                    _owner:
                        LegacyConnectableAdvertisingRecurringFailStopOwner::EmptyListCancellation {
                            _task: task,
                            _failure: failure,
                        },
                },
            ),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingRecurrenceCancellationPending<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Observe one bounded abandoned-request drain edge.
    pub fn recheck_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(C, Self) -> R,
        cancelled: impl FnOnce(
            C,
            LegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self
            .pending
            .recheck::<LegacyConnectableAdvertisingRecurrenceRollbackFailed>()
        {
            TimedPreparationCancellationStep::Waiting(pending) => waiting(
                continuation,
                Self {
                    context: self.context,
                    pending,
                },
            ),
            TimedPreparationCancellationStep::Recovered(controller) => match controller.rollback {
                None => cancelled(
                    continuation,
                    LegacyConnectableAdvertisingRecurrenceCancelled {
                        task: controller.task,
                        stopped: self.context.stopped(),
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
            TimedPreparationCancellationStep::FailStop(failure) => {
                fail_stop(continuation, timed_fail_stop(self.context, failure))
            }
        }
    }
}
