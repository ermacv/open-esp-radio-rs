//! Bounded first-event runner for restricted legacy LE advertising.
//!
//! The accepted HCI Enable remains affine through controller time, SRAM
//! preparation, scheduler-head publication and the final `RUN` command. Each
//! step is finite; hardware-owned controller-time requests return to the
//! executor instead of being polled internally.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_hci::{
    LeControllerDeferredLegacyNonconnectableAdvertisingStart, LeControllerResponsePending,
};

use crate::controller::boot::{
    BluetoothLegacyAdvertisingControllerInitialPreparationFailure,
    BluetoothLegacyAdvertisingControllerPreparationFailStop,
};
use crate::{
    BluetoothAlwaysAwakePostEnableTimeBeginFailure, BluetoothAlwaysAwakePostEnableTimeFailure,
    BluetoothAlwaysAwakePostEnableTimePending, BluetoothAlwaysAwakePostEnableTimeStep,
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginFailure,
    BluetoothControllerSchedulerCurrentFailure, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothControllerSchedulerNowReady,
    BluetoothLegacyAdvertisingControllerPreparationError,
    BluetoothLegacyAdvertisingControllerPreparationOutcome,
    BluetoothLegacyAdvertisingControllerPreparationPending,
    BluetoothLegacyAdvertisingControllerPreparationStep,
    BluetoothLegacyAdvertisingEmptySchedulerMergePrepared,
    BluetoothLegacyAdvertisingSchedulerHeadPublished, BluetoothSchedulerHeadPublicationError,
    BluetoothSchedulerRunInterruptStorage, prepare_legacy_advertising_set,
};

#[must_use = "retain the accepted Enable until hardware starts or idle ownership is recovered"]
pub struct BluetoothLegacyAdvertisingDeferredStart<'runtime> {
    command: LeControllerDeferredLegacyNonconnectableAdvertisingStart<'runtime, ()>,
}

impl<'runtime> BluetoothLegacyAdvertisingDeferredStart<'runtime> {
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
pub struct BluetoothLegacyAdvertisingFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothLegacyAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothLegacyAdvertisingFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdCurrent {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        pending:
            BluetoothLegacyAdvertisingControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Prepared {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>,
    },
    Head {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothLegacyAdvertisingSchedulerHeadPublished<'static>,
    },
}

/// One finite runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum BluetoothLegacyAdvertisingFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(BluetoothLegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First advertising graph after hardware scheduler `RUN`.
#[must_use = "publish the accepted Enable response and retain the running radio owner"]
pub struct BluetoothLegacyAdvertisingFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: crate::scheduler::core::BluetoothSingleItemSchedulerRunning<
        crate::legacy_advertising_completion::BluetoothLegacyAdvertisingCompletionRole<'static>,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn into_response_pending_session(
        self,
    ) -> crate::BluetoothLegacyAdvertisingResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>
    {
        crate::BluetoothLegacyAdvertisingResponsePendingSession::new(
            crate::BluetoothControllerIdleResponsePending::new(
                self.command.into_started_response(self.task),
            ),
            self.running,
        )
    }
}

/// Retryable pre-`RUN` hardware edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingFirstRunnerRetryCause<E> {
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    SchedulerStart(E),
}

#[must_use = "inspect and retry the exact retained pre-RUN phase"]
pub struct BluetoothLegacyAdvertisingFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingFirstRunnerRetryCause<S::Error>,
    runner: BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &BluetoothLegacyAdvertisingFirstRunnerRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner
    }
}

/// Exact failed first-event owner.
#[must_use = "recover idle response ownership or retry the retained pre-RUN phase"]
pub enum BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdBegin {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Recovered {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothLegacyAdvertisingControllerPreparationError,
    },
    PreparationFailStop {
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        failure: BluetoothLegacyAdvertisingControllerPreparationFailStop<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    },
    Retryable(BluetoothLegacyAdvertisingFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Convert only a failure which has recovered idle ownership into HCI status.
    pub fn into_hardware_failure_response(
        self,
    ) -> Result<crate::BluetoothControllerIdleResponsePending<'runtime, S, SCHEDULER_CAPACITY>, Self>
    {
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
        Ok(crate::BluetoothControllerIdleResponsePending::new(
            command.into_hardware_failure_response(task),
        ))
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_phase(
        phase: BluetoothLegacyAdvertisingFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact command and Controller owner"
    )]
    pub(crate) fn begin(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        command: LeControllerDeferredLegacyNonconnectableAdvertisingStart<'runtime, ()>,
    ) -> Result<Self, BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>>
    {
        let command = BluetoothLegacyAdvertisingDeferredStart::new(command);
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothLegacyAdvertisingFirstRunnerPhase::WarmCurrent { command, pending },
                )),
                Err(failure) => Err(BluetoothLegacyAdvertisingFirstRunnerFailure::WarmBegin {
                    command,
                    failure,
                }),
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothLegacyAdvertisingFirstRunnerPhase::ColdCurrent { command, pending },
                )),
                Err(failure) => Err(BluetoothLegacyAdvertisingFirstRunnerFailure::ColdBegin {
                    command,
                    failure,
                }),
            },
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(
        self,
    ) -> BluetoothLegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothLegacyAdvertisingFirstRunnerPhase::ColdCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                BluetoothLegacyAdvertisingFirstRunnerPhase::ColdCurrent {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    }
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothLegacyAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyAdvertisingFirstRunnerFailure::ColdRecheck {
                            command,
                            failure,
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingFirstRunnerPhase::WarmCurrent { command, pending } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                BluetoothLegacyAdvertisingFirstRunnerPhase::WarmCurrent {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothLegacyAdvertisingFirstRunnerPhase::CurrentReady {
                                command,
                                current,
                            },
                        ))
                    }
                    Err(failure) => BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyAdvertisingFirstRunnerFailure::WarmRecheck {
                            command,
                            failure,
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingFirstRunnerPhase::CurrentReady { command, current } => {
                let set = match prepare_legacy_advertising_set(command.command.request()) {
                    Ok(set) => set,
                    Err(error) => {
                        return Self::recovered_failure(
                            command,
                            current.into_retained_epoch().into_task_service(),
                            BluetoothLegacyAdvertisingControllerPreparationError::Set(error),
                        );
                    }
                };
                match current.begin_legacy_advertising_first_event(set) {
                    Ok(pending) => BluetoothLegacyAdvertisingFirstRunnerStep::WaitControllerTime(
                        Self::from_phase(BluetoothLegacyAdvertisingFirstRunnerPhase::Preparation {
                            command,
                            pending,
                        }),
                    ),
                    Err(
                        BluetoothLegacyAdvertisingControllerInitialPreparationFailure::Rejected {
                            current,
                            error,
                        },
                    ) => Self::recovered_failure(
                        command,
                        current.into_retained_epoch().into_task_service(),
                        error,
                    ),
                    Err(
                        BluetoothLegacyAdvertisingControllerInitialPreparationFailure::FailStop(
                            failure,
                        ),
                    ) => BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyAdvertisingFirstRunnerFailure::PreparationFailStop {
                            command,
                            failure,
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingFirstRunnerPhase::Preparation { command, pending } => {
                match pending.recheck() {
                    BluetoothLegacyAdvertisingControllerPreparationStep::Pending(pending) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::WaitControllerTime(
                            Self::from_phase(
                                BluetoothLegacyAdvertisingFirstRunnerPhase::Preparation {
                                    command,
                                    pending,
                                },
                            ),
                        )
                    }
                    BluetoothLegacyAdvertisingControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(command, terminal)
                    }
                    BluetoothLegacyAdvertisingControllerPreparationStep::FailStop(failure) => {
                        BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                            BluetoothLegacyAdvertisingFirstRunnerFailure::PreparationFailStop {
                                command,
                                failure,
                            },
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingFirstRunnerPhase::Prepared {
                command,
                mut task,
                merged,
            } => {
                match task.publish_legacy_advertising_scheduler_head(merged) {
                    Ok(head) => BluetoothLegacyAdvertisingFirstRunnerStep::Continue(
                        Self::from_phase(BluetoothLegacyAdvertisingFirstRunnerPhase::Head {
                            command,
                            task,
                            head,
                        }),
                    ),
                    Err(failure) => {
                        let error = failure.error();
                        BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyAdvertisingFirstRunnerFailure::Retryable(
                            BluetoothLegacyAdvertisingFirstRunnerRetry {
                                cause: BluetoothLegacyAdvertisingFirstRunnerRetryCause::HeadPublication(error),
                                runner: Self::from_phase(
                                    BluetoothLegacyAdvertisingFirstRunnerPhase::Prepared {
                                        command,
                                        task,
                                        merged: failure.into_merged(),
                                    },
                                ),
                            },
                        ),
                    )
                    }
                }
            }
            BluetoothLegacyAdvertisingFirstRunnerPhase::Head {
                command,
                mut task,
                head,
            } => {
                match task.start_legacy_advertising_scheduler(head) {
                    Ok(running) => BluetoothLegacyAdvertisingFirstRunnerStep::Running(
                        BluetoothLegacyAdvertisingFirstRunning {
                            command,
                            task,
                            running,
                        },
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
                        BluetoothLegacyAdvertisingFirstRunnerFailure::Retryable(
                            BluetoothLegacyAdvertisingFirstRunnerRetry {
                                cause: BluetoothLegacyAdvertisingFirstRunnerRetryCause::SchedulerStart(error),
                                runner: Self::from_phase(
                                    BluetoothLegacyAdvertisingFirstRunnerPhase::Head {
                                        command,
                                        task,
                                        head,
                                    },
                                ),
                            },
                        ),
                    )
                    }
                }
            }
        }
    }

    fn finish_preparation(
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        terminal: crate::BluetoothLegacyAdvertisingControllerPreparationTerminal<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> BluetoothLegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        let task = epoch.into_task_service();
        match outcome {
            BluetoothLegacyAdvertisingControllerPreparationOutcome::Prepared(merged) => {
                BluetoothLegacyAdvertisingFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothLegacyAdvertisingFirstRunnerPhase::Prepared {
                        command,
                        task,
                        merged,
                    },
                ))
            }
            BluetoothLegacyAdvertisingControllerPreparationOutcome::Rejected(error) => {
                Self::recovered_failure(command, task, error)
            }
        }
    }

    fn recovered_failure(
        command: BluetoothLegacyAdvertisingDeferredStart<'runtime>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothLegacyAdvertisingControllerPreparationError,
    ) -> BluetoothLegacyAdvertisingFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothLegacyAdvertisingFirstRunnerStep::Failed(
            BluetoothLegacyAdvertisingFirstRunnerFailure::Recovered {
                command,
                task,
                error,
            },
        )
    }
}
