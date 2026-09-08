//! One finite completion or successor-preparation transition per call.

use super::super::{
    LegacyConnectablePeripheralFirstCompleted as Completed,
    LegacyConnectablePeripheralFirstCompletionFailStop as CompletionFault,
    LegacyConnectablePeripheralFirstNormalizationUnavailable as Normalization,
    LegacyConnectablePeripheralFirstRecycleFailStop as RecycleFault,
    LegacyConnectablePeripheralFirstRunning as Running,
    LegacyConnectablePeripheralFirstRunningContinuations as Continuations,
    LegacyConnectablePeripheralFirstRunningEvidence as Evidence,
    LegacyConnectablePeripheralFirstRunningWait as RunningWait,
};
use crate::{controller as ctrl, scheduler as sched};
use core::ops::ControlFlow;
use ctrl::SchedulerRunInterruptStorage;

type Task<'a, S, const N: usize> = ctrl::ControllerPublishedTaskService<'a, S, N>;

/// Wake source borrowed from the exact retained connection transaction.
pub enum PeripheralConnectionActiveWait<'a> {
    Scheduler(&'a crate::interrupt::SchedulerWakeCell),
    PostUnlink(&'a crate::le::dtm::DtmPostUnlinkWakeCell),
    ControllerTime,
}

/// Failure closes this active lifecycle without reclaiming its affine owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionActiveFaultCause {
    Completion(super::super::LegacyConnectablePeripheralFirstCompletionFailStopCause),
    Recycle(super::super::LegacyConnectablePeripheralFirstRecycleFailStopCause),
    UnrelatedFinishedList,
    SchedulerEpochUnavailable,
    TimingPolicyUnavailable,
    Candidate(sched::PeripheralConnectionRecurringCandidateError),
    Preparation(sched::PeripheralConnectionRecurringEventPreparationError),
    ControllerTimeBegin(ctrl::ControllerSchedulerCurrentBeginError),
    ControllerTime(ctrl::ControllerSchedulerCurrentError),
    EmptyList(sched::SchedulerEmptyListMergeError),
    Validation(sched::SchedulerHeadPublicationError),
    InterruptStorage,
    Publication(oer_esp32s31_bluetooth_memory::PeripheralConnectionMemoryGraphPublicationError),
}

pub(super) enum Radio<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Running(Running<'a, S, N>),
    Completed(Completed<'a, S, N>),
    Candidate {
        task: Task<'a, S, N>,
        candidate: sched::PeripheralConnectionRecurringEventCandidate,
        evidence: Evidence,
    },
    Current {
        wait_for_recheck: bool,
        pending: ctrl::ControllerSchedulerCurrentPending<'a, S, N>,
        admitted: sched::PeripheralConnectionRecurringPreSequence,
        evidence: Evidence,
    },
    Merged {
        task: Task<'a, S, N>,
        merged: sched::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        evidence: Evidence,
    },
}

pub(super) enum Step<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Continue(Radio<'a, S, N>),
    Published(Radio<'a, S, N>),
    Fault(Fault<'a, S, N>),
}

pub(super) struct Fault<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    pub(super) cause: PeripheralConnectionActiveFaultCause,
    _owner: FaultOwner<'a, S, N>,
}

// Every variant deliberately retains a sealed lower owner, including task,
// reservation and advertising provenance. None grants a retry/reclaim path.
#[allow(
    dead_code,
    reason = "sealed affine failure owners are retained, never inspected or replayed"
)]
enum FaultOwner<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Completion(CompletionFault<'a, S, N>),
    Recycle(RecycleFault<'a, S, N>),
    Unrelated(
        Running<'a, S, N>,
        sched::BluetoothSchedulerFinishedHardwareListObserved,
    ),
    Normalization(Normalization<'a, S, N>),
    Candidate(
        Task<'a, S, N>,
        ctrl::PeripheralConnectionRecurringRetry,
        Evidence,
    ),
    Preparation(
        Task<'a, S, N>,
        sched::PeripheralConnectionRecurringEventPreparationFailure,
        Evidence,
    ),
    Epoch(
        ctrl::ControllerSchedulerEpochUnavailable<'a, S, N>,
        sched::PeripheralConnectionRecurringPreSequence,
        Evidence,
    ),
    CurrentBegin(
        ctrl::ControllerSchedulerCurrentBeginFailure<'a, S, N>,
        sched::PeripheralConnectionRecurringPreSequence,
        Evidence,
    ),
    Current(
        ctrl::ControllerSchedulerCurrentFailure<'a, S, N>,
        sched::PeripheralConnectionRecurringPreSequence,
        Evidence,
    ),
    EmptyList(
        Task<'a, S, N>,
        sched::PeripheralConnectionRecurringEmptySchedulerMergeFailure,
        Evidence,
    ),
    Validation(
        Task<'a, S, N>,
        sched::core::PeripheralConnectionRecurringSchedulerValidationFailure,
        Evidence,
    ),
    InterruptStorage(
        Task<'a, S, N>,
        S::Error,
        sched::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        Evidence,
    ),
    Publication(
        Task<'a, S, N>,
        sched::core::PeripheralConnectionRecurringSchedulerPublicationFailStop,
        Evidence,
    ),
}

fn fault<'a, S: SchedulerRunInterruptStorage, const N: usize>(
    cause: PeripheralConnectionActiveFaultCause,
    owner: FaultOwner<'a, S, N>,
) -> Step<'a, S, N> {
    Step::Fault(Fault {
        cause,
        _owner: owner,
    })
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize> Radio<'a, S, N> {
    pub(super) fn wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        match self {
            Self::Running(running) => running.radio_wait().map(|wait| match wait {
                RunningWait::Scheduler(wake) => PeripheralConnectionActiveWait::Scheduler(wake),
                RunningWait::PostUnlink(wake) => PeripheralConnectionActiveWait::PostUnlink(wake),
            }),
            Self::Current {
                wait_for_recheck: true,
                ..
            } => Some(PeripheralConnectionActiveWait::ControllerTime),
            Self::Current {
                wait_for_recheck: false,
                ..
            } => None,
            Self::Completed(_) | Self::Candidate { .. } | Self::Merged { .. } => None,
        }
    }

    pub(super) fn step(self) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        match self {
            Self::Running(running) => running.step_radio_with(
                (),
                Continuations::new(
                    |(), running| Step::Continue(Self::Running(running)),
                    |(), running| Step::Continue(Self::Running(running)),
                    |(), running, observed| {
                        fault(
                            Cause::UnrelatedFinishedList,
                            FaultOwner::Unrelated(running, observed),
                        )
                    },
                    |(), owner| {
                        fault(
                            Cause::SchedulerEpochUnavailable,
                            FaultOwner::Normalization(owner),
                        )
                    },
                    |(), completed| Step::Continue(Self::Completed(completed)),
                    |(), owner: CompletionFault<'a, S, N>| {
                        fault(
                            Cause::Completion(owner.cause()),
                            FaultOwner::Completion(owner),
                        )
                    },
                    |(), owner: RecycleFault<'a, S, N>| {
                        fault(Cause::Recycle(owner.cause()), FaultOwner::Recycle(owner))
                    },
                ),
            ),
            Self::Completed(completed) => {
                let (mut task, completed, evidence) = completed.into_parts();
                // Only contiguous successors are composed. Missed-anchor recovery,
                // peripheral latency and supervision policy remain separate work.
                let delta =
                    oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::new(1).unwrap();
                match task.prepare_peripheral_connection_recurring_candidate(completed, delta) {
                    ctrl::PeripheralConnectionRecurringCandidateStep::Prepared(candidate) => {
                        Step::Continue(Self::Candidate {
                            task,
                            candidate,
                            evidence,
                        })
                    }
                    ctrl::PeripheralConnectionRecurringCandidateStep::SchedulerEpochUnavailable(
                        retry,
                    ) => fault(
                        Cause::SchedulerEpochUnavailable,
                        FaultOwner::Candidate(task, retry, evidence),
                    ),
                    ctrl::PeripheralConnectionRecurringCandidateStep::TimingPolicyUnavailable(
                        retry,
                    ) => fault(
                        Cause::TimingPolicyUnavailable,
                        FaultOwner::Candidate(task, retry, evidence),
                    ),
                    ctrl::PeripheralConnectionRecurringCandidateStep::Rejected { error, retry } => {
                        fault(
                            Cause::Candidate(error),
                            FaultOwner::Candidate(task, retry, evidence),
                        )
                    }
                }
            }
            Self::Candidate {
                mut task,
                candidate,
                evidence,
            } => match task.admit_peripheral_connection_recurring_candidate(candidate) {
                ControlFlow::Break(failure) => fault(
                    Cause::Preparation(failure.error()),
                    FaultOwner::Preparation(task, failure, evidence),
                ),
                ControlFlow::Continue(admitted) => {
                    let retained = match task.retain_scheduler_epoch() {
                        Ok(retained) => retained,
                        Err(unavailable) => {
                            return fault(
                                Cause::SchedulerEpochUnavailable,
                                FaultOwner::Epoch(unavailable, admitted, evidence),
                            );
                        }
                    };
                    match retained.begin_fresh_scheduler_current() {
                        Ok(pending) => Step::Continue(Self::Current {
                            wait_for_recheck: false,
                            pending,
                            admitted,
                            evidence,
                        }),
                        Err(failure) => fault(
                            Cause::ControllerTimeBegin(failure.error()),
                            FaultOwner::CurrentBegin(failure, admitted, evidence),
                        ),
                    }
                }
            },
            Self::Current {
                pending,
                admitted,
                evidence,
                ..
            } => match pending.recheck() {
                Err(failure) => fault(
                    Cause::ControllerTime(failure.error()),
                    FaultOwner::Current(failure, admitted, evidence),
                ),
                Ok(ctrl::ControllerSchedulerCurrentStep::Waiting(pending)) => {
                    Step::Continue(Self::Current {
                        wait_for_recheck: true,
                        pending,
                        admitted,
                        evidence,
                    })
                }
                Ok(ctrl::ControllerSchedulerCurrentStep::Ready(now)) => match now
                    .finish_peripheral_connection_recurring_event(admitted)
                {
                    ctrl::PeripheralConnectionRecurringSequenceCompletion::Prepared {
                        task,
                        merged,
                    } => Step::Continue(Self::Merged {
                        task,
                        merged,
                        evidence,
                    }),
                    ctrl::PeripheralConnectionRecurringSequenceCompletion::EventRejected {
                        task,
                        failure,
                    } => fault(
                        Cause::Preparation(failure.error()),
                        FaultOwner::Preparation(task, failure, evidence),
                    ),
                    ctrl::PeripheralConnectionRecurringSequenceCompletion::EmptyListRejected {
                        task,
                        failure,
                    } => fault(
                        Cause::EmptyList(failure.error()),
                        FaultOwner::EmptyList(task, failure, evidence),
                    ),
                },
            },
            Self::Merged {
                mut task,
                merged,
                evidence,
            } => {
                let event_counter = merged.event_counter();
                match task.start_peripheral_connection_recurring_scheduler(merged) {
                    ControlFlow::Break(failure) => fault(
                        Cause::Validation(failure.error()),
                        FaultOwner::Validation(task, failure, evidence),
                    ),
                    ControlFlow::Continue(ControlFlow::Break((error, merged))) => fault(
                        Cause::InterruptStorage,
                        FaultOwner::InterruptStorage(task, error, merged, evidence),
                    ),
                    ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure))) => {
                        fault(
                            Cause::Publication(failure.error()),
                            FaultOwner::Publication(task, failure, evidence),
                        )
                    }
                    ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(
                        running,
                    ))) => Step::Published(Self::Running(Running::from_recurring(
                        task,
                        running,
                        event_counter,
                        evidence,
                    ))),
                }
            }
        }
    }
}
