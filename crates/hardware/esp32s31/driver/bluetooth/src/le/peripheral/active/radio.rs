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

type Restoration = Option<oer_esp32s31_phy::tracking::deadline::TrackingDeadline>;

type Task<'a, S, const N: usize> = ctrl::ControllerPublishedTaskService<'a, S, N>;

use super::super::progress::PeripheralConnectionProgressDeadline as ProgressDeadline;

/// One acquisition keeps its own budget across every recheck and cancelled
/// executor wait. A new request starts a new budget, including after Host stall.
pub(super) struct TimedCurrent<'a, S, const N: usize> {
    pending: ctrl::ControllerSchedulerCurrentPending<'a, S, N>,
    deadline: ProgressDeadline,
}

impl<'a, S: SchedulerRunInterruptStorage, const N: usize> TimedCurrent<'a, S, N> {
    fn new(pending: ctrl::ControllerSchedulerCurrentPending<'a, S, N>) -> Self {
        Self {
            pending,
            deadline: ProgressDeadline::for_operation(S::monotonic_micros()),
        }
    }

    #[allow(
        clippy::result_large_err,
        reason = "the exact affine Controller is retained on failure"
    )]
    fn recheck(
        self,
    ) -> Result<
        ControlFlow<Self, ctrl::ControllerSchedulerNowReady<'a, S, N>>,
        ctrl::ControllerSchedulerCurrentFailure<'a, S, N>,
    > {
        let Self { pending, deadline } = self;
        match pending.recheck()? {
            ctrl::ControllerSchedulerCurrentStep::Waiting(pending) => {
                Ok(ControlFlow::Break(Self { pending, deadline }))
            }
            ctrl::ControllerSchedulerCurrentStep::Ready(now) => Ok(ControlFlow::Continue(now)),
        }
    }
}

pub(super) struct Deadlines<'a> {
    pub(super) supervision:
        &'a mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
    pub(super) termination:
        &'a mut Option<super::super::termination::PeripheralTerminationDeadline>,
    pub(super) procedure: &'a mut Option<super::super::procedure::PeripheralProcedureDeadline>,
}

struct LinkState<'a> {
    control: &'a mut oer_bluetooth_ll::control::LePeripheralControl,
    encryption: &'a mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
    acl: &'a mut super::acl::PeripheralConnectionAcl,
    supervision: &'a mut Option<super::super::supervision::PeripheralSupervisionDeadline>,
    termination: &'a mut Option<super::super::termination::PeripheralTerminationDeadline>,
    procedure: &'a mut Option<super::super::procedure::PeripheralProcedureDeadline>,
    host_events: &'a mut super::host_events::PeripheralConnectionHostEvents,
    random: &'a mut dyn super::super::PeripheralEncryptionRandomSource,
}

impl LinkState<'_> {
    fn deadlines(&self) -> super::super::deadlines::Deadlines {
        super::super::deadlines::Deadlines {
            termination: *self.termination,
            procedure: *self.procedure,
            supervision: *self.supervision,
        }
    }

    fn expire_deadline(&mut self, expired: super::super::deadlines::Expired) -> u8 {
        use super::super::deadlines::Expired;
        match expired {
            Expired::Termination => self
                .control
                .local_termination_completion_reason()
                .expect("an armed termination deadline retains its reason"),
            Expired::Procedure { reason } => {
                self.control.expire_local_procedure();
                *self.procedure = None;
                reason
            }
            Expired::Supervision => 0x08,
        }
    }
}

fn update_procedure_deadline(
    link: &mut LinkState<'_>,
    reference: Option<crate::SchedulerInstant>,
    packet_enqueued: bool,
) {
    let local_control_transmitted = link.control.local_feature_request_transmitted()
        || link.control.local_version_request_transmitted();
    if (link.encryption.is_active() || link.encryption.is_idle()) && !local_control_transmitted {
        *link.procedure = None;
        return;
    }
    *link.procedure = super::super::procedure::PeripheralProcedureDeadline::after_graph_update(
        *link.procedure,
        reference,
        link.encryption.blocks_unrelated_transmission() || local_control_transmitted,
        packet_enqueued,
    );
}

/// Wake source borrowed from the exact retained connection transaction.
pub enum PeripheralConnectionActiveWait<'a> {
    Scheduler(&'a crate::interrupt::SchedulerWakeCell),
    PostUnlink(&'a crate::le::dtm::DtmPostUnlinkWakeCell),
    ControllerTime,
    HostEventCapacityOrControllerTime,
}

/// Failure closes this active lifecycle without reclaiming its affine owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionActiveFaultCause {
    MaintenanceRestorationExpired,
    CompletionAbortDeadlineExpired,
    CompletionAbortInvariant,
    ControllerTimeDeadlineExpired,
    UnlinkDeadlineExpired,
    Completion(super::super::LegacyConnectablePeripheralFirstCompletionFailStopCause),
    Control(oer_bluetooth_ll::control::LePeripheralControlError),
    ControllerRxReservationInvariant,
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
    Stopping(Running<'a, S, N>),
    Completed(Completed<'a, S, N>),
    ControllerRxBackpressured {
        task: Task<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
    },
    ControllerRxCurrent {
        wait_for_recheck: bool,
        pending: TimedCurrent<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
    },
    Candidate {
        task: Task<'a, S, N>,
        candidate: sched::PeripheralConnectionRecurringEventCandidate,
        evidence: Evidence,
        restoration: Restoration,
    },
    Current {
        wait_for_recheck: bool,
        pending: TimedCurrent<'a, S, N>,
        admitted: sched::PeripheralConnectionRecurringPreSequence,
        evidence: Evidence,
        restoration: Restoration,
    },
    TerminationCurrent {
        wait_for_recheck: bool,
        pending: TimedCurrent<'a, S, N>,
        completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
    },
    Merged {
        task: Task<'a, S, N>,
        merged: sched::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        progress_deadline: ProgressDeadline,
        evidence: Evidence,
        restoration: Restoration,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeadlinePhase {
    MaintenanceRestoration,
    SchedulerCompletion,
    SchedulerStop,
    PostUnlink,
    ControllerTime,
}

pub(super) enum Step<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Continue(Radio<'a, S, N>),
    Published(
        Radio<'a, S, N>,
        Option<super::super::maintenance::PeripheralMaintenanceRun>,
    ),
    Fault(Fault<'a, S, N>),
}

pub(super) enum ResetStep<'a, S: SchedulerRunInterruptStorage, const N: usize> {
    Published(Radio<'a, S, N>),
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
    Deadline(Radio<'a, S, N>),
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
    ControllerRxEpoch(
        ctrl::ControllerSchedulerEpochUnavailable<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
        Evidence,
    ),
    ControllerRxCurrentBegin(
        ctrl::ControllerSchedulerCurrentBeginFailure<'a, S, N>,
        sched::PeripheralConnectionSchedulerCompleted,
        Evidence,
    ),
    ControllerRxCurrent(
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
    pub(super) fn progress_deadline(&self) -> ProgressDeadline {
        let Self::Running(running) = self else {
            unreachable!("only a new RUN supplies an event deadline")
        };
        running.progress_deadline()
    }

    pub(super) fn expired_phase(&mut self, event: &mut ProgressDeadline) -> Option<DeadlinePhase> {
        let restoration = match self {
            Self::Candidate { restoration, .. }
            | Self::Current { restoration, .. }
            | Self::Merged { restoration, .. } => *restoration,
            _ => None,
        };
        if restoration.is_some_and(|deadline| deadline.check(Some(S::monotonic_micros())).is_err())
        {
            return Some(DeadlinePhase::MaintenanceRestoration);
        }
        let phase = self.deadline_phase()?;
        let deadline = match self {
            Self::Current { pending, .. }
            | Self::TerminationCurrent { pending, .. }
            | Self::ControllerRxCurrent { pending, .. } => &mut pending.deadline,
            _ => event,
        };
        deadline
            .expired_after_final_poll(S::monotonic_micros())
            .then_some(phase)
    }

    pub(super) fn begin_unlink_budget(
        &self,
        previous: Option<DeadlinePhase>,
        progress: &mut ProgressDeadline,
    ) {
        if previous != Some(DeadlinePhase::PostUnlink)
            && self.deadline_phase() == Some(DeadlinePhase::PostUnlink)
        {
            *progress = ProgressDeadline::for_operation(S::monotonic_micros());
        }
    }

    pub(super) fn deadline_phase(&self) -> Option<DeadlinePhase> {
        match self {
            Self::Running(running) => match running.radio_wait() {
                Some(RunningWait::Scheduler(_)) => Some(DeadlinePhase::SchedulerCompletion),
                Some(RunningWait::PostUnlink(_)) => Some(DeadlinePhase::PostUnlink),
                Some(RunningWait::SchedulerStop) => {
                    unreachable!("an ordinary running owner cannot retain scheduler stop")
                }
                None => None,
            },
            Self::Stopping(_) => Some(DeadlinePhase::SchedulerStop),
            Self::Current { .. }
            | Self::TerminationCurrent { .. }
            | Self::ControllerRxCurrent { .. } => Some(DeadlinePhase::ControllerTime),
            _ => None,
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "a rejected no-alloc abort must retain the complete affine radio owner"
    )]
    pub(super) fn begin_completion_abort(self) -> Result<Self, Fault<'a, S, N>> {
        let Self::Running(running) = self else {
            return Err(Fault {
                cause: PeripheralConnectionActiveFaultCause::CompletionAbortInvariant,
                _owner: FaultOwner::Deadline(self),
            });
        };
        match running.begin_scheduler_stop() {
            Ok(running) => Ok(Self::Stopping(running)),
            Err(running) => Err(Fault {
                cause: PeripheralConnectionActiveFaultCause::CompletionAbortInvariant,
                _owner: FaultOwner::Deadline(Self::Running(running)),
            }),
        }
    }

    pub(super) fn expire_deadline(
        self,
        cause: PeripheralConnectionActiveFaultCause,
    ) -> Fault<'a, S, N> {
        Fault {
            cause,
            _owner: FaultOwner::Deadline(self),
        }
    }

    pub(super) fn wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        match self {
            Self::Stopped { .. } => Some(PeripheralConnectionActiveWait::ControllerTime),
            Self::Running(running) => running.radio_wait().map(|wait| match wait {
                RunningWait::Scheduler(wake) => PeripheralConnectionActiveWait::Scheduler(wake),
                RunningWait::PostUnlink(wake) => PeripheralConnectionActiveWait::PostUnlink(wake),
                RunningWait::SchedulerStop => {
                    unreachable!("ordinary running owner cannot expose scheduler stop")
                }
            }),
            Self::Stopping(_) => Some(PeripheralConnectionActiveWait::ControllerTime),
            Self::ControllerRxBackpressured { .. } => {
                Some(PeripheralConnectionActiveWait::HostEventCapacityOrControllerTime)
            }
            Self::ControllerRxCurrent {
                wait_for_recheck: true,
                ..
            } => Some(PeripheralConnectionActiveWait::ControllerTime),
            Self::ControllerRxCurrent {
                wait_for_recheck: false,
                ..
            } => None,
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

    pub(super) fn reset_wait(&self) -> Option<PeripheralConnectionActiveWait<'_>> {
        match self {
            Self::ControllerRxBackpressured { .. } => None,
            Self::ControllerRxCurrent { .. } => {
                Some(PeripheralConnectionActiveWait::ControllerTime)
            }
            _ => self.wait(),
        }
    }

    // Keep affine transition temporaries out of the enclosing lifecycle frame.
    #[inline(never)]
    pub(super) fn step(
        self,
        control: &mut oer_bluetooth_ll::control::LePeripheralControl,
        encryption: &mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
        acl: &mut super::acl::PeripheralConnectionAcl,
        deadlines: Deadlines<'_>,
        host_events: &mut super::host_events::PeripheralConnectionHostEvents,
        random: &mut impl super::super::PeripheralEncryptionRandomSource,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        let mut link = LinkState {
            control,
            encryption,
            acl,
            supervision: deadlines.supervision,
            termination: deadlines.termination,
            procedure: deadlines.procedure,
            host_events,
            random,
        };
        match self {
            stopped @ Self::Stopped { .. } => Step::Continue(stopped),
            Self::Running(running) => Self::poll_running(running, false),
            Self::Stopping(running) => Self::poll_running(running, true),
            Self::Completed(completed) => Self::complete(completed, &mut link),
            Self::ControllerRxBackpressured {
                task,
                completed,
                evidence,
            } => Self::begin_controller_rx_current(task, completed, evidence, true),
            Self::Candidate {
                mut task,
                candidate,
                evidence,
                restoration,
            } => match task.admit_peripheral_connection_recurring_candidate(candidate) {
                ControlFlow::Break(failure) => fault(
                    Cause::Preparation(failure.error()),
                    FaultOwner::Preparation(task, failure, evidence),
                ),
                ControlFlow::Continue(admitted) => {
                    Self::begin_current(task, admitted, evidence, false, restoration)
                }
            },
            Self::Current {
                pending,
                admitted,
                evidence,
                restoration,
                ..
            } => match pending.recheck() {
                Err(failure) => fault(
                    Cause::ControllerTime(failure.error()),
                    FaultOwner::Current(failure, admitted, evidence),
                ),
                Ok(ControlFlow::Break(pending)) => Step::Continue(Self::Current {
                    wait_for_recheck: true,
                    pending,
                    admitted,
                    evidence,
                    restoration,
                }),
                Ok(ControlFlow::Continue(now)) => {
                    Self::finish_current(now, admitted, evidence, &mut link, restoration)
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
                Ok(ControlFlow::Break(pending)) => Step::Continue(Self::TerminationCurrent {
                    wait_for_recheck: true,
                    pending,
                    completed,
                    evidence,
                }),
                Ok(ControlFlow::Continue(now)) => {
                    let reference = now.peripheral_current_instant();
                    let task = now.into_retained_epoch().into_task_service();
                    Self::complete_ready(task, completed, evidence, Some(reference), &mut link)
                }
            },
            Self::ControllerRxCurrent {
                pending,
                completed,
                evidence,
                ..
            } => match pending.recheck() {
                Err(failure) => fault(
                    Cause::ControllerTime(failure.error()),
                    FaultOwner::ControllerRxCurrent(failure, completed, evidence),
                ),
                Ok(ControlFlow::Break(pending)) => Step::Continue(Self::ControllerRxCurrent {
                    wait_for_recheck: true,
                    pending,
                    completed,
                    evidence,
                }),
                Ok(ControlFlow::Continue(now)) => {
                    Self::finish_controller_rx_current(now, completed, evidence, &mut link)
                }
            },
            Self::Merged {
                task,
                merged,
                progress_deadline,
                evidence,
                restoration,
            } => Self::publish(task, merged, evidence, progress_deadline, restoration),
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
        encryption: &mut oer_bluetooth_ll::security::LePeripheralEncryptionProcedure,
        acl: &mut super::acl::PeripheralConnectionAcl,
        deadlines: Deadlines<'_>,
        host_events: &mut super::host_events::PeripheralConnectionHostEvents,
        random: &mut impl super::super::PeripheralEncryptionRandomSource,
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
            Self::ControllerRxBackpressured {
                mut task,
                completed,
                evidence,
            } => match task.retire_peripheral_connection(completed) {
                ControlFlow::Continue(()) => ResetStep::Quiesced(task),
                ControlFlow::Break(completed) => ResetStep::Fault(Fault {
                    cause: PeripheralConnectionActiveFaultCause::RetirementIdentityMismatch,
                    _owner: FaultOwner::Control(task, completed, evidence),
                }),
            },
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
                Ok(ControlFlow::Break(pending)) => ResetStep::Continue(Self::TerminationCurrent {
                    wait_for_recheck: true,
                    pending,
                    completed,
                    evidence,
                }),
                Ok(ControlFlow::Continue(now)) => {
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
            Self::ControllerRxCurrent {
                pending,
                completed,
                evidence,
                ..
            } => match pending.recheck() {
                Err(failure) => ResetStep::Fault(Fault {
                    cause: PeripheralConnectionActiveFaultCause::ControllerTime(failure.error()),
                    _owner: FaultOwner::ControllerRxCurrent(failure, completed, evidence),
                }),
                Ok(ControlFlow::Break(pending)) => ResetStep::Continue(Self::ControllerRxCurrent {
                    wait_for_recheck: true,
                    pending,
                    completed,
                    evidence,
                }),
                Ok(ControlFlow::Continue(now)) => {
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
            radio => match radio.step(control, encryption, acl, deadlines, host_events, random) {
                Step::Continue(radio) => ResetStep::Continue(radio),
                Step::Published(radio, _) => ResetStep::Published(radio),
                Step::Fault(fault) => ResetStep::Fault(fault),
            },
        }
    }

    #[inline(never)]
    fn publish(
        mut task: Task<'a, S, N>,
        merged: sched::PeripheralConnectionRecurringEmptySchedulerMergePrepared,
        evidence: Evidence,
        progress_deadline: ProgressDeadline,
        restoration: Restoration,
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
                let running = Self::Running(Running::from_recurring(
                    task,
                    running,
                    event_counter,
                    evidence,
                    progress_deadline,
                ));
                let observation = if let Some(deadline) = restoration {
                    let now = S::monotonic_micros();
                    if deadline.check(Some(now)).is_err() {
                        return Step::Fault(
                            running.expire_deadline(Cause::MaintenanceRestorationExpired),
                        );
                    }
                    Some(super::super::maintenance::PeripheralMaintenanceRun {
                        admitted_at_micros: deadline.started_at_micros(),
                        run_at_micros: now,
                        restoration_deadline_micros: deadline.expires_at_micros(),
                        event_counter,
                    })
                } else {
                    None
                };
                Step::Published(running, observation)
            }
        }
    }

    #[inline(never)]
    fn poll_running(running: Running<'a, S, N>, stopping: bool) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;
        running.step_radio_with(
            stopping,
            Continuations::new(
                |stopping, running| {
                    Step::Continue(if stopping {
                        Self::Stopping(running)
                    } else {
                        Self::Running(running)
                    })
                },
                |stopping, running| {
                    Step::Continue(if stopping {
                        Self::Stopping(running)
                    } else {
                        Self::Running(running)
                    })
                },
                |_, running, observed| {
                    fault(
                        Cause::UnrelatedFinishedList,
                        FaultOwner::Unrelated(running, observed),
                    )
                },
                |_, owner| {
                    fault(
                        Cause::SchedulerEpochUnavailable,
                        FaultOwner::Normalization(owner),
                    )
                },
                |_, completed| Step::Continue(Self::Completed(completed)),
                |_, owner: CompletionFault<'a, S, N>| {
                    fault(
                        Cause::Completion(owner.cause()),
                        FaultOwner::Completion(owner),
                    )
                },
                |_, owner: RecycleFault<'a, S, N>| {
                    fault(Cause::Recycle(owner.cause()), FaultOwner::Recycle(owner))
                },
            ),
        )
    }

    // LL/control completion and candidate formation own separate large results.
    #[inline(never)]
    fn complete(completed: Completed<'a, S, N>, link: &mut LinkState<'_>) -> Step<'a, S, N> {
        if matches!(
            completed.connection().status(),
            oer_esp32s31_bluetooth_memory::PeripheralConnectionSchedulerItemCompletionStatus::Aborted
        ) {
            let (task, completed, evidence) = completed.into_parts();
            link.host_events.observe_radio_abort();
            return Self::retire(
                task,
                completed,
                evidence,
                0x1f,
                false,
                link.acl,
                link.host_events,
            );
        }
        if !link.acl.controller_event_is_reserved()
            || !link
                .acl
                .can_accept_controller_batch(completed.connection().required_controller_acl_slots())
        {
            let (task, completed, evidence) = completed.into_parts();
            return fault(
                PeripheralConnectionActiveFaultCause::ControllerRxReservationInvariant,
                FaultOwner::Control(task, completed, evidence),
            );
        }

        let (task, completed, evidence) = completed.into_parts();
        if (link.termination.is_none() && link.control.local_termination_queued())
            || link.encryption.blocks_unrelated_transmission()
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
        let control_result = task.process_peripheral_control(
            &mut completed,
            link.control,
            link.encryption,
            link.acl,
            link.random,
        );
        link.acl.complete_controller_event();
        if link.termination.is_none() {
            *link.termination =
                super::super::termination::PeripheralTerminationDeadline::after_graph_update(
                    termination_reference,
                    termination_timeout,
                    link.control.local_termination_queued(),
                    link.control.local_termination_reason(),
                );
        }
        update_procedure_deadline(
            link,
            termination_reference,
            matches!(control_result, Ok(true)),
        );
        if !link
            .host_events
            .observe_completion(completed.link_layer_completion())
        {
            link.control.request_local_termination(0x1f);
        }
        if link.encryption.termination_reason() == Some(0x3d) {
            link.host_events
                .observe_acl_completed(link.acl.take_completed_host_packets());
            return Self::retire(
                task,
                completed,
                evidence,
                0x3d,
                true,
                link.acl,
                link.host_events,
            );
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
        if link.control.local_termination_acknowledged()
            && let Some(reason) = link.control.local_termination_completion_reason()
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
        if !link.acl.reserve_controller_event() {
            return Step::Continue(Self::ControllerRxBackpressured {
                task,
                completed,
                evidence,
            });
        }
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
                    restoration: None,
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
        link: &mut LinkState<'_>,
        restoration: Restoration,
    ) -> Step<'a, S, N> {
        use PeripheralConnectionActiveFaultCause as Cause;

        use super::super::deadlines::Decision;
        match now.check_peripheral_deadlines(link.deadlines(), &admitted) {
            Decision::Expired(expired) => {
                let reason = link.expire_deadline(expired);
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
            Decision::Wait => {
                let task = now.into_retained_epoch().into_task_service();
                return Self::begin_current(task, admitted, evidence, true, restoration);
            }
            Decision::Run => {}
        }
        use super::super::recovery::PeripheralMissedAnchorDecision;
        match now.decide_peripheral_missed_anchor(&admitted, restoration.is_some()) {
            PeripheralMissedAnchorDecision::Run
            | PeripheralMissedAnchorDecision::EstablishmentPolicyRequired => {}
            PeripheralMissedAnchorDecision::Skip(delta) => {
                let mut task = now.into_retained_epoch().into_task_service();
                let (completed, _) = task
                    .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                    .into_parts();
                return Self::prepare_candidate(task, completed, delta, evidence, link);
            }
            PeripheralMissedAnchorDecision::MaintenanceWindowMissed => {
                let mut task = now.into_retained_epoch().into_task_service();
                let (completed, _) = task
                    .cancel_peripheral_connection_recurring_pre_sequence(admitted)
                    .into_parts();
                return fault(
                    Cause::MaintenanceRestorationExpired,
                    FaultOwner::Recovery(task, completed, evidence),
                );
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
        let progress_deadline = now.peripheral_progress_deadline(&admitted);
        match now.finish_peripheral_connection_recurring_event(admitted) {
            ctrl::PeripheralConnectionRecurringSequenceCompletion::Prepared { task, merged } => {
                Step::Continue(Self::Merged {
                    task,
                    merged,
                    progress_deadline,
                    evidence,
                    restoration,
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
    fn finish_controller_rx_current(
        now: ctrl::ControllerSchedulerNowReady<'a, S, N>,
        mut completed: sched::PeripheralConnectionSchedulerCompleted,
        evidence: Evidence,
        link: &mut LinkState<'_>,
    ) -> Step<'a, S, N> {
        let reference = now.peripheral_current_instant();
        let mut task = now.into_retained_epoch().into_task_service();

        if let super::super::deadlines::Decision::Expired(expired) =
            link.deadlines().decide(reference, reference)
        {
            let reason = link.expire_deadline(expired);
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

        let termination_timeout = completed
            .link_layer_completion()
            .timing()
            .supervision_timeout_micros();
        let packet_enqueued = task.enqueue_peripheral_transmission(
            &mut completed,
            link.control,
            link.encryption,
            link.acl,
        );
        if link.termination.is_none() {
            *link.termination =
                super::super::termination::PeripheralTerminationDeadline::after_graph_update(
                    Some(reference),
                    termination_timeout,
                    link.control.local_termination_queued(),
                    link.control.local_termination_reason(),
                );
        }
        update_procedure_deadline(link, Some(reference), packet_enqueued);

        if link.acl.reserve_controller_event() {
            let delta =
                oer_bluetooth_ll::connection::LePeripheralConnectionEventDelta::new(1).unwrap();
            Self::prepare_candidate(task, completed, delta, evidence, link)
        } else {
            Step::Continue(Self::ControllerRxBackpressured {
                task,
                completed,
                evidence,
            })
        }
    }

    fn begin_controller_rx_current(
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
                    FaultOwner::ControllerRxEpoch(unavailable, completed, evidence),
                );
            }
        };
        match retained.begin_fresh_scheduler_current() {
            Ok(pending) => Step::Continue(Self::ControllerRxCurrent {
                wait_for_recheck,
                pending: TimedCurrent::new(pending),
                completed,
                evidence,
            }),
            Err(failure) => fault(
                Cause::ControllerTimeBegin(failure.error()),
                FaultOwner::ControllerRxCurrentBegin(failure, completed, evidence),
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
                pending: TimedCurrent::new(pending),
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
        restoration: Restoration,
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
                pending: TimedCurrent::new(pending),
                admitted,
                evidence,
                restoration,
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
