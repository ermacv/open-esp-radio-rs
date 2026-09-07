//! Bounded first-window runner for restricted passive LE scanning.
//!
//! Portable LL state remains affine while the S31 Controller acquires live
//! time, prepares private SRAM, publishes the common scheduler head and starts
//! hardware. Every transition is finite and executor-neutral.

#![forbid(unsafe_code)]

use crate::{
    controller::{
        AlwaysAwakePostEnableTimeBeginFailure, AlwaysAwakePostEnableTimeFailure,
        AlwaysAwakePostEnableTimePending, AlwaysAwakePostEnableTimeStep,
        ControllerPublishedTaskService, ControllerSchedulerCurrentBeginFailure,
        ControllerSchedulerCurrentFailure, ControllerSchedulerCurrentPending,
        ControllerSchedulerCurrentStep, ControllerSchedulerNowReady,
        PassiveScanControllerPreparationError, PassiveScanControllerPreparationOutcome,
        PassiveScanControllerPreparationPending, PassiveScanControllerPreparationStep,
        SchedulerRunInterruptStorage,
        boot::{
            PassiveScanControllerInitialPreparationFailure,
            PassiveScanControllerPreparationFailStop,
        },
    },
    le::scanning::PassiveScanEventPhase,
    scheduler::{
        PassiveScanEmptySchedulerMergePrepared, PassiveScanSchedulerHeadPublished,
        SchedulerHeadPublicationError,
    },
};

use oer_bluetooth_ll::scanning::{LegacyPassiveScanWindowInFlight, LegacyPassiveScannerEnabled};

/// Portable in-flight window plus the optional prior S31 recurrence phase.
#[must_use = "retain the exact window request until it starts or is recovered"]
pub struct PassiveScanWindowRequest {
    window: LegacyPassiveScanWindowInFlight,
    previous_phase: Option<PassiveScanEventPhase>,
}

impl PassiveScanWindowRequest {
    fn first(scanner: LegacyPassiveScannerEnabled) -> Self {
        Self {
            window: scanner.begin_window(),
            previous_phase: None,
        }
    }

    fn recurring(
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: PassiveScanEventPhase,
    ) -> Self {
        Self {
            window: scanner.begin_window(),
            previous_phase: Some(previous_phase),
        }
    }

    pub fn cancel(self) -> (LegacyPassiveScannerEnabled, Option<PassiveScanEventPhase>) {
        (self.window.cancel(), self.previous_phase)
    }
}

#[must_use = "step or retain the exact first scanner runner"]
pub struct PassiveScanFirstRunner<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: PassiveScanFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>,
}

enum PassiveScanFirstRunnerPhase<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdCurrent {
        request: PassiveScanWindowRequest,
        pending: AlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        request: PassiveScanWindowRequest,
        pending: ControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        request: PassiveScanWindowRequest,
        current: ControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        request: PassiveScanWindowRequest,
        pending: PassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Prepared {
        window: LegacyPassiveScanWindowInFlight,
        phase: PassiveScanEventPhase,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: PassiveScanEmptySchedulerMergePrepared,
    },
    Head {
        window: LegacyPassiveScanWindowInFlight,
        phase: PassiveScanEventPhase,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        head: PassiveScanSchedulerHeadPublished,
    },
}

/// One finite first-window runner transition.
#[must_use = "retain a wait, continue, running owner, or exact failure"]
pub enum PassiveScanFirstRunnerStep<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    WaitControllerTime(PassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Continue(PassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>),
    Running(PassiveScanFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(PassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// First scanner window after the exact scheduler `RUN` publication.
#[must_use = "retain the running hardware and portable LL window owners"]
pub struct PassiveScanFirstRunning<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    window: LegacyPassiveScanWindowInFlight,
    phase: PassiveScanEventPhase,
    task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: crate::scheduler::core::SingleItemSchedulerRunning<
        crate::le::scanning::passive::active::PassiveScanCompletionRole,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyPassiveScanWindowInFlight,
        PassiveScanEventPhase,
        crate::scheduler::core::SingleItemSchedulerRunning<
            crate::le::scanning::passive::active::PassiveScanCompletionRole,
        >,
    ) {
        (self.task, self.window, self.phase, self.running)
    }
}

/// Retryable pre-`RUN` scanner hardware edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassiveScanFirstRunnerRetryCause<E> {
    HeadPublication(SchedulerHeadPublicationError),
    SchedulerStart(E),
}

#[must_use = "inspect and retry the exact retained pre-RUN scanner phase"]
pub struct PassiveScanFirstRunnerRetry<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: PassiveScanFirstRunnerRetryCause<S::Error>,
    runner: PassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
}

/// Scanner owner sealed after an RX-list publication could not join its graph.
///
/// The task service is retained with the proof-mismatch owner because the
/// first MMIO write already completed. No operation can relabel this state as
/// retryable or restore an idle Controller.
#[must_use = "retain the permanently faulted Controller and scanner owners"]
pub struct PassiveScanFirstRunnerPublicationFailStop<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    _window: LegacyPassiveScanWindowInFlight,
    _phase: PassiveScanEventPhase,
    _task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ownership: PassiveScanFirstRunnerPublicationFailStopOwnership,
}

enum PassiveScanFirstRunnerPublicationFailStopOwnership {
    RxPublication(crate::scheduler::PassiveScanSchedulerHeadPublicationFailure),
    RetryabilityInvariant {
        _merged: PassiveScanEmptySchedulerMergePrepared,
    },
}

impl<S, const SCHEDULER_CAPACITY: usize>
    PassiveScanFirstRunnerPublicationFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Exact RX proof mismatch observed after the irreversible publication.
    pub const fn error(
        &self,
    ) -> Option<oer_esp32s31_bluetooth_memory::PassiveScanMemoryGraphPublicationError> {
        match &self.ownership {
            PassiveScanFirstRunnerPublicationFailStopOwnership::RxPublication(failure) => {
                failure.rx_publication_error()
            }
            PassiveScanFirstRunnerPublicationFailStopOwnership::RetryabilityInvariant {
                ..
            } => None,
        }
    }
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &PassiveScanFirstRunnerRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> PassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner
    }
}

/// Exact failed first-window owner.
#[must_use = "recover the portable scanner and Controller or retry the retained phase"]
pub enum PassiveScanFirstRunnerFailure<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    ColdBegin {
        request: PassiveScanWindowRequest,
        failure: AlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        request: PassiveScanWindowRequest,
        failure: AlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        request: PassiveScanWindowRequest,
        failure: ControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        request: PassiveScanWindowRequest,
        failure: ControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Recovered {
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: Option<PassiveScanEventPhase>,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: PassiveScanControllerPreparationError,
    },
    PreparationFailStop {
        request: PassiveScanWindowRequest,
        failure: PassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    },
    PublicationFailStop(PassiveScanFirstRunnerPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>),
    Retryable(PassiveScanFirstRunnerRetry<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    PassiveScanFirstRunner<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    fn from_phase(phase: PassiveScanFirstRunnerPhase<'runtime, S, SCHEDULER_CAPACITY>) -> Self {
        Self { phase }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    pub fn begin(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        scanner: LegacyPassiveScannerEnabled,
    ) -> Result<Self, PassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        Self::begin_request(task, PassiveScanWindowRequest::first(scanner))
    }

    /// Begin the next interval-preserving scanner window.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    pub fn begin_recurring(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: PassiveScanEventPhase,
    ) -> Result<Self, PassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        Self::begin_request(
            task,
            PassiveScanWindowRequest::recurring(scanner, previous_phase),
        )
    }

    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    fn begin_request(
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        request: PassiveScanWindowRequest,
    ) -> Result<Self, PassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(PassiveScanFirstRunnerPhase::WarmCurrent {
                    request,
                    pending,
                })),
                Err(failure) => Err(PassiveScanFirstRunnerFailure::WarmBegin { request, failure }),
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(PassiveScanFirstRunnerPhase::ColdCurrent {
                    request,
                    pending,
                })),
                Err(failure) => Err(PassiveScanFirstRunnerFailure::ColdBegin { request, failure }),
            },
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> PassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            PassiveScanFirstRunnerPhase::ColdCurrent { request, pending } => {
                match pending.recheck() {
                    Ok(AlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        PassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            PassiveScanFirstRunnerPhase::ColdCurrent { request, pending },
                        ))
                    }
                    Ok(AlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        PassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            PassiveScanFirstRunnerPhase::CurrentReady {
                                request,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => PassiveScanFirstRunnerStep::Failed(
                        PassiveScanFirstRunnerFailure::ColdRecheck { request, failure },
                    ),
                }
            }
            PassiveScanFirstRunnerPhase::WarmCurrent { request, pending } => {
                match pending.recheck() {
                    Ok(ControllerSchedulerCurrentStep::Waiting(pending)) => {
                        PassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            PassiveScanFirstRunnerPhase::WarmCurrent { request, pending },
                        ))
                    }
                    Ok(ControllerSchedulerCurrentStep::Ready(current)) => {
                        PassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            PassiveScanFirstRunnerPhase::CurrentReady { request, current },
                        ))
                    }
                    Err(failure) => PassiveScanFirstRunnerStep::Failed(
                        PassiveScanFirstRunnerFailure::WarmRecheck { request, failure },
                    ),
                }
            }
            PassiveScanFirstRunnerPhase::CurrentReady { request, current } => {
                let parameters = request.window.parameters();
                let channel = request.window.channel();
                match current.begin_passive_scan_first_event(
                    parameters,
                    channel,
                    request.previous_phase,
                ) {
                    Ok(pending) => {
                        PassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            PassiveScanFirstRunnerPhase::Preparation { request, pending },
                        ))
                    }
                    Err(PassiveScanControllerInitialPreparationFailure::Rejected {
                        current,
                        error,
                    }) => Self::recovered_failure(
                        request,
                        current.into_retained_epoch().into_task_service(),
                        error,
                    ),
                    Err(PassiveScanControllerInitialPreparationFailure::FailStop(failure)) => {
                        PassiveScanFirstRunnerStep::Failed(
                            PassiveScanFirstRunnerFailure::PreparationFailStop { request, failure },
                        )
                    }
                }
            }
            PassiveScanFirstRunnerPhase::Preparation { request, pending } => {
                match pending.recheck() {
                    PassiveScanControllerPreparationStep::Pending(pending) => {
                        PassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            PassiveScanFirstRunnerPhase::Preparation { request, pending },
                        ))
                    }
                    PassiveScanControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(request, terminal)
                    }
                    PassiveScanControllerPreparationStep::FailStop(failure) => {
                        PassiveScanFirstRunnerStep::Failed(
                            PassiveScanFirstRunnerFailure::PreparationFailStop { request, failure },
                        )
                    }
                }
            }
            PassiveScanFirstRunnerPhase::Prepared {
                window,
                phase,
                mut task,
                merged,
            } => match task.publish_passive_scan_scheduler_head(merged) {
                Ok(head) => PassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    PassiveScanFirstRunnerPhase::Head {
                        window,
                        phase,
                        task,
                        head,
                    },
                )),
                Err(failure) => {
                    let head_error = failure.head_error();
                    match failure.into_retryable_merged() {
                        Ok(merged) => match head_error {
                            Some(error) => PassiveScanFirstRunnerStep::Failed(
                                PassiveScanFirstRunnerFailure::Retryable(
                                    PassiveScanFirstRunnerRetry {
                                        cause: PassiveScanFirstRunnerRetryCause::HeadPublication(
                                            error,
                                        ),
                                        runner: Self::from_phase(
                                            PassiveScanFirstRunnerPhase::Prepared {
                                                window,
                                                phase,
                                                task,
                                                merged,
                                            },
                                        ),
                                    },
                                ),
                            ),
                            None => PassiveScanFirstRunnerStep::Failed(
                                PassiveScanFirstRunnerFailure::PublicationFailStop(
                                    PassiveScanFirstRunnerPublicationFailStop {
                                        _window: window,
                                        _phase: phase,
                                        _task: task,
                                        ownership: PassiveScanFirstRunnerPublicationFailStopOwnership::RetryabilityInvariant {
                                            _merged: merged,
                                        },
                                    },
                                ),
                            ),
                        },
                        Err(failure) => PassiveScanFirstRunnerStep::Failed(
                            PassiveScanFirstRunnerFailure::PublicationFailStop(
                                PassiveScanFirstRunnerPublicationFailStop {
                                    _window: window,
                                    _phase: phase,
                                    _task: task,
                                    ownership: PassiveScanFirstRunnerPublicationFailStopOwnership::RxPublication(failure),
                                },
                            ),
                        ),
                    }
                }
            },
            PassiveScanFirstRunnerPhase::Head {
                window,
                phase,
                mut task,
                head,
            } => match task.start_passive_scan_scheduler(head) {
                Ok(running) => PassiveScanFirstRunnerStep::Running(PassiveScanFirstRunning {
                    window,
                    phase,
                    task,
                    running,
                }),
                Err(failure) => {
                    let (error, head) = failure.into_parts();
                    PassiveScanFirstRunnerStep::Failed(PassiveScanFirstRunnerFailure::Retryable(
                        PassiveScanFirstRunnerRetry {
                            cause: PassiveScanFirstRunnerRetryCause::SchedulerStart(error),
                            runner: Self::from_phase(PassiveScanFirstRunnerPhase::Head {
                                window,
                                phase,
                                task,
                                head,
                            }),
                        },
                    ))
                }
            },
        }
    }

    fn finish_preparation(
        request: PassiveScanWindowRequest,
        terminal: crate::controller::PassiveScanControllerPreparationTerminal<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> PassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        let task = epoch.into_task_service();
        match outcome {
            PassiveScanControllerPreparationOutcome::Prepared { merged, phase } => {
                PassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    PassiveScanFirstRunnerPhase::Prepared {
                        window: request.window,
                        phase,
                        task,
                        merged,
                    },
                ))
            }
            PassiveScanControllerPreparationOutcome::Rejected(error) => {
                Self::recovered_failure(request, task, error)
            }
        }
    }

    fn recovered_failure(
        request: PassiveScanWindowRequest,
        task: ControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: PassiveScanControllerPreparationError,
    ) -> PassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (scanner, previous_phase) = request.cancel();
        PassiveScanFirstRunnerStep::Failed(PassiveScanFirstRunnerFailure::Recovered {
            scanner,
            previous_phase,
            task,
            error,
        })
    }
}
