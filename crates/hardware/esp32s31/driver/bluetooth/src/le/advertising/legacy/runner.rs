//! Bounded first-event runner for restricted legacy LE advertising.
//!
//! The accepted HCI Enable remains affine through controller time, SRAM
//! preparation, scheduler-head publication and the final `RUN` command. Each
//! step is finite; hardware-owned controller-time requests return to the
//! executor instead of being polled internally.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        AlwaysAwakePostEnableTimeBeginFailure, AlwaysAwakePostEnableTimeFailure,
        AlwaysAwakePostEnableTimePending, AlwaysAwakePostEnableTimeStep,
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginFailure,
        ControllerSchedulerCurrentFailure, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, ControllerSchedulerNowReady,
        LegacyAdvertisingControllerPreparationError, LegacyAdvertisingControllerPreparationOutcome,
        LegacyAdvertisingControllerPreparationPending, LegacyAdvertisingControllerPreparationStep,
        SchedulerRunInterruptStorage,
        boot::{
            LegacyAdvertisingControllerInitialPreparationFailure,
            LegacyAdvertisingControllerPreparationFailStop,
        },
    },
    le::advertising::prepare_legacy_advertising_set,
    scheduler::{
        LegacyAdvertisingEmptySchedulerMergePrepared, LegacyAdvertisingSchedulerHeadPublished,
        SchedulerHeadPublicationError,
    },
};

use oer_bluetooth_hci::{
    LeControllerDeferredLegacyNonconnectableAdvertisingStart, LeControllerResponsePending,
};

#[must_use = "retain the accepted Enable until hardware starts or idle ownership is recovered"]
pub struct LegacyAdvertisingDeferredStart<'runtime> {
    command: LeControllerDeferredLegacyNonconnectableAdvertisingStart<'runtime, ()>,
}

impl<'runtime> LegacyAdvertisingDeferredStart<'runtime> {
    pub(crate) const fn new(
        command: LeControllerDeferredLegacyNonconnectableAdvertisingStart<'runtime, ()>,
    ) -> Self {
        Self { command }
    }

    fn into_started_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        self.command.map_owner(|()| owner).into_started_response()
    }

    fn into_hardware_failure_response<Owner>(
        self,
        owner: Owner,
    ) -> LeControllerResponsePending<'runtime, Owner> {
        self.command
            .map_owner(|()| owner)
            .into_hardware_failure_response()
    }
}

#[must_use = "step or retain the exact first advertising runner"]
pub struct LegacyAdvertisingFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: LegacyAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum LegacyAdvertisingFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        pending: AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        pending: ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        pending: LegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Prepared {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: LegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
    Head {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: LegacyAdvertisingSchedulerHeadPublished<'static>,
    },
}

/// One finite runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum LegacyAdvertisingFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(LegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(LegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First advertising graph after hardware scheduler `RUN`.
#[must_use = "publish the accepted Enable response and retain the running radio owner"]
pub struct LegacyAdvertisingFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    command: LegacyAdvertisingDeferredStart<'runtime>,
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: crate::scheduler::core::SingleItemSchedulerRunning<
        crate::le::advertising::legacy::completion::LegacyAdvertisingCompletionRole<'static>,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub fn into_response_pending_session(
        self,
    ) -> crate::le::advertising::LegacyAdvertisingResponsePendingSession<
        'runtime,
        S,
        SCHEDULER_CAPACITY,
    > {
        crate::le::advertising::LegacyAdvertisingResponsePendingSession::new(
            crate::controller::ControllerIdleResponsePending::new(
                self.command.into_started_response(self.task),
            ),
            self.running,
        )
    }
}

/// Retryable pre-`RUN` hardware edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingFirstRunnerRetryCause<E> {
    HeadPublication(SchedulerHeadPublicationError),
    SchedulerStart(E),
}

#[must_use = "inspect and retry the exact retained pre-RUN phase"]
pub struct LegacyAdvertisingFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: LegacyAdvertisingFirstRunnerRetryCause<S::Error>,
    runner: LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &LegacyAdvertisingFirstRunnerRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner
    }
}

/// Exact failed first-event owner.
#[must_use = "recover idle response ownership or retry the retained pre-RUN phase"]
pub enum LegacyAdvertisingFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdBegin {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        failure: AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        failure: AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        failure: ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        failure: ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Recovered {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: LegacyAdvertisingControllerPreparationError,
    },
    PreparationFailStop {
        command: LegacyAdvertisingDeferredStart<'runtime>,
        failure: LegacyAdvertisingControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Retryable(LegacyAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Convert only a failure which has recovered idle ownership into HCI status.
    #[expect(
        clippy::result_large_err,
        reason = "the recoverable failure retains the exact affine radio state and continuation owners without allocation"
    )]
    pub fn into_hardware_failure_response(
        self,
    ) -> Result<
        crate::controller::ControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>,
        Self,
    > {
        let (command, task) = match self {
            Self::ColdBegin { command, failure } => (command, failure.into_parts().0),
            Self::ColdRecheck { command, failure } => (command, failure.into_parts().0),
            Self::WarmBegin { command, failure } => {
                (command, failure.into_parts().0.into_task_service())
            }
            Self::WarmRecheck { command, failure } => {
                (command, failure.into_parts().0.into_task_service())
            }
            Self::Recovered { command, task, .. } => (command, task),
            retained @ (Self::PreparationFailStop { .. } | Self::Retryable(_)) => {
                return Err(retained);
            }
        };
        Ok(crate::controller::ControllerIdleResponsePending::new(
            command.into_hardware_failure_response(task),
        ))
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_phase(
        phase: LegacyAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact command and Controller owner"
    )]
    pub(crate) fn begin(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: LeControllerDeferredLegacyNonconnectableAdvertisingStart<'runtime, ()>,
    ) -> Result<Self, LegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        let command = LegacyAdvertisingDeferredStart::new(command);
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    LegacyAdvertisingFirstRunnerPhase::WarmCurrent { command, pending },
                )),
                Err(failure) => {
                    Err(LegacyAdvertisingFirstRunnerFailure::WarmBegin { command, failure })
                }
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(
                    LegacyAdvertisingFirstRunnerPhase::ColdCurrent { command, pending },
                )),
                Err(failure) => {
                    Err(LegacyAdvertisingFirstRunnerFailure::ColdBegin { command, failure })
                }
            },
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> LegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            LegacyAdvertisingFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(AlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        LegacyAdvertisingFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::ColdCurrent { command, pending },
                        ))
                    }
                    Ok(AlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        LegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => LegacyAdvertisingFirstRunnerStep::Failed(
                        LegacyAdvertisingFirstRunnerFailure::ColdRecheck { command, failure },
                    ),
                }
            }
            LegacyAdvertisingFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                        LegacyAdvertisingFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::WarmCurrent { command, pending },
                        ))
                    }
                    Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                        LegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::CurrentReady { command, current },
                        ))
                    }
                    Err(failure) => LegacyAdvertisingFirstRunnerStep::Failed(
                        LegacyAdvertisingFirstRunnerFailure::WarmRecheck { command, failure },
                    ),
                }
            }
            LegacyAdvertisingFirstRunnerPhase::CurrentReady { command, current } => {
                let set = match prepare_legacy_advertising_set(command.command.request()) {
                    Ok(set) => set,
                    Err(error) => {
                        return Self::recovered_failure(
                            command,
                            current.into_retained_epoch().into_task_service(),
                            LegacyAdvertisingControllerPreparationError::Set(error),
                        );
                    }
                };
                match current.begin_legacy_advertising_first_event(set) {
                    Ok(pending) => {
                        LegacyAdvertisingFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    Err(LegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                        current,
                        error,
                    }) => Self::recovered_failure(
                        command,
                        current.into_retained_epoch().into_task_service(),
                        error,
                    ),
                    Err(LegacyAdvertisingControllerInitialPreparationFailure::FailStop(
                        failure,
                    )) => LegacyAdvertisingFirstRunnerStep::Failed(
                        LegacyAdvertisingFirstRunnerFailure::PreparationFailStop {
                            command,
                            failure,
                        },
                    ),
                }
            }
            LegacyAdvertisingFirstRunnerPhase::Preparation { command, pending } => {
                match pending.recheck() {
                    LegacyAdvertisingControllerPreparationStep::Pending(pending) => {
                        LegacyAdvertisingFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            LegacyAdvertisingFirstRunnerPhase::Preparation { command, pending },
                        ))
                    }
                    LegacyAdvertisingControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(command, terminal)
                    }
                    LegacyAdvertisingControllerPreparationStep::FailStop(failure) => {
                        LegacyAdvertisingFirstRunnerStep::Failed(
                            LegacyAdvertisingFirstRunnerFailure::PreparationFailStop {
                                command,
                                failure,
                            },
                        )
                    }
                }
            }
            LegacyAdvertisingFirstRunnerPhase::Prepared {
                command,
                mut task,
                merged,
            } => match task.publish_legacy_advertising_scheduler_head(merged) {
                Ok(head) => LegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                    LegacyAdvertisingFirstRunnerPhase::Head {
                        command,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let error = failure.error();
                    LegacyAdvertisingFirstRunnerStep::Failed(
                        LegacyAdvertisingFirstRunnerFailure::Retryable(
                            LegacyAdvertisingFirstRunnerRetry {
                                cause: LegacyAdvertisingFirstRunnerRetryCause::HeadPublication(
                                    error,
                                ),
                                runner: Self::from_phase(
                                    LegacyAdvertisingFirstRunnerPhase::Prepared {
                                        command,
                                        task,
                                        merged: failure.into_merged(),
                                    },
                                ),
                            },
                        ),
                    )
                }
            },
            LegacyAdvertisingFirstRunnerPhase::Head {
                command,
                mut task,
                head,
            } => match task.start_legacy_advertising_scheduler(head) {
                Ok(running) => {
                    LegacyAdvertisingFirstRunnerStep::Running(LegacyAdvertisingFirstRunning {
                        command,
                        task,
                        running,
                    })
                }
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    LegacyAdvertisingFirstRunnerStep::Failed(
                        LegacyAdvertisingFirstRunnerFailure::Retryable(
                            LegacyAdvertisingFirstRunnerRetry {
                                cause: LegacyAdvertisingFirstRunnerRetryCause::SchedulerStart(
                                    error,
                                ),
                                runner: Self::from_phase(LegacyAdvertisingFirstRunnerPhase::Head {
                                    command,
                                    task,
                                    head,
                                }),
                            },
                        ),
                    )
                }
            },
        }
    }

    fn finish_preparation(
        command: LegacyAdvertisingDeferredStart<'runtime>,
        terminal: crate::controller::LegacyAdvertisingControllerPreparationTerminal<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> LegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        let task = epoch.into_task_service();
        match outcome {
            LegacyAdvertisingControllerPreparationOutcome::Prepared(merged) => {
                LegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                    LegacyAdvertisingFirstRunnerPhase::Prepared {
                        command,
                        task,
                        merged,
                    },
                ))
            }
            LegacyAdvertisingControllerPreparationOutcome::Rejected(error) => {
                Self::recovered_failure(command, task, error)
            }
        }
    }

    fn recovered_failure(
        command: LegacyAdvertisingDeferredStart<'runtime>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: LegacyAdvertisingControllerPreparationError,
    ) -> LegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        LegacyAdvertisingFirstRunnerStep::Failed(LegacyAdvertisingFirstRunnerFailure::Recovered {
            command,
            task,
            error,
        })
    }
}
