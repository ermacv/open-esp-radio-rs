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

struct LinkState<'a> {
    control: &'a mut oer_bluetooth_ll::control::LePeripheralControl,
    acl: &'a mut super::acl::PeripheralConnectionAcl,
    supervision: &'a mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
    termination: &'a mut Option<super::super::termination::PeripheralTerminationDeadline>,
    procedure: &'a mut Option<super::super::procedure::PeripheralProcedureDeadline>,
    host_events: &'a mut super::host_events::PeripheralConnectionHostEvents,
}

/// Wake source borrowed from the exact retained connection transaction.
pub enum PeripheralConnectionActiveWait<'a> {
    Scheduler(&'a crate::interrupt::SchedulerWakeCell),
    PostUnlink(&'a crate::le::dtm::DtmPostUnlinkWakeCell),
    ControllerTime,
    HostEventCapacity,
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
    RecoveryDeltaUnavailable,
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
    TerminationCurrent {
        wait_for_recheck: bool,
        pending: ctrl::ControllerSchedulerCurrentPending<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
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

pub(super) enum ResetStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Continue(Radio<'a, S, N>),
    Quiesced(Task<'a, S, N>),
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
    Recovery(
        Task<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
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
    TerminationEpoch(
        ctrl::ControllerSchedulerEpochUnavailable<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
        Evidence,
    ),
    CurrentBegin(
        ctrl::ControllerSchedulerCurrentBeginFailure<'a, S, N>,
        sched::PeripheralConnectionRecurringPreSequence,
        Evidence,
    ),
    TerminationCurrentBegin(
        ctrl::ControllerSchedulerCurrentBeginFailure<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
        Evidence,
    ),
    Current(
        ctrl::ControllerSchedulerCurrentFailure<'a, S, N>,
        sched::PeripheralConnectionRecurringPreSequence,
        Evidence,
    ),
    TerminationCurrent(
        ctrl::ControllerSchedulerCurrentFailure<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
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
    pub(super) fn completed_receive_batch_len(&self) -> Option<usize> {
        match self {
            Self::Completed(completed) => Some(completed.connection().received().len()),
            _ => None,
        }
    }

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
            }
            | Self::TerminationCurrent {
                wait_for_recheck: true,
                ..
            } => Some(PeripheralConnectionActiveWait::ControllerTime),
            Self::Current {
                wait_for_recheck: false,
                ..
            }
            | Self::TerminationCurrent {
                wait_for_recheck: false,
                ..
            } => None,
            Self::Completed(_) | Self::Candidate { .. } | Self::Merged { .. } => None,
        }
    }

    pub(super) fn step(
        self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        acl: &mut super::acl::PeripheralConnectionAcl,
        supervision: &mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
        termination: &mut Option<super::super::termination::PeripheralTerminationDeadline>,
        procedure: &mut Option<super::super::procedure::PeripheralProcedureDeadline>,
        host_events: &mut super::host_events::PeripheralConnectionHostEvents,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        let mut link = LinkState {
            control,
            acl,
            supervision,
            termination,
            procedure,
            host_events,
        };
        match self {
            stopped @ Self::Stopped { .. } => Step::Continue(stopped),
            Self::Running(running) => Self::poll_running(running),
            Self::Completed(completed) => Self::complete(completed, &mut link),
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
                    Self::finish_current(now, admitted, evidence, *link.supervision, &mut link)
                }
            },
            Self::TerminationCurrent {
                pending,
                completed,
                evidence,
                ..
            } => match pending.recheck() {
                Err(failure) => fault(
                    Cause::ControllerTime(failure.error()),
                    FaultOwner::TerminationCurrent(failure, completed, evidence),
                ),
                Ok(ctrl::ControllerSchedulerCurrentStep::Waiting(pending)) => {
                    Step::Continue(Self::TerminationCurrent {
                        wait_for_recheck: true,
                        pending,
                        completed,
                        evidence,
                    })
                }
                Ok(ctrl::ControllerSchedulerCurrentStep::Ready(now)) => {
                    let reference = now.peripheral_current_instant();
                    let task = now.into_retained_epoch().into_task_service();
                    Self::complete_ready(task, completed, evidence, Some(reference), &mut link)
                }
            },
            Self::Merged {
                task,
                merged,
                evidence,
            } => Self::publish(task, merged, evidence),
        }
    }

    /// Drive active ownership to a CPU-owned retired graph for HCI Reset.
    ///
    /// Published RUN work completes normally. Unpublished preparation may
    /// finish and publish at most its already-admitted successor before the
    /// next completion is retired; no new recurrence is constructed.
    pub(super) fn step_reset(
        self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        acl: &mut super::acl::PeripheralConnectionAcl,
        supervision: &mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
        termination: &mut Option<super::super::termination::PeripheralTerminationDeadline>,
        procedure: &mut Option<super::super::procedure::PeripheralProcedureDeadline>,
        host_events: &mut super::host_events::PeripheralConnectionHostEvents,
    ) -> ResetStep<'a, S, N> {
        match self {
            Self::Stopped { task, .. } => ResetStep::Quiesced(task),
            Self::Completed(completed) => {
                let (mut task, completed, evidence) = completed.into_parts();
                match task.retire_peripheral_connection(completed) {
                    ControlFlow::Continue(()) => ResetStep::Quiesced(task),
                    ControlFlow::Break(completed) => ResetStep::Fault(Fault {
                        cause: PeripheralConnectionActiveFaultCause::RetirementIdentityMismatch,
                        _owner: FaultOwner::Control(task, completed, evidence),
                    }),
                }
            }
            Self::TerminationCurrent {
                pending,
                completed,
                evidence,
                ..
            } => match pending.recheck() {
                Err(failure) => ResetStep::Fault(Fault {
                    cause: PeripheralConnectionActiveFaultCause::ControllerTime(failure.error()),
                    _owner: FaultOwner::TerminationCurrent(failure, completed, evidence),
                }),
                Ok(ctrl::ControllerSchedulerCurrentStep::Waiting(pending)) => {
                    ResetStep::Continue(Self::TerminationCurrent {
                        wait_for_recheck: true,
                        pending,
                        completed,
                        evidence,
                    })
                }
                Ok(ctrl::ControllerSchedulerCurrentStep::Ready(now)) => {
                    let mut task = now.into_retained_epoch().into_task_service();
                    match task.retire_peripheral_connection(completed) {
                        ControlFlow::Continue(()) => ResetStep::Quiesced(task),
                        ControlFlow::Break(completed) => ResetStep::Fault(Fault {
                            cause: PeripheralConnectionActiveFaultCause::RetirementIdentityMismatch,
                            _owner: FaultOwner::Control(task, completed, evidence),
                        }),
                    }
                }
            },
            radio => match radio.step(
                control,
                acl,
                supervision,
                termination,
                procedure,
                host_events,
            ) {
                Step::Continue(radio) | Step::Published(radio) => ResetStep::Continue(radio),
                Step::Fault(fault) => ResetStep::Fault(fault),
            },
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
    fn complete(completed: Completed<'a, S, N>, link: &mut LinkState<'_>) -> Step<'a, S, N> {
        if !link
            .acl
            .can_accept_controller_batch(completed.connection().received().len())
        {
            return Step::Continue(Self::Completed(completed));
        }

        let (task, completed, evidence) = completed.into_parts();
        if (link.termination.is_none() && link.control.local_termination_queued())
            || (link.procedure.is_none()
                && (link.control.local_feature_request_queued()
                    || link.control.local_version_request_queued()))
            || (link.procedure.is_some()
                && (link.control.pending_response().is_some() || !completed.received().is_empty()))
        {
            return Self::begin_termination_current(task, completed, evidence, false);
        }
        Self::complete_ready(task, completed, evidence, None, link)
    }

    #[inline(never)]
    fn complete_ready(
        mut task: Task<'a, S, N>,
        mut completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
        termination_reference: Option<crate::SchedulerInstant>,
        link: &mut LinkState<'_>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

        let termination_timeout = completed
            .link_layer_completion()
            .timing()
            .supervision_timeout_micros();
        let control_result =
            task.process_peripheral_control(&mut completed, link.control, link.acl);
        if link.termination.is_none() {
            *link.termination =
                super::super::termination::PeripheralTerminationDeadline::after_graph_update(
                    termination_reference,
                    termination_timeout,
                    link.control.local_termination_queued(),
                    link.control.local_termination_reason(),
                );
        }
        *link.procedure = super::super::procedure::PeripheralProcedureDeadline::after_graph_update(
            *link.procedure,
            termination_reference,
            link.control.local_feature_request_transmitted()
                || link.control.local_version_request_transmitted(),
            matches!(control_result, Ok(true)),
        );
        if !link
            .host_events
            .observe_completion(completed.link_layer_completion())
        {
            link.control.request_local_termination(0x1f);
        }
        if let Err(error) = control_result {
            link.host_events
                .observe_acl_completed(link.acl.take_completed_host_packets());
            if let oer_bluetooth_ll::control::LePeripheralControlError::PeerTermination { reason } =
                error
            {
                return Self::retire(
                    task,
                    completed,
                    evidence,
                    reason,
                    true,
                    link.acl,
                    link.host_events,
                );
            }
            if let Some(reason) = error.termination_reason() {
                link.control.request_local_termination(reason);
            } else {
                return fault(
                    Cause::Control(error),
                    FaultOwner::Control(task, completed, evidence),
                );
            }
        }
        link.host_events
            .observe_acl_completed(link.acl.take_completed_host_packets());
        if let Some(reason) = link.control.local_termination_acknowledged_reason() {
            return Self::retire(
                task,
                completed,
                evidence,
                reason,
                true,
                link.acl,
                link.host_events,
            );
        }
        if completed.link_layer_completion().establishment_failed() {
            // HCI error: Connection Failed to be Established. Reclaim
            // only this closed, unlinked event; no live RUN is aborted.
            return Self::retire(
                task,
                completed,
                evidence,
                0x3e,
                false,
                link.acl,
                link.host_events,
            );
        }
        *link.supervision = task.peripheral_supervision_deadline(&completed);
        // Start with the contiguous successor. A fresh pre-publication sample
        // may rebuild this same completed owner at a later established event.
        let delta = oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::new(1).unwrap();
        Self::prepare_candidate(task, completed, delta, evidence, link)
    }

    fn prepare_candidate(
        mut task: Task<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        delta: oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta,
        evidence: Evidence,
        link: &mut LinkState<'_>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
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
            ctrl::PeripheralConnectionRecurringCandidateStep::Rejected {
                error:
                    sched::PeripheralConnectionRecurringCandidateError::ConnectionUpdateInstantSkipped
                    | sched::PeripheralConnectionRecurringCandidateError::ChannelMapUpdateInstantSkipped,
                retry,
            } => {
                let (completed, _) = retry.into_parts();
                Self::retire(
                    task,
                    completed,
                    evidence,
                    0x28,
                    true,
                    link.acl,
                    link.host_events,
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
        link: &mut LinkState<'_>,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

        if let Some(deadline) = *link.termination {
            use super::super::termination::PeripheralTerminationDecision;
            match now.check_peripheral_termination(deadline, &admitted) {
                PeripheralTerminationDecision::Expired => {
                    let reason = link
                        .control
                        .local_termination_reason()
                        .expect("an armed termination deadline retains its reason");
                    let mut task = now.into_retained_epoch().into_task_service();
                    let (completed, _) = task
                        .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                        .into_parts();
                    return Self::retire(
                        task,
                        completed,
                        evidence,
                        reason,
                        true,
                        link.acl,
                        link.host_events,
                    );
                }
                PeripheralTerminationDecision::Wait => {
                    let task = now.into_retained_epoch().into_task_service();
                    return Self::begin_current(task, admitted, evidence, true);
                }
                PeripheralTerminationDecision::Run => {}
            }
        }
        if let Some(deadline) = *link.procedure {
            use super::super::procedure::PeripheralProcedureDecision;
            match now.check_peripheral_procedure(deadline, &admitted) {
                PeripheralProcedureDecision::ConnectionLost { reason } => {
                    link.control.expire_local_procedure();
                    *link.procedure = None;
                    let mut task = now.into_retained_epoch().into_task_service();
                    let (completed, _) = task
                        .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                        .into_parts();
                    return Self::retire(
                        task,
                        completed,
                        evidence,
                        reason,
                        true,
                        link.acl,
                        link.host_events,
                    );
                }
                PeripheralProcedureDecision::Wait => {
                    let task = now.into_retained_epoch().into_task_service();
                    return Self::begin_current(task, admitted, evidence, true);
                }
                PeripheralProcedureDecision::Run => {}
            }
        }
        if let Some(deadline) = supervision {
            use super::super::supervision::PeripheralSupervisionDecision;
            match now.check_peripheral_supervision(deadline, &admitted) {
                PeripheralSupervisionDecision::Expired => {
                    let mut task = now.into_retained_epoch().into_task_service();
                    let (completed, _) = task
                        .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                        .into_parts();
                    return Self::retire(
                        task,
                        completed,
                        evidence,
                        0x08,
                        true,
                        link.acl,
                        link.host_events,
                    );
                }
                PeripheralSupervisionDecision::Wait => {
                    let task = now.into_retained_epoch().into_task_service();
                    return Self::begin_current(task, admitted, evidence, true);
                }
                PeripheralSupervisionDecision::Run => {}
            }
        }
        use super::super::recovery::PeripheralMissedAnchorDecision;
        match now.decide_peripheral_missed_anchor(&admitted) {
            PeripheralMissedAnchorDecision::Run
            | PeripheralMissedAnchorDecision::EstablishmentPolicyRequired => {}
            PeripheralMissedAnchorDecision::Skip(delta) => {
                let mut task = now.into_retained_epoch().into_task_service();
                let (completed, _) = task
                    .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                    .into_parts();
                return Self::prepare_candidate(task, completed, delta, evidence, link);
            }
            PeripheralMissedAnchorDecision::DeltaUnavailable => {
                let mut task = now.into_retained_epoch().into_task_service();
                let (completed, _) = task
                    .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                    .into_parts();
                return fault(
                    Cause::RecoveryDeltaUnavailable,
                    FaultOwner::Recovery(task, completed, evidence),
                );
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

    fn begin_termination_current(
        task: Task<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
        wait_for_recheck: bool,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        let retained = match task.retain_scheduler_epoch() {
            Ok(retained) => retained,
            Err(unavailable) => {
                return fault(
                    Cause::SchedulerEpochUnavailable,
                    FaultOwner::TerminationEpoch(unavailable, completed, evidence),
                );
            }
        };
        match retained.begin_fresh_scheduler_current() {
            Ok(pending) => Step::Continue(Self::TerminationCurrent {
                wait_for_recheck,
                pending,
                completed,
                evidence,
            }),
            Err(failure) => fault(
                Cause::ControllerTimeBegin(failure.error()),
                FaultOwner::TerminationCurrentBegin(failure, completed, evidence),
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
        publish_disconnection: bool,
        acl: &mut super::acl::PeripheralConnectionAcl,
        host_events: &mut super::host_events::PeripheralConnectionHostEvents,
    ) -> Step<'a, S, N> {
        match task.retire_peripheral_connection(completed) {
            ControlFlow::Continue(()) => {
                acl.cancel_host_packet();
                host_events.observe_acl_completed(acl.take_completed_host_packets());
                if publish_disconnection {
                    host_events.observe_disconnection(reason);
                }
                Step::Continue(Self::Stopped { task, reason })
            }
            ControlFlow::Break(completed) => fault(
                PeripheralConnectionActiveFaultCause::RetirementIdentityMismatch,
                FaultOwner::Control(task, completed, evidence),
            ),
        }
    }
}
