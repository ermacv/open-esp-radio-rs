//! Executor-neutral recurrence of a completed connectable advertising event.
//!
//! Every pre-publication phase is a distinct affine type. This keeps the chip
//! driver free of one maximum-sized type-erased state enum while preserving
//! exact ownership through the portable deadline, Controller-time request,
//! common timeline, exclusive list, and atomic RX/HEAD/RUN suffix.

#![forbid(unsafe_code)]

mod cancellation;
mod phases;

pub use cancellation::{
    BluetoothLegacyConnectableAdvertisingRecurrenceCancellationPending,
    BluetoothLegacyConnectableAdvertisingRecurrenceCancelled,
};
pub use phases::{
    BluetoothLegacyConnectableAdvertisingRecurrenceCandidate,
    BluetoothLegacyConnectableAdvertisingRecurrenceGraphPrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceMerged,
    BluetoothLegacyConnectableAdvertisingRecurrencePrepared,
    BluetoothLegacyConnectableAdvertisingRecurrenceScheduled,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequencePending,
    BluetoothLegacyConnectableAdvertisingRecurrenceSequenceReady,
};

use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError,
    BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    BluetoothLegacyConnectableAdvertisingPduFitError,
    BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
};

use crate::{
    BluetoothControllerPublishedTaskService, BluetoothControllerTimeAcquisitionError,
    BluetoothLegacyAdvertisingEventPhase, BluetoothPeripheralConnectionRuntimeBeginError,
    BluetoothSchedulerEmptyListMergeError, BluetoothSchedulerHeadPublicationError,
    BluetoothSchedulerReservationError, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerSequenceAuthorizationError,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure,
        BluetoothLegacyConnectableAdvertisingNextEventPortable,
        BluetoothLegacyConnectableAdvertisingNextEventScheduled,
        BluetoothLegacyConnectableAdvertisingRecurrenceStopped,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
        BluetoothLegacyConnectableAdvertisingSetPrepared,
    },
    controller::boot::{
        BluetoothLegacyConnectableAdvertisingSchedulerFailStop,
        connectable_advertising::BluetoothLegacyConnectableAdvertisingRollbackFailure,
        timed_preparation::{
            BluetoothTimedPreparationController, BluetoothTimedPreparationFailStop,
            BluetoothTimedPreparationFailStopCause,
        },
    },
    controller::time::{
        BluetoothControllerTimeEventError, BluetoothControllerTimePendingOrphanStep,
        BluetoothControllerTimePendingOwner, BluetoothControllerTimePendingOwnerStep,
        BluetoothControllerTimeRequest,
    },
    legacy_connectable_advertising_active::BluetoothLegacyConnectableAdvertisingAwaitingRecurrence,
    scheduler::core::{
        BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure,
        BluetoothLegacyConnectableAdvertisingEventPreparationError,
    },
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;

pub(super) struct BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) task: Task<'runtime, S, CAPACITY>,
    pub(super) rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackFailure>,
}

impl<S, const CAPACITY: usize> BluetoothControllerTimePendingOwner
    for BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<BluetoothControllerTimePendingOwnerStep, BluetoothControllerTimeEventError> {
        self.task.recheck_owned_controller_time(request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: BluetoothControllerTimeRequest,
    ) -> Result<(), BluetoothControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<BluetoothControllerTimePendingOrphanStep, BluetoothControllerTimeEventError> {
        self.task.drain_orphan_controller_time()
    }
}

impl<S, const CAPACITY: usize> BluetoothTimedPreparationController
    for BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<BluetoothControllerTimeRequest, BluetoothControllerTimeAcquisitionError> {
        self.task.request_timed_preparation_sample()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed;

#[derive(Clone, Copy)]
pub(super) struct BluetoothLegacyConnectableAdvertisingRecurrenceContext {
    definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
    identity: open_esp_radio_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity,
    start_offset_micros: u64,
    previous_phase: BluetoothLegacyAdvertisingEventPhase,
    previous_scheduler_status: BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

impl BluetoothLegacyConnectableAdvertisingRecurrenceContext {
    fn from_completed(
        completed: &crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingNoConnectionRestored,
    ) -> Self {
        Self {
            definition: completed.definition(),
            identity: completed.identity(),
            start_offset_micros: 0,
            previous_phase: completed.phase(),
            previous_scheduler_status: completed.scheduler_status(),
            rejected_packets: completed.rejected_packets(),
        }
    }

    fn from_scheduled(
        scheduled: BluetoothLegacyConnectableAdvertisingNextEventScheduled,
    ) -> (Self, BluetoothLegacyConnectableAdvertisingNextEventPortable) {
        let (
            definition,
            portable,
            start_offset_micros,
            previous_phase,
            previous_scheduler_status,
            rejected_packets,
        ) = scheduled.into_parts();
        let identity = portable.identity();
        (
            Self {
                definition,
                identity,
                start_offset_micros,
                previous_phase,
                previous_scheduler_status,
                rejected_packets,
            },
            portable,
        )
    }

    const fn stopped(self) -> BluetoothLegacyConnectableAdvertisingRecurrenceStopped {
        BluetoothLegacyConnectableAdvertisingRecurrenceStopped::from_restored_definition(
            self.definition,
            self.identity,
            self.previous_phase,
            self.previous_scheduler_status,
            self.rejected_packets,
        )
    }

    const fn stopped_with_portable_set(
        self,
        portable_set: open_esp_radio_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<
            'static,
        >,
    ) -> BluetoothLegacyConnectableAdvertisingRecurrenceStopped {
        BluetoothLegacyConnectableAdvertisingRecurrenceStopped::from_portable_set(
            portable_set,
            self.identity,
            self.previous_phase,
            self.previous_scheduler_status,
            self.rejected_packets,
        )
    }
}

/// Finite ordinary reason one unchanged phase should be retried.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E> {
    TimingWindow,
    Timeline(BluetoothSchedulerReservationError),
    Sequence(BluetoothSchedulerSequenceAuthorizationError),
    EventFields(BluetoothLegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
    EmptyList(BluetoothSchedulerEmptyListMergeError),
    SchedulerHead(BluetoothSchedulerHeadPublicationError),
    SchedulerInterrupts(E),
}

/// Exact phase-specific retry owner.
#[must_use = "inspect the cause and retry or cancel the unchanged phase"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringRetry<P, E> {
    cause: BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E>,
    phase: P,
}

impl<P, E> BluetoothLegacyConnectableAdvertisingRecurringRetry<P, E> {
    pub(super) const fn new(
        cause: BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E>,
        phase: P,
    ) -> Self {
        Self { cause, phase }
    }

    pub const fn cause(&self) -> &BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E> {
        &self.cause
    }

    pub fn retry(self) -> P {
        self.phase
    }

    pub fn into_parts(
        self,
    ) -> (
        BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E>,
        P,
    ) {
        (self.cause, self.phase)
    }
}

/// Permanent ownership/publication class which forbids controller reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyConnectableAdvertisingRecurringFailStopCause {
    SchedulerEpochUnavailable,
    EventSequenceExhausted,
    RestoredDefinition(BluetoothLegacyConnectableAdvertisingPduFitError),
    RestoredAdvertisingRuntimeBusy,
    RestoredPeripheralRuntime(BluetoothPeripheralConnectionRuntimeBeginError),
    RestoredMemoryGraph(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError),
    RuntimeOwnership,
    ControllerTime(BluetoothControllerTimeAcquisitionError),
    Rollback,
    PhaseOwnership,
    EmptyListCancellation,
    ReceivePublication(BluetoothLegacyConnectableAdvertisingMemoryGraphPublicationError),
    SchedulerHeadPublication(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
    SchedulerRunPublication(BluetoothLegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
}

pub(super) enum BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Scheduled {
        _task: Task<'runtime, S, CAPACITY>,
        _portable: BluetoothLegacyConnectableAdvertisingNextEventPortable,
    },
    Runtime {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    },
    DisabledRestore {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: BluetoothLegacyConnectableAdvertisingDisabledRestoreFailure,
    },
    GraphPrepared {
        _task: Task<'runtime, S, CAPACITY>,
        _prepared: crate::connectable_advertising::BluetoothLegacyConnectableAdvertisingPrepared,
    },
    Timed {
        _task: Task<'runtime, S, CAPACITY>,
        _rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackFailure>,
        _signal: Option<BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed>,
    },
    Rollback {
        _task: Task<'runtime, S, CAPACITY>,
        _rollback: BluetoothLegacyConnectableAdvertisingRollbackFailure,
    },
    EmptyListCancellation {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: BluetoothLegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    },
    SchedulerPublication {
        _failure: BluetoothLegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, CAPACITY>,
    },
}

/// Sealed exact owner after a non-recoverable recurring transition.
#[must_use = "retain the sealed controller and radio owner for shutdown diagnostics"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(super) cause: BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    pub(super) context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    pub(super) _owner:
        BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyConnectableAdvertisingRecurringFailStopCause {
        self.cause
    }

    pub const fn previous_phase(&self) -> BluetoothLegacyAdvertisingEventPhase {
        self.context.previous_phase
    }

    pub const fn previous_scheduler_status(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.context.previous_scheduler_status
    }

    pub const fn rejected_packets(&self) -> usize {
        self.context.rejected_packets
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Select the next portable deadline without embedding an entropy policy.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY> {
        let (task, completed) = self.into_parts();
        let (context, event) =
            BluetoothLegacyConnectableAdvertisingRecurrenceContext::from_scheduled(
                completed.schedule_next(delay),
            );
        BluetoothLegacyConnectableAdvertisingRecurrenceScheduled::new(context, task, event)
    }
}

pub(super) fn timed_fail_stop<S, const CAPACITY: usize>(
    context: BluetoothLegacyConnectableAdvertisingRecurrenceContext,
    failure: BluetoothTimedPreparationFailStop<
        BluetoothLegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>,
        BluetoothLegacyConnectableAdvertisingRecurrenceRollbackFailed,
    >,
) -> BluetoothLegacyConnectableAdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    let cause = match failure.cause() {
        BluetoothTimedPreparationFailStopCause::ControllerTime(error) => {
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::ControllerTime(error)
        }
        BluetoothTimedPreparationFailStopCause::Rollback => {
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::Rollback
        }
        BluetoothTimedPreparationFailStopCause::PhaseOwnership => {
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::PhaseOwnership
        }
    };
    let (controller, signal) = failure.into_parts();
    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
        cause,
        context,
        _owner: BluetoothLegacyConnectableAdvertisingRecurringFailStopOwner::Timed {
            _task: controller.task,
            _rollback: controller.rollback,
            _signal: signal,
        },
    }
}

pub(super) fn event_retry_cause<E>(
    error: BluetoothLegacyConnectableAdvertisingEventPreparationError,
) -> BluetoothLegacyConnectableAdvertisingRecurringRetryCause<E> {
    match error {
        BluetoothLegacyConnectableAdvertisingEventPreparationError::Timeline(error) => {
            BluetoothLegacyConnectableAdvertisingRecurringRetryCause::Timeline(error)
        }
        BluetoothLegacyConnectableAdvertisingEventPreparationError::Sequence(error) => {
            BluetoothLegacyConnectableAdvertisingRecurringRetryCause::Sequence(error)
        }
        BluetoothLegacyConnectableAdvertisingEventPreparationError::EventFields(error) => {
            BluetoothLegacyConnectableAdvertisingRecurringRetryCause::EventFields(error)
        }
    }
}

pub(super) fn runtime_fail_stop_parts(
    failure: BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
) -> (
    BluetoothLegacyConnectableAdvertisingRecurringFailStopCause,
    BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
) {
    match failure {
        failure @ BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
            failure,
        ),
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
            definition,
            error,
        } => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RestoredDefinition(error),
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                definition,
                error,
            },
        ),
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
            definition,
        } => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RestoredAdvertisingRuntimeBusy,
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                definition,
            },
        ),
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
            definition,
            error,
        } => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RestoredPeripheralRuntime(
                error,
            ),
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                definition,
                error,
            },
        ),
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
            definition,
            error,
        } => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RestoredMemoryGraph(error),
            BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                definition,
                error,
            },
        ),
        failure @ BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
            ..
        } => (
            BluetoothLegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
            failure,
        ),
    }
}
