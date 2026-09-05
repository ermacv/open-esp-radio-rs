//! Lossless cancellation of each unpublished recurrence phase.

#![forbid(unsafe_code)]

use crate::{
    BluetoothSchedulerRunInterruptStorage,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingNextEventPortable,
        BluetoothLegacyConnectableAdvertisingRecurrenceStopped,
    },
    controller::boot::timed_preparation::{
        BluetoothTimedPreparationCancellationPending, BluetoothTimedPreparationCancellationStep,
    },
    legacy_connectable_advertising_active::BluetoothLegacyConnectableAdvertisingAwaitingRecurrence,
};
use open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertiserConfigured;

use super::{
    BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed,
    BluetoothLegacyConnectableAdvertisingRecurrenceTimedController,
    BluetoothLegacyConnectableAdvertisingRecurringFailStop,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner, Task, phases::*, timed_fail_stop,
};

type SequenceCancellationPending<'runtime, S, const CAPACITY: usize> =
    BluetoothTimedPreparationCancellationPending<
        BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<'runtime, S, CAPACITY>,
    >;

fn restore_disabled_with<'runtime, S, const CAPACITY: usize, C, R>(
    context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    mut task: Task<'runtime, S, CAPACITY>,
    configured: LegacyConnectableAdvertiserConfigured<'static>,
    stopped: BluetoothLegacyConnectableAdvertisingRecurrenceStopped,
    continuation: C,
    cancelled: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    ) -> R,
    fail_stop: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    ) -> R,
) -> R
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match task.restore_legacy_connectable_advertising_disabled(configured) {
        Ok(()) => cancelled(
            continuation,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled { task, stopped },
        ),
        Err(failure) => fail_stop(
            continuation,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause:
                    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
                context,
                _owner:
                    BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::DisabledRestore {
                        _task: task,
                        _failure: failure,
                    },
            },
        ),
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn stop_recurrence_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let (task, completed) = self.into_parts();
        let context =
            BluetoothLegacyConnectableAdvertisingRecurrenceContext::from_completed(&completed);
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task: Task<'runtime, S, CAPACITY>,
    stopped: BluetoothLegacyConnectableAdvertisingRecurrenceStopped,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn identity(
        &self,
    ) -> open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity {
        self.stopped.identity()
    }

    pub const fn previous_phase(&self) -> crate::BluetoothLegacyAdvertisingEventPhase {
        self.stopped.previous_phase()
    }

    pub const fn previous_scheduler_status(
        &self,
    ) -> open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus{
        self.stopped.previous_scheduler_status()
    }

    pub const fn rejected_packets(&self) -> usize {
        self.stopped.rejected_packets()
    }

    pub const fn portable_set(
        &self,
    ) -> open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<
        'static,
    > {
        self.stopped.portable_set()
    }

    pub fn into_parts(
        self,
    ) -> (
        Task<'runtime, S, CAPACITY>,
        open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<
            'static,
        >,
        crate::BluetoothLegacyAdvertisingEventPhase,
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
        usize,
    ){
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
pub struct BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pending: SequenceCancellationPending<'runtime, S, CAPACITY>,
}

fn restore_with<'runtime, S, const CAPACITY: usize, C, R>(
    context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    task: Task<'runtime, S, CAPACITY>,
    restored: Result<
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    >,
    continuation: C,
    cancelled: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
    ) -> R,
    fail_stop: impl FnOnce(
        C,
        BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
    ) -> R,
) -> R
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    task.restore_legacy_connectable_advertising_cancelled_with(
        restored,
        (context, continuation),
        |(context, continuation), task| {
            cancelled(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurrenceCancelled {
                    task,
                    stopped: context.stopped(),
                },
            )
        },
        |(context, continuation), task, rollback| {
            fail_stop(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                    cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::Rollback,
                    context,
                    _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Rollback {
                        _task: task,
                        _rollback: rollback,
                    },
                },
            )
        },
    )
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Cancel the untouched portable successor without a fallible ownership join.
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        let configured = match self.portable {
            BluetoothLegacyConnectableAdvertisingNextEventPortable::Event(event) => event.disable(),
            BluetoothLegacyConnectableAdvertisingNextEventPortable::SequenceExhausted(complete) => {
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
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
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
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
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
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Cancel and roll back now, but do not expose the task before orphan drain.
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        draining: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending<
                'runtime,
                S,
                CAPACITY,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self.pending.cancel() {
            Ok(pending) => draining(
                continuation,
                BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending {
                    context: self.context,
                    pending,
                },
            ),
            Err(failure) => fail_stop(continuation, timed_fail_stop(self.context, failure)),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
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
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
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
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn cancel_with<C, R>(
        self,
        continuation: C,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
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
            Err(failure) => fail_stop(continuation, BluetoothLegacyConnectableAdvertisingRecurringFailStop {
                cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::EmptyListCancellation,
                context: self.context,
                _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::EmptyListCancellation {
                    _task: task,
                    _failure: failure,
                },
            }),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Observe one bounded abandoned-request drain edge.
    pub fn recheck_with<C, R>(
        self,
        continuation: C,
        waiting: impl FnOnce(C, Self) -> R,
        cancelled: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurrenceCancelled<'runtime, S, CAPACITY>,
        ) -> R,
        fail_stop: impl FnOnce(
            C,
            BluetoothLegacyConnectableAdvertisingRecurringFailStop<'runtime, S, CAPACITY>,
        ) -> R,
    ) -> R {
        match self
            .pending
            .recheck::<BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed>()
        {
            BluetoothTimedPreparationCancellationStep::Waiting(pending) => waiting(
                continuation,
                Self {
                    context: self.context,
                    pending,
                },
            ),
            BluetoothTimedPreparationCancellationStep::Recovered(controller) => {
                match controller.rollback {
                    None => cancelled(continuation, BluetoothLegacyConnectableAdvertisingRecurrenceCancelled {
                        task: controller.task,
                        stopped: self.context.stopped(),
                    }),
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
                }
            }
            BluetoothTimedPreparationCancellationStep::FailStop(failure) => {
                fail_stop(continuation, timed_fail_stop(self.context, failure))
            }
        }
    }
}
