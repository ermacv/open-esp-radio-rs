//! Controller-time preparation for the first response-capable advertisement.
//!
//! This module owns only the pre-MMIO prefix. It checks out both static
//! runtimes, projects the complete radio window, obtains independent admission
//! and sequence samples, and joins the event to the proven empty scheduler
//! list. Any ordinary rejection is returned only after both runtimes and the
//! controller-time worker are idle again. Identity disagreements remain sealed
//! with the complete owner graph instead of being converted into an HCI error.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth_memory::{
    BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError,
    BluetoothLegacyConnectableAdvertisingPduFitError,
};

use super::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerEpochRetained,
    BluetoothControllerSchedulerNowReady, timed_preparation,
};
use crate::{
    BluetoothControllerTimeAcquisitionError, BluetoothLegacyAdvertisingTimingObservation,
    BluetoothPeripheralConnectionRuntimeBeginError, BluetoothSchedulerInstant,
    connectable_advertising::{
        BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        BluetoothLegacyConnectableAdvertisingCancelled,
        BluetoothLegacyConnectableAdvertisingEventCandidate,
        BluetoothLegacyConnectableAdvertisingPrepared,
        BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
        BluetoothLegacyConnectableAdvertisingSetPrepared,
    },
    scheduler::{
        BluetoothLegacyConnectableAdvertisingAdmissionObservation,
        BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        BluetoothLegacyConnectableAdvertisingEventPreparationError,
        BluetoothLegacyConnectableAdvertisingPreSequence,
        BluetoothLegacyConnectableAdvertisingSequenceObservation,
    },
};

/// Finite ordinary reason why the first event returned to both idle runtimes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingControllerPreparationError {
    /// No unique Enable generation remains available.
    GenerationExhausted,
    /// One encoded PDU does not fit the reviewed controller-memory extent.
    PduFit(BluetoothLegacyConnectableAdvertisingPduFitError),
    /// The sole connectable-advertising graph is already checked out.
    AdvertisingEventActive,
    /// The peripheral role cannot loan its receive allocation.
    PeripheralEventActive(BluetoothPeripheralConnectionRuntimeBeginError),
    /// CPU-owned response-graph preparation rejected the input.
    MemoryPreparation(BluetoothLegacyConnectableAdvertisingMemoryGraphPrepareError),
    /// The first-event timing geometry cannot be represented safely.
    TimingWindow,
    /// Timeline admission, sequencing, or event-field preparation failed.
    Event(BluetoothLegacyConnectableAdvertisingEventPreparationError),
    /// The exclusive scheduler list was not empty at the merge edge.
    EmptyList(crate::BluetoothSchedulerEmptyListMergeError),
}

/// Class of invariant which prevented lossless pre-publication rollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingRollbackInvariantKind {
    /// The peripheral receive allocation did not rejoin its reserved identity.
    CancellationOwnership,
    /// The cancelled graph did not belong to one of the two originating runtimes.
    RuntimeRestore,
}

/// Why one preparation is permanently sealed from ordinary task reuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothLegacyConnectableAdvertisingControllerFailStopCause {
    /// Runtime checkout exposed an impossible affine ownership disagreement.
    RuntimeOwnership,
    /// A rollback could not restore every pre-publication owner.
    Rollback(BluetoothLegacyConnectableAdvertisingRollbackInvariantKind),
    /// The controller-time worker is busy, faulted, or permanently exhausted.
    ControllerTime {
        /// Exact worker observation.
        error: BluetoothControllerTimeAcquisitionError,
        /// A simultaneous owner-restoration disagreement, if one occurred.
        rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackInvariantKind>,
    },
    /// Private phase storage and its controller-time request disagreed.
    PhaseOwnership,
}

pub(crate) enum BluetoothLegacyConnectableAdvertisingRollbackFailure {
    Cancellation {
        _owner: BluetoothLegacyConnectableAdvertisingCancellationInvariant,
    },
    RuntimeRestore {
        _owner: BluetoothLegacyConnectableAdvertisingCancelled,
    },
}

impl BluetoothLegacyConnectableAdvertisingRollbackFailure {
    const fn kind(&self) -> BluetoothLegacyConnectableAdvertisingRollbackInvariantKind {
        match self {
            Self::Cancellation { .. } => {
                BluetoothLegacyConnectableAdvertisingRollbackInvariantKind::CancellationOwnership
            }
            Self::RuntimeRestore { .. } => {
                BluetoothLegacyConnectableAdvertisingRollbackInvariantKind::RuntimeRestore
            }
        }
    }
}

enum BluetoothLegacyConnectableAdvertisingControllerFailStopState<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    Initial {
        _current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
        _failure: BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure,
    },
    Active {
        _controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        _rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackFailure>,
    },
}

/// Complete owner graph sealed after a permanent controller-time or ownership fault.
///
/// Deliberately no method can recover the nested task service. A caller may
/// retain this value for diagnostics, but cannot relabel a non-idle worker or
/// fabricate either of the static runtime owners.
#[must_use = "a connectable-advertising fail-stop retains the complete controller owner"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    cause: BluetoothLegacyConnectableAdvertisingControllerFailStopCause,
    _state: BluetoothLegacyConnectableAdvertisingControllerFailStopState<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    >
{
    /// Exact permanent-fault classification without exposing the sealed owner.
    pub(crate) const fn cause(
        &self,
    ) -> BluetoothLegacyConnectableAdvertisingControllerFailStopCause {
        self.cause
    }

    fn active(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cause: BluetoothLegacyConnectableAdvertisingControllerFailStopCause,
        rollback: Option<BluetoothLegacyConnectableAdvertisingRollbackFailure>,
    ) -> Self {
        Self {
            cause,
            _state: BluetoothLegacyConnectableAdvertisingControllerFailStopState::Active {
                _controller: controller,
                _rollback: rollback,
            },
        }
    }

    fn from_timed(
        failure: timed_preparation::BluetoothTimedPreparationFailStop<
            BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
            BluetoothLegacyConnectableAdvertisingRollbackFailure,
        >,
    ) -> Self {
        let timed_cause = failure.cause();
        let (controller, rollback) = failure.into_parts();
        let rollback_kind = rollback
            .as_ref()
            .map(BluetoothLegacyConnectableAdvertisingRollbackFailure::kind);
        let cause = match timed_cause {
            timed_preparation::BluetoothTimedPreparationFailStopCause::ControllerTime(error) => {
                BluetoothLegacyConnectableAdvertisingControllerFailStopCause::ControllerTime {
                    error,
                    rollback: rollback_kind,
                }
            }
            timed_preparation::BluetoothTimedPreparationFailStopCause::Rollback => {
                match rollback_kind {
                    Some(kind) => {
                        BluetoothLegacyConnectableAdvertisingControllerFailStopCause::Rollback(kind)
                    }
                    None => {
                        BluetoothLegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership
                    }
                }
            }
            timed_preparation::BluetoothTimedPreparationFailStopCause::PhaseOwnership => {
                BluetoothLegacyConnectableAdvertisingControllerFailStopCause::PhaseOwnership
            }
        };
        Self::active(controller, cause, rollback)
    }
}

enum BluetoothLegacyConnectableAdvertisingControllerPreparationPhase {
    AlwaysAwakeTiming {
        prepared: BluetoothLegacyConnectableAdvertisingPrepared,
        now: crate::controller_time::BluetoothControllerSchedulerNow,
    },
    Admission(BluetoothLegacyConnectableAdvertisingEventCandidate),
    Sequence(BluetoothLegacyConnectableAdvertisingPreSequence),
}

/// One exact timing, admission, or sequence sample request.
#[must_use = "recheck or explicitly cancel the connectable-advertising time request"]
pub(crate) struct BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> {
    timed: timed_preparation::BluetoothTimedPreparationPending<
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        BluetoothLegacyConnectableAdvertisingControllerPreparationPhase,
        BluetoothLegacyConnectableAdvertisingRollbackFailure,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>
{
    pub(crate) fn restore_legacy_connectable_advertising_cancelled_in_place(
        &mut self,
        cancelled: Result<
            BluetoothLegacyConnectableAdvertisingCancelled,
            BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        >,
    ) -> timed_preparation::BluetoothTimedPreparationRollbackOutcome<
        BluetoothLegacyConnectableAdvertisingRollbackFailure,
    > {
        let cancelled = match cancelled {
            Ok(cancelled) => cancelled,
            Err(owner) => {
                return timed_preparation::BluetoothTimedPreparationRollbackOutcome::FailStop(
                    BluetoothLegacyConnectableAdvertisingRollbackFailure::Cancellation {
                        _owner: owner,
                    },
                );
            }
        };
        match self
            .legacy_connectable_advertising_resources
            .restore_cancelled(cancelled, self.peripheral_connection_resources)
        {
            Ok(_definition) => {
                timed_preparation::BluetoothTimedPreparationRollbackOutcome::Restored
            }
            Err(owner) => timed_preparation::BluetoothTimedPreparationRollbackOutcome::FailStop(
                BluetoothLegacyConnectableAdvertisingRollbackFailure::RuntimeRestore {
                    _owner: owner,
                },
            ),
        }
    }

    pub(crate) fn restore_legacy_connectable_advertising_cancelled_with<R, Context>(
        mut self,
        cancelled: Result<
            BluetoothLegacyConnectableAdvertisingCancelled,
            BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        >,
        context: Context,
        restored: impl FnOnce(Context, Self) -> R,
        fail_stop: impl FnOnce(Context, Self, BluetoothLegacyConnectableAdvertisingRollbackFailure) -> R,
    ) -> R {
        match self.restore_legacy_connectable_advertising_cancelled_in_place(cancelled) {
            timed_preparation::BluetoothTimedPreparationRollbackOutcome::Restored => {
                restored(context, self)
            }
            timed_preparation::BluetoothTimedPreparationRollbackOutcome::FailStop(owner) => {
                fail_stop(context, self, owner)
            }
        }
    }

    fn begin_legacy_connectable_advertising_preparation_time_with<R, Context>(
        self,
        phase: BluetoothLegacyConnectableAdvertisingControllerPreparationPhase,
        context: Context,
        pending: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        let timed = timed_preparation::BluetoothTimedPreparationPending::begin(
            self,
            phase,
            |controller, phase| {
                let cancelled = match phase {
                    BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                        prepared,
                        ..
                    } => prepared.cancel(),
                    BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Admission(
                        candidate,
                    ) => candidate.cancel(),
                    BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Sequence(
                        admitted,
                    ) => controller
                        .runtime
                        .cancel_legacy_connectable_advertising_pre_sequence(admitted),
                };
                controller.restore_legacy_connectable_advertising_cancelled_in_place(cancelled)
            },
        );
        match timed {
            Ok(timed) => pending(
                context,
                BluetoothLegacyConnectableAdvertisingControllerPreparationPending { timed },
            ),
            Err(failure) => fail_stop(
                context,
                BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop::from_timed(
                    failure,
                ),
            ),
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    >
{
    fn rollback_after_idle_with<R, Context>(
        controller: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        cancelled: Result<
            BluetoothLegacyConnectableAdvertisingCancelled,
            BluetoothLegacyConnectableAdvertisingCancellationInvariant,
        >,
        error: BluetoothLegacyConnectableAdvertisingControllerPreparationError,
        context: Context,
        recovered: impl FnOnce(
            Context,
            BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            BluetoothLegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
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
                    BluetoothControllerSchedulerEpochRetained { controller },
                    error,
                )
            },
            |context, controller, rollback| {
                let cause = BluetoothLegacyConnectableAdvertisingControllerFailStopCause::Rollback(
                    rollback.kind(),
                );
                fail_stop(
                    context,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop::active(
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
            BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        ready: impl FnOnce(
            Context,
            BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            BluetoothLegacyConnectableAdvertisingEmptySchedulerMergePrepared,
        ) -> R,
        recovered: impl FnOnce(
            Context,
            BluetoothControllerSchedulerEpochRetained<'runtime, S, SCHEDULER_CAPACITY>,
            BluetoothLegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
    ) -> R {
        let (mut controller, phase, sample) = match self.timed.recheck() {
            timed_preparation::BluetoothTimedPreparationStep::Waiting(timed) => {
                return pending(context, Self { timed });
            }
            timed_preparation::BluetoothTimedPreparationStep::Ready {
                controller,
                phase,
                sample,
            } => (controller, phase, sample),
            timed_preparation::BluetoothTimedPreparationStep::FailStop(failure) => {
                return fail_stop(
                    context,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop::from_timed(
                        failure,
                    ),
                );
            }
        };
        match phase {
            BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                prepared,
                now,
            } => {
                let epoch = now.epoch();
                let current = BluetoothSchedulerInstant::from_image(now.micros());
                let radio_ready = controller
                    .ble_phy_timing
                    .complete_always_awake(epoch, sample)
                    .into_scheduler_instant();
                let timing = BluetoothLegacyAdvertisingTimingObservation {
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
                            BluetoothLegacyConnectableAdvertisingControllerPreparationError::TimingWindow,
                            context,
                            recovered,
                            fail_stop,
                        );
                    }
                };
                controller.begin_legacy_connectable_advertising_preparation_time_with(
                    BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Admission(
                        candidate,
                    ),
                    context,
                    pending,
                    fail_stop,
                )
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Admission(
                candidate,
            ) => {
                let admitted = match controller
                    .runtime
                    .admit_legacy_connectable_advertising_first_event(
                        candidate,
                        BluetoothLegacyConnectableAdvertisingAdmissionObservation { sample },
                    ) {
                    Ok(admitted) => admitted,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle_with(
                            controller,
                            failure.into_candidate().cancel(),
                            BluetoothLegacyConnectableAdvertisingControllerPreparationError::Event(
                                error,
                            ),
                            context,
                            recovered,
                            fail_stop,
                        );
                    }
                };
                controller.begin_legacy_connectable_advertising_preparation_time_with(
                    BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Sequence(
                        admitted,
                    ),
                    context,
                    pending,
                    fail_stop,
                )
            }
            BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::Sequence(admitted) => {
                let prepared = match controller
                    .runtime
                    .prepare_legacy_connectable_advertising_event(
                        admitted,
                        BluetoothLegacyConnectableAdvertisingSequenceObservation { sample },
                    ) {
                    Ok(prepared) => prepared,
                    Err(failure) => {
                        let error = failure.error();
                        return Self::rollback_after_idle_with(
                            controller,
                            failure.into_candidate().cancel(),
                            BluetoothLegacyConnectableAdvertisingControllerPreparationError::Event(
                                error,
                            ),
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
                        BluetoothControllerSchedulerEpochRetained { controller },
                        merged,
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        let cancelled = controller
                            .runtime
                            .cancel_legacy_connectable_advertising_event(
                                failure.into_prepared(),
                            );
                        Self::rollback_after_idle_with(
                            controller,
                            cancelled,
                            BluetoothLegacyConnectableAdvertisingControllerPreparationError::EmptyList(
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
    BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>
{
    /// Begin the first response-capable event from one cold or warm current.
    pub(crate) fn begin_legacy_connectable_advertising_first_event_with<R, Context>(
        self,
        definition: BluetoothLegacyConnectableAdvertisingSetPrepared,
        context: Context,
        pending: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationPending<
                'runtime,
                S,
                SCHEDULER_CAPACITY,
            >,
        ) -> R,
        recovered: impl FnOnce(
            Context,
            BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
            BluetoothLegacyConnectableAdvertisingControllerPreparationError,
        ) -> R,
        fail_stop: impl FnOnce(
            Context,
            BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop<
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
            Err(BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::GenerationExhausted) => {
                return recovered(
                    context,
                    current,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationError::GenerationExhausted,
                );
            }
            Err(BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PduFit {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationError::PduFit(error),
                );
            }
            Err(BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::AdvertisingEventActive {
                definition: _,
            }) => {
                return recovered(
                    context,
                    current,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationError::AdvertisingEventActive,
                );
            }
            Err(BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::PeripheralEventActive {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationError::PeripheralEventActive(error),
                );
            }
            Err(BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::MemoryPreparation {
                definition: _,
                error,
            }) => {
                return recovered(
                    context,
                    current,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationError::MemoryPreparation(error),
                );
            }
            Err(failure @ BluetoothLegacyConnectableAdvertisingRuntimeBeginFailure::OwnershipInvariant { .. }) => {
                return fail_stop(
                    context,
                    BluetoothLegacyConnectableAdvertisingControllerPreparationFailStop {
                        cause: BluetoothLegacyConnectableAdvertisingControllerFailStopCause::RuntimeOwnership,
                        _state:
                            BluetoothLegacyConnectableAdvertisingControllerFailStopState::Initial {
                                _current: current,
                                _failure: failure,
                            },
                    },
                );
            }
        };
        let (controller, now) = current.into_parts();
        controller.begin_legacy_connectable_advertising_preparation_time_with(
            BluetoothLegacyConnectableAdvertisingControllerPreparationPhase::AlwaysAwakeTiming {
                prepared,
                now,
            },
            context,
            pending,
            fail_stop,
        )
    }
}
