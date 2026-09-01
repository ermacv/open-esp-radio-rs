//! Bounded first-window runner for restricted passive LE scanning.
//!
//! Portable LL state remains affine while the S31 Controller acquires live
//! time, prepares private SRAM, publishes the common scheduler head and starts
//! hardware. Every transition is finite and executor-neutral.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::{
    LegacyPassiveScanWindowInFlight, LegacyPassiveScannerEnabled,
};

use crate::controller_start::BluetoothPassiveScanControllerInitialPreparationFailure;
use crate::{
    BluetoothAlwaysAwakePostEnableTimeBeginFailure, BluetoothAlwaysAwakePostEnableTimeFailure,
    BluetoothAlwaysAwakePostEnableTimePending, BluetoothAlwaysAwakePostEnableTimeStep,
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginFailure,
    BluetoothControllerSchedulerCurrentFailure, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothControllerSchedulerNowReady,
    BluetoothPassiveScanControllerPreparationError,
    BluetoothPassiveScanControllerPreparationOutcome,
    BluetoothPassiveScanControllerPreparationPending,
    BluetoothPassiveScanControllerPreparationStep, BluetoothPassiveScanEmptySchedulerMergePrepared,
    BluetoothPassiveScanSchedulerHeadPublished, BluetoothPassiveScanSchedulerRunning,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerRunInterruptStorage,
};

#[must_use = "step or retain the exact first scanner runner"]
pub struct BluetoothPassiveScanFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothPassiveScanFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum BluetoothPassiveScanFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdCurrent {
        window: LegacyPassiveScanWindowInFlight,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        window: LegacyPassiveScanWindowInFlight,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        window: LegacyPassiveScanWindowInFlight,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        window: LegacyPassiveScanWindowInFlight,
        pending: BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Prepared {
        window: LegacyPassiveScanWindowInFlight,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
    },
    Head {
        window: LegacyPassiveScanWindowInFlight,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: BluetoothPassiveScanSchedulerHeadPublished,
    },
}

/// One finite first-window runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum BluetoothPassiveScanFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    WaitControllerTime(BluetoothPassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(BluetoothPassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(BluetoothPassiveScanFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(BluetoothPassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First scanner window after the exact scheduler `RUN` publication.
#[must_use = "retain the running hardware and portable LL window owners"]
pub struct BluetoothPassiveScanFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    window: LegacyPassiveScanWindowInFlight,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: BluetoothPassiveScanSchedulerRunning,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyPassiveScanWindowInFlight,
        BluetoothPassiveScanSchedulerRunning,
    ) {
        (self.task, self.window, self.running)
    }
}

/// Retryable pre-`RUN` scanner hardware edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPassiveScanFirstRunnerRetryCause<E> {
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    SchedulerStart(E),
}

#[must_use = "inspect and retry the exact retained pre-RUN scanner phase"]
pub struct BluetoothPassiveScanFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothPassiveScanFirstRunnerRetryCause<S::Error>,
    runner: BluetoothPassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &BluetoothPassiveScanFirstRunnerRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> BluetoothPassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner
    }
}

/// Exact failed first-window owner.
#[must_use = "recover the portable scanner and Controller or retry the retained phase"]
pub enum BluetoothPassiveScanFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    ColdBegin {
        window: LegacyPassiveScanWindowInFlight,
        failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        window: LegacyPassiveScanWindowInFlight,
        failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        window: LegacyPassiveScanWindowInFlight,
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        window: LegacyPassiveScanWindowInFlight,
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Recovered {
        scanner: LegacyPassiveScannerEnabled,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPassiveScanControllerPreparationError,
    },
    Retryable(BluetoothPassiveScanFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_phase(
        phase: BluetoothPassiveScanFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
    ) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    pub fn begin(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        scanner: LegacyPassiveScannerEnabled,
    ) -> Result<Self, BluetoothPassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        let window = scanner.begin_window();
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { window, pending },
                )),
                Err(failure) => {
                    Err(BluetoothPassiveScanFirstRunnerFailure::WarmBegin { window, failure })
                }
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { window, pending },
                )),
                Err(failure) => {
                    Err(BluetoothPassiveScanFirstRunnerFailure::ColdBegin { window, failure })
                }
            },
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { window, pending } => {
                match pending.recheck() {
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { window, pending },
                        ))
                    }
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::CurrentReady {
                                window,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::ColdRecheck { window, failure },
                    ),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { window, pending } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { window, pending },
                        ))
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::CurrentReady { window, current },
                        ))
                    }
                    Err(failure) => BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::WarmRecheck { window, failure },
                    ),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::CurrentReady { window, current } => {
                let scan_window = window.parameters().window();
                let channel = window.channel();
                match current.begin_passive_scan_first_event(scan_window, channel) {
                    Ok(pending) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::Preparation { window, pending },
                        ))
                    }
                    Err(BluetoothPassiveScanControllerInitialPreparationFailure::Rejected {
                        current,
                        error,
                    }) => Self::recovered_failure(
                        window,
                        current.into_retained_epoch().into_task_service(),
                        error,
                    ),
                    Err(BluetoothPassiveScanControllerInitialPreparationFailure::Terminal(
                        terminal,
                    )) => Self::finish_preparation(window, terminal),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::Preparation { window, pending } => {
                match pending.recheck() {
                    BluetoothPassiveScanControllerPreparationStep::Pending(pending) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::Preparation { window, pending },
                        ))
                    }
                    BluetoothPassiveScanControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(window, terminal)
                    }
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::Prepared {
                window,
                mut task,
                merged,
            } => match task.publish_passive_scan_scheduler_head(merged) {
                Ok(head) => BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::Head { window, task, head },
                )),
                Err(failure) => {
                    let cause =
                        BluetoothPassiveScanFirstRunnerRetryCause::HeadPublication(failure.error());
                    BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::Retryable(
                            BluetoothPassiveScanFirstRunnerRetry {
                                cause,
                                runner: Self::from_phase(
                                    BluetoothPassiveScanFirstRunnerPhase::Prepared {
                                        window,
                                        task,
                                        merged: failure.into_merged(),
                                    },
                                ),
                            },
                        ),
                    )
                }
            },
            BluetoothPassiveScanFirstRunnerPhase::Head {
                window,
                mut task,
                head,
            } => match task.start_passive_scan_scheduler(head) {
                Ok(running) => {
                    BluetoothPassiveScanFirstRunnerStep::Running(BluetoothPassiveScanFirstRunning {
                        window,
                        task,
                        running,
                    })
                }
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::Retryable(
                            BluetoothPassiveScanFirstRunnerRetry {
                                cause: BluetoothPassiveScanFirstRunnerRetryCause::SchedulerStart(
                                    error,
                                ),
                                runner: Self::from_phase(
                                    BluetoothPassiveScanFirstRunnerPhase::Head {
                                        window,
                                        task,
                                        head,
                                    },
                                ),
                            },
                        ),
                    )
                }
            },
        }
    }

    fn finish_preparation(
        window: LegacyPassiveScanWindowInFlight,
        terminal: crate::BluetoothPassiveScanControllerPreparationTerminal<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        let task = epoch.into_task_service();
        match outcome {
            BluetoothPassiveScanControllerPreparationOutcome::Prepared(merged) => {
                BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::Prepared {
                        window,
                        task,
                        merged,
                    },
                ))
            }
            BluetoothPassiveScanControllerPreparationOutcome::Rejected(error) => {
                Self::recovered_failure(window, task, error)
            }
        }
    }

    fn recovered_failure(
        window: LegacyPassiveScanWindowInFlight,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPassiveScanControllerPreparationError,
    ) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        BluetoothPassiveScanFirstRunnerStep::Failed(
            BluetoothPassiveScanFirstRunnerFailure::Recovered {
                scanner: window.cancel(),
                task,
                error,
            },
        )
    }
}
