//! Controller-time preparation for the first response-capable advertisement.
//!
//! This module owns only the pre-MMIO prefix. It checks out both static
//! runtimes, projects the complete radio window, obtains independent admission
//! and sequence samples, and joins the event to the proven empty scheduler
//! list. Any ordinary rejection is returned only after both runtimes and the
//! controller-time worker are idle again. Identity disagreements remain sealed
//! with the complete owner graph instead of being converted into an HCI error.

#![forbid(unsafe_code)]

use crate::{
    SchedulerInstant,
    le::{
        advertising::{
            LegacyAdvertisingTimingObservation,
            connectable::{
                LegacyConnectableAdvertisingCancellationInvariant,
                LegacyConnectableAdvertisingCancelled, LegacyConnectableAdvertisingEventCandidate,
                LegacyConnectableAdvertisingPrepared,
                LegacyConnectableAdvertisingRuntimeBeginFailure,
                LegacyConnectableAdvertisingSetPrepared,
            },
        },
        peripheral::PeripheralConnectionRuntimeBeginError,
    },
    scheduler::{
        ControllerTimeAcquisitionError,
        core::{
            LegacyConnectableAdvertisingAdmissionObservation,
            LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
            LegacyConnectableAdvertisingEventPreparationError,
            LegacyConnectableAdvertisingPreSequence,
            LegacyConnectableAdvertisingSequenceObservation,
        },
    },
};

use oer_esp32s31_bluetooth_memory::{
    LegacyConnectableAdvertisingMemoryGraphPrepareError, LegacyConnectableAdvertisingPduFitError,
};

use super::{
    ControllerPublishedTaskService, ControllerSchedulerEpochRetained, ControllerSchedulerNowReady,
    timed_preparation,
};

/// Finite ordinary reason why the first event returned to both idle runtimes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingControllerPreparationError {
    /// No unique Enable generation remains available.
    GenerationExhausted,
    /// One encoded PDU does not fit the reviewed controller-memory extent.
    PduFit(LegacyConnectableAdvertisingPduFitError),
    /// The sole connectable-advertising graph is already checked out.
    AdvertisingEventActive,
    /// The peripheral role cannot loan its receive allocation.
    PeripheralEventActive(PeripheralConnectionRuntimeBeginError),
    /// CPU-owned response-graph preparation rejected the input.
    MemoryPreparation(LegacyConnectableAdvertisingMemoryGraphPrepareError),
    /// The first-event timing geometry cannot be represented safely.
    TimingWindow,
    /// Timeline admission, sequencing, or event-field preparation failed.
    Event(LegacyConnectableAdvertisingEventPreparationError),
    /// The exclusive scheduler list was not empty at the merge edge.
    EmptyList(crate::scheduler::SchedulerEmptyListMergeError),
}

/// Class of invariant which prevented lossless pre-publication rollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingRollbackInvariantKind {
    /// The peripheral receive allocation did not rejoin its reserved identity.
    CancellationOwnership,
    /// The cancelled graph did not belong to one of the two originating runtimes.
    RuntimeRestore,
}

/// Why one preparation is permanently sealed from ordinary task reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LegacyConnectableAdvertisingControllerFailStopCause {
    /// Runtime checkout exposed an impossible affine ownership disagreement.
    RuntimeOwnership,
    /// A rollback could not restore every pre-publication owner.
    Rollback(LegacyConnectableAdvertisingRollbackInvariantKind),
    /// The controller-time worker is busy, faulted, or permanently exhausted.
    ControllerTime {
        /// Exact worker observation.
        error: ControllerTimeAcquisitionError,
        /// A simultaneous owner-restoration disagreement, if one occurred.
        rollback: Option<LegacyConnectableAdvertisingRollbackInvariantKind>,
    },
    /// Private phase storage and its controller-time request disagreed.
    PhaseOwnership,
}

pub(crate) enum LegacyConnectableAdvertisingRollbackFailure {
    Cancellation {
        _owner: LegacyConnectableAdvertisingCancellationInvariant,
    },
    RuntimeRestore {
        _owner: LegacyConnectableAdvertisingCancelled,
    },
}

impl LegacyConnectableAdvertisingRollbackFailure {
    const fn kind(&self) -> LegacyConnectableAdvertisingRollbackInvariantKind {
        match self {
            Self::Cancellation { .. } => {
                LegacyConnectableAdvertisingRollbackInvariantKind::CancellationOwnership
            }
            Self::RuntimeRestore { .. } => {
                LegacyConnectableAdvertisingRollbackInvariantKind::RuntimeRestore
            }
        }
    }
}

enum LegacyConnectableAdvertisingControllerFailStopState<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Initial {
        _current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        _failure: LegacyConnectableAdvertisingRuntimeBeginFailure,
    },
    Active {
        _controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _rollback: Option<LegacyConnectableAdvertisingRollbackFailure>,
    },
}

/// Complete owner graph sealed after a permanent controller-time or ownership fault.
///
/// Deliberately no method can recover the nested task service. A caller may
/// retain this value for diagnostics, but cannot relabel a non-idle worker or
/// fabricate either of the static runtime owners.
#[must_use = "a connectable-advertising fail-stop retains the complete controller owner"]
pub(crate) struct LegacyConnectableAdvertisingControllerPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    cause: LegacyConnectableAdvertisingControllerFailStopCause,
    _state: LegacyConnectableAdvertisingControllerFailStopState<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Exact permanent-fault classification without exposing the sealed owner.
    pub(crate) const fn cause(&self) -> LegacyConnectableAdvertisingControllerFailStopCause {
        self.cause
    }

    fn active(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: LegacyConnectableAdvertisingControllerFailStopCause,
        rollback: Option<LegacyConnectableAdvertisingRollbackFailure>,
    ) -> Self {
        Self {
            cause,
            _state: LegacyConnectableAdvertisingControllerFailStopState::Active {
                _controller: controller,
                _rollback: rollback,
            },
        }
    }

    fn from_timed(
        failure: timed_preparation::TimedPreparationFailStop<
            ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
            LegacyConnectableAdvertisingRollbackFailure,
        >,
    ) -> Self {
        let timed_cause = failure.cause();
        let (controller, rollback) = failure.into_parts();
        let rollback_kind = rollback
            .as_ref()
            .map(LegacyConnectableAdvertisingRollbackFailure::kind);
        let cause = match timed_cause {
            timed_preparation::TimedPreparationFailStopCause::ControllerTime(error) => {
                LegacyConnectableAdvertisingControllerFailStopCause::ControllerTime {
                    error,
                    rollback: rollback_kind,
                }
            }
            timed_preparation::TimedPreparationFailStopCause::Rollback => match rollback_kind {
                Some(kind) => LegacyConnectableAdvertisingControllerFailStopCause::Rollback(kind),
                None => LegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership,
            },
            timed_preparation::TimedPreparationFailStopCause::PhaseOwnership => {
                LegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership
            }
        };
        Self::active(controller, cause, rollback)
    }
}

enum LegacyConnectableAdvertisingControllerPreparationPhase {
    AlwaysAwakeTiming {
        prepared: LegacyConnectableAdvertisingPrepared,
        now: crate::controller::time::ControllerSchedulerNow,
    },
    Admission(LegacyConnectableAdvertisingEventCandidate),
    Sequence(LegacyConnectableAdvertisingPreSequence),
}

/// One exact timing, admission, or sequence sample request.
#[must_use = "recheck or explicitly cancel the connectable-advertising time request"]
pub(crate) struct LegacyConnectableAdvertisingControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    timed: timed_preparation::TimedPreparationPending<
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyConnectableAdvertisingControllerPreparationPhase,
        LegacyConnectableAdvertisingRollbackFailure,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn restore_legacy_connectable_advertising_cancelled_in_place(
        &mut self,
        cancelled: Result<
            LegacyConnectableAdvertisingCancelled,
            LegacyConnectableAdvertisingCancellationInvariant,
        >,
    ) -> timed_preparation::TimedPreparationRollbackOutcome<
        LegacyConnectableAdvertisingRollbackFailure,
    > {
        let cancelled = match cancelled {
            Ok(cancelled) => cancelled,
            Err(owner) => {
                return timed_preparation::TimedPreparationRollbackOutcome::FailStop(
                    LegacyConnectableAdvertisingRollbackFailure::Cancellation { _owner: owner },
                );
            }
        };
        match self
            .legacy_connectable_advertising_resources
            .restore_cancelled(cancelled, self.peripheral_connection_resources)
        {
            Ok(_definition) => timed_preparation::TimedPreparationRollbackOutcome::Restored,
            Err(owner) => timed_preparation::TimedPreparationRollbackOutcome::FailStop(
                LegacyConnectableAdvertisingRollbackFailure::RuntimeRestore { _owner: owner },
            ),
        }
    }

    pub(crate) fn restore_legacy_connectable_advertising_cancelled_with<R, Context>(
        mut self,
        cancelled: Result<
            LegacyConnectableAdvertisingCancelled,
            LegacyConnectableAdvertisingCancellationInvariant,
        >,
        context: Context,
        restored: impl FnOnce(Context, Self) -> R,
        fail_stop: impl FnOnce(Context, Self, LegacyConnectableAdvertisingRollbackFailure) -> R,
    ) -> R {
        match self.restore_legacy_connectable_advertising_cancelled_in_place(cancelled) {
            timed_preparation::TimedPreparationRollbackOutcome::Restored => restored(context, self),
            timed_preparation::TimedPreparationRollbackOutcome::FailStop(owner) => {
                fail_stop(context, self, owner)
            }
        }
    }

    fn begin_legacy_connectable_advertising_preparation_time_with<R, Context>(
        self,
        phase: LegacyConnectableAdvertisingControllerPreparationPhase,
        context: Context,
        pending: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        let timed =
            timed_preparation::TimedPreparationPending::begin(self, phase, |controller, phase| {
                let cancelled = match phase {
                    LegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                        prepared,
                        ..
                    } => prepared.cancel(),
                    LegacyConnectableAdvertisingControllerPreparationPhase::Admission(
                        candidate,
                    ) => candidate.cancel(),
                    LegacyConnectableAdvertisingControllerPreparationPhase::Sequence(admitted) => {
                        controller
                            .runtime
                            .cancel_legacy_connectable_advertising_pre_sequence(admitted)
                    }
                };
                controller.restore_legacy_connectable_advertising_cancelled_in_place(cancelled)
            });
        match timed {
            Ok(timed) => pending(
                context,
                LegacyConnectableAdvertisingControllerPreparationPending { timed },
            ),
            Err(failure) => fail_stop(
                context,
                LegacyConnectableAdvertisingControllerPreparationFailStop::from_timed(failure),
            ),
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyConnectableAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>
{
    fn rollback_after_idle_with<R, Context>(
        controller: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cancelled: Result<
            LegacyConnectableAdvertisingCancelled,
            LegacyConnectableAdvertisingCancellationInvariant,
        >,
        error: LegacyConnectableAdvertisingControllerPreparationError,
        context: Context,
        recovered: impl FnOnce(
            Context,
            ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            LegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        controller.restore_legacy_connectable_advertising_cancelled_with(
            cancelled,
            context,
            |context, controller| {
                recovered(
                    context,
                    ControllerSchedulerEpochRetained { controller },
                    error,
                )
            },
            |context, controller, rollback| {
                let cause =
                    LegacyConnectableAdvertisingControllerFailStopCause::Rollback(rollback.kind());
                fail_stop(
                    context,
                    LegacyConnectableAdvertisingControllerPreparationFailStop::active(
                        controller,
                        cause,
                        Some(rollback),
                    ),
                )
            },
        )
    }

    /// Perform exactly one observation of the active controller-time request.
    pub(crate) fn recheck_with<R, Context>(
        self,
        context: Context,
        pending: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        ready: impl FnOnce(
            Context,
            ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            LegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        ) -> R,
        recovered: impl FnOnce(
            Context,
            ControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            LegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        let (mut controller, phase, sample) = match self.timed.recheck() {
            timed_preparation::TimedPreparationStep::Waiting(timed) => {
                return pending(context, Self { timed });
            }
            timed_preparation::TimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => (controller, phase, sample),
            timed_preparation::TimedPreparationStep::FailStop(failure) => {
                return fail_stop(
                    context,
                    LegacyConnectableAdvertisingControllerPreparationFailStop::from_timed(failure),
                );
            }
        };
        match phase {
            LegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                prepared,
                now,
            } => {
                let epoch = now.epoch();
                let current = SchedulerInstant::from_image(now.micros());
                let radio_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample)
                    .into_scheduler_instant();
                let timing = LegacyAdvertisingTimingObservation {
                    current,
                    radio_ready,
                    epoch,
                };
                let candidate = match prepared
                    .form_first_event_candidate(timing, controller.runtime.scheduler_config())
                {
                    Ok(candidate) => candidate,
                    Err(failure) => {
                        return Self::rollback_after_idle_with(
                            controller,
                            failure.into_prepared().cancel(),
                            LegacyConnectableAdvertisingControllerPreparationError::TimingWindow,
                            context,
                            recovered,
                            fail_stop,
                        );
                    }
                };
                controller.begin_legacy_connectable_advertising_preparation_time_with(
                    LegacyConnectableAdvertisingControllerPreparationPhase::Admission(candidate),
                    context,
                    pending,
                    fail_stop,
                )
            }
            LegacyConnectableAdvertisingControllerPreparationPhase::Admission(candidate) => {
                let admitted = match controller
                    .runtime
                    .admit_legacy_connectable_advertising_first_event(
                        candidate,
                        LegacyConnectableAdvertisingAdmissionObservation { sample },
                    ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle_with(
                            controller,
                            failure.into_candidate().cancel(),
                            LegacyConnectableAdvertisingControllerPreparationError::Event(error),
                            context,
                            recovered,
                            fail_stop,
                        );
                    }
                };
                controller.begin_legacy_connectable_advertising_preparation_time_with(
                    LegacyConnectableAdvertisingControllerPreparationPhase::Sequence(admitted),
                    context,
                    pending,
                    fail_stop,
                )
            }
            LegacyConnectableAdvertisingControllerPreparationPhase::Sequence(admitted) => {
                let prepared = match controller
                    .runtime
                    .prepare_legacy_connectable_advertising_event(
                        admitted,
                        LegacyConnectableAdvertisingSequenceObservation { sample },
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle_with(
                            controller,
                            failure.into_candidate().cancel(),
                            LegacyConnectableAdvertisingControllerPreparationError::Event(error),
                            context,
                            recovered,
                            fail_stop,
                        );
                    }
                };
                match controller
                    .runtime
                    .prepare_legacy_connectable_advertising_empty_list_merge(prepared)
                {
                    Ok(merged) => ready(
                        context,
                        ControllerSchedulerEpochRetained { controller },
                        merged,
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let cancelled = controller
                            .runtime
                            .cancel_legacy_connectable_advertising_event(failure.into_prepared());
                        Self::rollback_after_idle_with(
                            controller,
                            cancelled,
                            LegacyConnectableAdvertisingControllerPreparationError::EmptyList(
                                error,
                            ),
                            context,
                            recovered,
                            fail_stop,
                        )
                    }
                }
            }
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Begin the first response-capable event from one cold or warm current.
    pub(crate) fn begin_legacy_connectable_advertising_first_event_with<R, Context>(
        self,
        definition: LegacyConnectableAdvertisingSetPrepared,
        context: Context,
        pending: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        recovered: impl FnOnce(
            Context,
            ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
            LegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            LegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        let current = self;
        let prepared = match current
            .controller
            .legacy_connectable_advertising_resources
            .begin_event(
                definition,
                current.controller.peripheral_connection_resources,
            ) {
            Ok(prepared) => prepared,
            Err(LegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted) => {
                return recovered(
                    context,
                    current,
                    LegacyConnectableAdvertisingControllerPreparationError::GenerationExhausted,
                );
            }
            Err(LegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    LegacyConnectableAdvertisingControllerPreparationError::PduFit(error),
                );
            }
            Err(LegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                definition: _,
            }) => {
                return recovered(
                    context,
                    current,
                    LegacyConnectableAdvertisingControllerPreparationError::AdvertisingEventActive,
                );
            }
            Err(LegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    LegacyConnectableAdvertisingControllerPreparationError::PeripheralEventActive(
                        error,
                    ),
                );
            }
            Err(LegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    LegacyConnectableAdvertisingControllerPreparationError::MemoryPreparation(
                        error,
                    ),
                );
            }
            Err(
                failure @ LegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant {
                    ..
                },
            ) => {
                return fail_stop(
                    context,
                    LegacyConnectableAdvertisingControllerPreparationFailStop {
                        cause:
                            LegacyConnectableAdvertisingControllerFailStopCause::RuntimeOwnership,
                        _state: LegacyConnectableAdvertisingControllerFailStopState::Initial {
                            _current: current,
                            _failure: failure,
                        },
                    },
                );
            }
        };
        let (controller, now) = current.into_parts();
        controller.begin_legacy_connectable_advertising_preparation_time_with(
            LegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                prepared,
                now,
            },
            context,
            pending,
            fail_stop,
        )
    }
}
