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
    LegacyConnectableAdvertisingRecurrenceCancellationPending,
    LegacyConnectableAdvertisingRecurrenceCancelled,
};

pub use phases::{
    LegacyConnectableAdvertisingRecurrenceCandidate,
    LegacyConnectableAdvertisingRecurrenceGraphPrepared,
    LegacyConnectableAdvertisingRecurrenceMerged, LegacyConnectableAdvertisingRecurrencePrepared,
    LegacyConnectableAdvertisingRecurrenceScheduled,
    LegacyConnectableAdvertisingRecurrenceSequencePending,
    LegacyConnectableAdvertisingRecurrenceSequenceReady,
};

use crate::{
    controller::{
        ControllerPublishedTaskService, SchedulerRunInterruptStorage,
        boot::{
            LegacyConnectableAdvertisingSchedulerFailStop,
            connectable_advertising::LegacyConnectableAdvertisingRollbackFailure,
            timed_preparation::{
                TimedPreparationController, TimedPreparationFailStop, TimedPreparationFailStopCause,
            },
        },
        time::{
            ControllerTimeEventError, ControllerTimePendingOrphanStep, ControllerTimePendingOwner,
            ControllerTimePendingOwnerStep, ControllerTimeRequest,
        },
    },
    le::{
        advertising::{
            LegacyAdvertisingEventPhase,
            connectable::{
                LegacyConnectableAdvertisingDisabledRestoreFailure,
                LegacyConnectableAdvertisingNextEventPortable,
                LegacyConnectableAdvertisingNextEventScheduled,
                LegacyConnectableAdvertisingRecurrenceStopped,
                LegacyConnectableAdvertisingRuntimeBeginFailure,
                LegacyConnectableAdvertisingSetPrepared,
                active::LegacyConnectableAdvertisingAwaitingRecurrence,
            },
        },
        peripheral::PeripheralConnectionRuntimeBeginError,
    },
    scheduler::{
        ControllerTimeAcquisitionError, SchedulerEmptyListMergeError,
        SchedulerHeadPublicationError, SchedulerReservationError,
        SchedulerSequenceAuthorizationError,
        core::{
            LegacyConnectableAdvertisingEmptySchedulerCancelFailure,
            LegacyConnectableAdvertisingEventPreparationError,
        },
    },
};

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_bluetooth_memory::{
    LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError,
    LegacyConnectableAdvertisingMemoryGraphPrepareError,
    LegacyConnectableAdvertisingMemoryGraphPublicationError,
    LegacyConnectableAdvertisingMemoryGraphSchedulerProofError,
    LegacyConnectableAdvertisingPduFitError,
    LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
};

type Task<'runtime, S, const CAPACITY: usize> =
    ControllerPublishedTaskService<'runtime, S, CAPACITY>;

pub(crate) struct LegacyConnectableAdvertisingRecurrenceTimedController<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) task: Task<'runtime, S, CAPACITY>,
    pub(crate) rollback: Option<LegacyConnectableAdvertisingRollbackFailure>,
}

impl<S, const CAPACITY: usize> ControllerTimePendingOwner
    for LegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn recheck_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<ControllerTimePendingOwnerStep, ControllerTimeEventError> {
        self.task.recheck_owned_controller_time(request)
    }

    fn cancel_owned_controller_time(
        &mut self,
        request: ControllerTimeRequest,
    ) -> Result<(), ControllerTimeEventError> {
        self.task.cancel_owned_controller_time(request)
    }

    fn drain_orphan_controller_time(
        &mut self,
    ) -> Result<ControllerTimePendingOrphanStep, ControllerTimeEventError> {
        self.task.drain_orphan_controller_time()
    }
}

impl<S, const CAPACITY: usize> TimedPreparationController
    for LegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn request_timed_preparation_sample(
        &mut self,
    ) -> Result<ControllerTimeRequest, ControllerTimeAcquisitionError> {
        self.task.request_timed_preparation_sample()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LegacyConnectableAdvertisingRecurrenceRollbackFailed;

#[derive(Clone, Copy)]
pub(crate) struct LegacyConnectableAdvertisingRecurrenceContext {
    definition: LegacyConnectableAdvertisingSetPrepared,
    identity: oer_bluetooth_ll::advertising_lifecycle::LegacyAdvertisingEventIdentity,
    start_offset_micros: u64,
    previous_phase: LegacyAdvertisingEventPhase,
    previous_scheduler_status: LegacyConnectableAdvertisingSchedulerItemCompletionStatus,
    rejected_packets: usize,
}

impl LegacyConnectableAdvertisingRecurrenceContext {
    fn from_completed(
        completed: &crate::le::advertising::connectable::LegacyConnectableAdvertisingNoConnectionRestored,
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
        scheduled: LegacyConnectableAdvertisingNextEventScheduled,
    ) -> (Self, LegacyConnectableAdvertisingNextEventPortable) {
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

    const fn stopped(self) -> LegacyConnectableAdvertisingRecurrenceStopped {
        LegacyConnectableAdvertisingRecurrenceStopped::from_restored_definition(
            self.definition,
            self.identity,
            self.previous_phase,
            self.previous_scheduler_status,
            self.rejected_packets,
        )
    }

    const fn stopped_with_portable_set(
        self,
        portable_set: oer_bluetooth_ll::connectable_advertising::LegacyConnectableAdvertisingSet<
            'static,
        >,
    ) -> LegacyConnectableAdvertisingRecurrenceStopped {
        LegacyConnectableAdvertisingRecurrenceStopped::from_portable_set(
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
pub enum LegacyConnectableAdvertisingRecurringRetryCause<E> {
    TimingWindow,
    Timeline(SchedulerReservationError),
    Sequence(SchedulerSequenceAuthorizationError),
    EventFields(LegacyConnectableAdvertisingMemoryGraphEventFieldsPrepareError),
    EmptyList(SchedulerEmptyListMergeError),
    SchedulerHead(SchedulerHeadPublicationError),
    SchedulerInterrupts(E),
}

/// Exact phase-specific retry owner.
#[must_use = "inspect the cause and retry or cancel the unchanged phase"]
pub struct LegacyConnectableAdvertisingRecurringRetry<P, E> {
    cause: LegacyConnectableAdvertisingRecurringRetryCause<E>,
    phase: P,
}

impl<P, E> LegacyConnectableAdvertisingRecurringRetry<P, E> {
    pub(crate) const fn new(
        cause: LegacyConnectableAdvertisingRecurringRetryCause<E>,
        phase: P,
    ) -> Self {
        Self { cause, phase }
    }

    pub const fn cause(&self) -> &LegacyConnectableAdvertisingRecurringRetryCause<E> {
        &self.cause
    }

    pub fn retry(self) -> P {
        self.phase
    }

    pub fn into_parts(self) -> (LegacyConnectableAdvertisingRecurringRetryCause<E>, P) {
        (self.cause, self.phase)
    }
}

/// Permanent ownership/publication class which forbids controller reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingRecurringFailStopCause {
    SchedulerEpochUnavailable,
    EventSequenceExhausted,
    RestoredDefinition(LegacyConnectableAdvertisingPduFitError),
    RestoredAdvertisingRuntimeBusy,
    RestoredPeripheralRuntime(PeripheralConnectionRuntimeBeginError),
    RestoredMemoryGraph(LegacyConnectableAdvertisingMemoryGraphPrepareError),
    RuntimeOwnership,
    ControllerTime(ControllerTimeAcquisitionError),
    Rollback,
    PhaseOwnership,
    EmptyListCancellation,
    ReceivePublication(LegacyConnectableAdvertisingMemoryGraphPublicationError),
    SchedulerHeadPublication(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
    SchedulerRunPublication(LegacyConnectableAdvertisingMemoryGraphSchedulerProofError),
}

pub(crate) enum LegacyConnectableAdvertisingRecurringFailStopOwner<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    Scheduled {
        _task: Task<'runtime, S, CAPACITY>,
        _portable: LegacyConnectableAdvertisingNextEventPortable,
    },
    Runtime {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: LegacyConnectableAdvertisingRuntimeBeginFailure,
    },
    DisabledRestore {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: LegacyConnectableAdvertisingDisabledRestoreFailure,
    },
    GraphPrepared {
        _task: Task<'runtime, S, CAPACITY>,
        _prepared: crate::le::advertising::connectable::LegacyConnectableAdvertisingPrepared,
    },
    Timed {
        _task: Task<'runtime, S, CAPACITY>,
        _rollback: Option<LegacyConnectableAdvertisingRollbackFailure>,
        _signal: Option<LegacyConnectableAdvertisingRecurrenceRollbackFailed>,
    },
    Rollback {
        _task: Task<'runtime, S, CAPACITY>,
        _rollback: LegacyConnectableAdvertisingRollbackFailure,
    },
    EmptyListCancellation {
        _task: Task<'runtime, S, CAPACITY>,
        _failure: LegacyConnectableAdvertisingEmptySchedulerCancelFailure,
    },
    SchedulerPublication {
        _failure: LegacyConnectableAdvertisingSchedulerFailStop<'runtime, S, CAPACITY>,
    },
}

/// Sealed exact owner after a non-recoverable recurring transition.
#[must_use = "retain the sealed controller and radio owner for shutdown diagnostics"]
pub struct BluetoothLegacyConnectableAdvertisingRecurringFailStop<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) cause: LegacyConnectableAdvertisingRecurringFailStopCause,
    pub(crate) context: LegacyConnectableAdvertisingRecurrenceContext,
    pub(crate) _owner: LegacyConnectableAdvertisingRecurringFailStopOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> LegacyConnectableAdvertisingRecurringFailStopCause {
        self.cause
    }

    pub const fn previous_phase(&self) -> LegacyAdvertisingEventPhase {
        self.context.previous_phase
    }

    pub const fn previous_scheduler_status(
        &self,
    ) -> LegacyConnectableAdvertisingSchedulerItemCompletionStatus {
        self.context.previous_scheduler_status
    }

    pub const fn rejected_packets(&self) -> usize {
        self.context.rejected_packets
    }
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingAwaitingRecurrence<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Select the next portable deadline without embedding an entropy policy.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> LegacyConnectableAdvertisingRecurrenceScheduled<'runtime, S, CAPACITY> {
        let (task, completed) = self.into_parts();
        let (context, event) = LegacyConnectableAdvertisingRecurrenceContext::from_scheduled(
            completed.schedule_next(delay),
        );
        LegacyConnectableAdvertisingRecurrenceScheduled::new(context, task, event)
    }
}

pub(crate) fn timed_fail_stop<S, const CAPACITY: usize>(
    context: LegacyConnectableAdvertisingRecurrenceContext,
    failure: TimedPreparationFailStop<
        LegacyConnectableAdvertisingRecurrenceTimedController<'_, S, CAPACITY>,
        LegacyConnectableAdvertisingRecurrenceRollbackFailed,
    >,
) -> BluetoothLegacyConnectableAdvertisingRecurringFailStop<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    let cause = match failure.cause() {
        TimedPreparationFailStopCause::ControllerTime(error) => {
            LegacyConnectableAdvertisingRecurringFailStopCause::ControllerTime(error)
        }
        TimedPreparationFailStopCause::Rollback => {
            LegacyConnectableAdvertisingRecurringFailStopCause::Rollback
        }
        TimedPreparationFailStopCause::PhaseOwnership => {
            LegacyConnectableAdvertisingRecurringFailStopCause::PhaseOwnership
        }
    };
    let (controller, signal) = failure.into_parts();
    BluetoothLegacyConnectableAdvertisingRecurringFailStop {
        cause,
        context,
        _owner: LegacyConnectableAdvertisingRecurringFailStopOwner::Timed {
            _task: controller.task,
            _rollback: controller.rollback,
            _signal: signal,
        },
    }
}

pub(crate) fn event_retry_cause<E>(
    error: LegacyConnectableAdvertisingEventPreparationError,
) -> LegacyConnectableAdvertisingRecurringRetryCause<E> {
    match error {
        LegacyConnectableAdvertisingEventPreparationError::Timeline(error) => {
            LegacyConnectableAdvertisingRecurringRetryCause::Timeline(error)
        }
        LegacyConnectableAdvertisingEventPreparationError::Sequence(error) => {
            LegacyConnectableAdvertisingRecurringRetryCause::Sequence(error)
        }
        LegacyConnectableAdvertisingEventPreparationError::EventFields(error) => {
            LegacyConnectableAdvertisingRecurringRetryCause::EventFields(error)
        }
    }
}

pub(crate) fn runtime_fail_stop_parts(
    failure: LegacyConnectableAdvertisingRuntimeBeginFailure,
) -> (
    LegacyConnectableAdvertisingRecurringFailStopCause,
    LegacyConnectableAdvertisingRuntimeBeginFailure,
) {
    match failure {
        failure @ LegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
            failure,
        ),
        LegacyConnectableAdvertisingRuntimeBeginFailure::PduFit { definition, error } => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RestoredDefinition(error),
            LegacyConnectableAdvertisingRuntimeBeginFailure::PduFit { definition, error },
        ),
        LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive { definition } => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RestoredAdvertisingRuntimeBusy,
            LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive { definition },
        ),
        LegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
            definition,
            error,
        } => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RestoredPeripheralRuntime(error),
            LegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                definition,
                error,
            },
        ),
        LegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
            definition,
            error,
        } => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RestoredMemoryGraph(error),
            LegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                definition,
                error,
            },
        ),
        failure @ LegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant { .. } => (
            LegacyConnectableAdvertisingRecurringFailStopCause::RuntimeOwnership,
            failure,
        ),
    }
}
