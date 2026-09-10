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
    Control(oer_bluetooth_ll::control::LePeripheralControlError),
    RetirementIdentityMismatch,
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
    Stopped {
        task: Task<'a, S, N>,
        reason: u8,
    },
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
    Control(
        Task<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
        Evidence,
    ),
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
            Self::Stopped { .. } => Some(PeripheralConnectionActiveWait::ControllerTime),
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

    pub(super) fn step(
        self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        supervision: &mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        match self {
            stopped @ Self::Stopped { .. } => Step::Continue(stopped),
            Self::Running(running) => Self::poll_running(running),
            Self::Completed(completed) => Self::complete(completed, control, supervision),
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
                    Self::begin_current(task, admitted, evidence, false)
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
                Ok(ctrl::ControllerSchedulerCurrentStep::Ready(now)) => {
                    Self::finish_current(now, admitted, evidence, *supervision)
                }
            },
            Self::Merged {
                task,
                merged,
                evidence,
            } => Self::publish(task, merged, evidence),
        }
    }

    #[inline(never)]
    fn publish(
        mut task: Task<'a, S, N>,
        merged: sched::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        evidence: Evidence,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

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
            ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Break(failure))) => fault(
                Cause::Publication(failure.error()),
                FaultOwner::Publication(task, failure, evidence),
            ),
            ControlFlow::Continue(ControlFlow::Continue(ControlFlow::Continue(running))) => {
                Step::Published(Self::Running(Running::from_recurring(
                    task,
                    running,
                    event_counter,
                    evidence,
                )))
            }
        }
    }

    #[inline(never)]
    fn poll_running(running: Running<'a, S, N>) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        running.step_radio_with(
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
        )
    }

    // LL/control completion and candidate formation own separate large results.
    #[inline(never)]
    fn complete(
        completed: Completed<'a, S, N>,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        supervision: &mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

        let (mut task, mut completed, evidence) = completed.into_parts();
        if let Err(error) = task.process_peripheral_control(&mut completed, control) {
            if let oer_bluetooth_ll::control::LePeripheralControlError::PeerTermination { reason } =
                error
            {
                return Self::retire(task, completed, evidence, reason);
            }
            return fault(
                Cause::Control(error),
                FaultOwner::Control(task, completed, evidence),
            );
        }
        if completed.link_layer_completion().establishment_failed() {
            // HCI error: Connection Failed to be Established. Reclaim
            // only this closed, unlinked event; no live RUN is aborted.
            return Self::retire(task, completed, evidence, 0x3e);
        }
        *supervision = task.peripheral_supervision_deadline(&completed);
        // Only contiguous successors are composed; peripheral latency
        // and skipped-event recovery remain separate policies.
        let delta = oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::new(1).unwrap();
        match task.prepare_peripheral_connection_recurring_candidate(completed, delta) {
            ctrl::PeripheralConnectionRecurringCandidateStep::Prepared(candidate) => {
                Step::Continue(Self::Candidate {
                    task,
                    candidate,
                    evidence,
                })
            }
            ctrl::PeripheralConnectionRecurringCandidateStep::SchedulerEpochUnavailable(retry) => {
                fault(
                    Cause::SchedulerEpochUnavailable,
                    FaultOwner::Candidate(task, retry, evidence),
                )
            }
            ctrl::PeripheralConnectionRecurringCandidateStep::TimingPolicyUnavailable(retry) => {
                fault(
                    Cause::TimingPolicyUnavailable,
                    FaultOwner::Candidate(task, retry, evidence),
                )
            }
            ctrl::PeripheralConnectionRecurringCandidateStep::Rejected { error, retry } => fault(
                Cause::Candidate(error),
                FaultOwner::Candidate(task, retry, evidence),
            ),
        }
    }

    // Keep the large affine cancellation/result owners out of step's frame.
    #[inline(never)]
    fn finish_current(
        now: ctrl::ControllerSchedulerNowReady<'a, S, N>,
        admitted: sched::PeripheralConnectionRecurringPreSequence,
        evidence: Evidence,
        supervision: Option<super::super::supervision::PeripheralSupervisionDeadline>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

        if let Some(deadline) = supervision {
            use super::super::supervision::PeripheralSupervisionDecision;
            match now.check_peripheral_supervision(deadline, &admitted) {
                PeripheralSupervisionDecision::Expired => {
                    let mut task = now.into_retained_epoch().into_task_service();
                    let (completed, _) = task
                        .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                        .into_parts();
                    return Self::retire(task, completed, evidence, 0x08);
                }
                PeripheralSupervisionDecision::Wait => {
                    let task = now.into_retained_epoch().into_task_service();
                    return Self::begin_current(task, admitted, evidence, true);
                }
                PeripheralSupervisionDecision::Run => {}
            }
        }
        match now.finish_peripheral_connection_recurring_event(admitted) {
            ctrl::PeripheralConnectionRecurringSequenceCompletion::Prepared { task, merged } => {
                Step::Continue(Self::Merged {
                    task,
                    merged,
                    evidence,
                })
            }
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
        }
    }

    #[inline(never)]
    fn begin_current(
        task: Task<'a, S, N>,
        admitted: sched::PeripheralConnectionRecurringPreSequence,
        evidence: Evidence,
        wait_for_recheck: bool,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
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
                wait_for_recheck,
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

    fn retire(
        mut task: Task<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
        reason: u8,
    ) -> Step<'a, S, N> {
        match task.retire_peripheral_connection(completed) {
            ControlFlow::Continue(()) => Step::Continue(Self::Stopped { task, reason }),
            ControlFlow::Break(completed) => fault(
                PeripheralConnectionActiveFaultCause::RetirementIdentityMismatch,
                FaultOwner::Control(task, completed, evidence),
            ),
        }
    }
}
