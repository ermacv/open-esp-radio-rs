//! Bounded first-window runner for restricted passive LE scanning.
//!
//! Portable LL state remains affine while the S31 Controller acquires live
//! time, prepares private SRAM, publishes the common scheduler head and starts
//! hardware. Every transition is finite and executor-neutral.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::scanning::{
    LegacyPassiveScanWindowInFlight, LegacyPassiveScannerEnabled,
};

use crate::controller::boot::{
    BluetoothPassiveScanControllerInitialPreparationFailure,
    BluetoothPassiveScanControllerPreparationFailStop,
};
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
    BluetoothPassiveScanEventPhase, BluetoothPassiveScanSchedulerHeadPublished,
    BluetoothSchedulerHeadPublicationError, BluetoothSchedulerRunInterruptStorage,
};

/// Portable in-flight window plus the optional prior S31 recurrence phase.
#[must_use = "retain the exact window request until it starts or is recovered"]
pub struct BluetoothPassiveScanWindowRequest {
    window: LegacyPassiveScanWindowInFlight,
    previous_phase: Option<BluetoothPassiveScanEventPhase>,
}

impl BluetoothPassiveScanWindowRequest {
    fn first(scanner: LegacyPassiveScannerEnabled) -> Self {
        Self {
            window: scanner.begin_window(),
            previous_phase: None,
        }
    }

    fn recurring(
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: BluetoothPassiveScanEventPhase,
    ) -> Self {
        Self {
            window: scanner.begin_window(),
            previous_phase: Some(previous_phase),
        }
    }

    pub fn cancel(
        self,
    ) -> (
        LegacyPassiveScannerEnabled,
        Option<BluetoothPassiveScanEventPhase>,
    ) {
        (self.window.cancel(), self.previous_phase)
    }
}

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
        request: BluetoothPassiveScanWindowRequest,
        pending: BluetoothAlwaysAwakePostEnableTimePending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmCurrent {
        request: BluetoothPassiveScanWindowRequest,
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    CurrentReady {
        request: BluetoothPassiveScanWindowRequest,
        current: BluetoothControllerSchedulerNowReady<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Preparation {
        request: BluetoothPassiveScanWindowRequest,
        pending: BluetoothPassiveScanControllerPreparationPending<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Prepared {
        window: LegacyPassiveScanWindowInFlight,
        phase: BluetoothPassiveScanEventPhase,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
    },
    Head {
        window: LegacyPassiveScanWindowInFlight,
        phase: BluetoothPassiveScanEventPhase,
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
    phase: BluetoothPassiveScanEventPhase,
    task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    running: crate::scheduler::core::BluetoothSingleItemSchedulerRunning<
        crate::le::scanning::passive::active::BluetoothPassiveScanCompletionRole,
    >,
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanFirstRunning<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        LegacyPassiveScanWindowInFlight,
        BluetoothPassiveScanEventPhase,
        crate::scheduler::core::BluetoothSingleItemSchedulerRunning<
            crate::le::scanning::passive::active::BluetoothPassiveScanCompletionRole,
        >,
    ) {
        (self.task, self.window, self.phase, self.running)
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

/// Scanner owner sealed after an RX-list publication could not join its graph.
///
/// The task service is retained with the proof-mismatch owner because the
/// first MMIO write already completed. No operation can relabel this state as
/// retryable or restore an idle Controller.
#[must_use = "retain the permanently faulted Controller and scanner owners"]
pub struct BluetoothPassiveScanFirstRunnerPublicationFailStop<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    _window: LegacyPassiveScanWindowInFlight,
    _phase: BluetoothPassiveScanEventPhase,
    _task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
    ownership: BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership,
}

enum BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership {
    RxPublication(crate::BluetoothPassiveScanSchedulerHeadPublicationFailure),
    RetryabilityInvariant {
        _merged: BluetoothPassiveScanEmptySchedulerMergePrepared,
    },
}

impl<S, const SCHEDULER_CAPACITY: usize>
    BluetoothPassiveScanFirstRunnerPublicationFailStop<'_, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Exact RX proof mismatch observed after the irreversible publication.
    pub const fn error(
        &self,
    ) -> Option<
        open_esp_radio_esp32s31_bluetooth_memory::BluetoothPassiveScanMemoryGraphPublicationError,
    > {
        match &self.ownership {
            BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership::RxPublication(failure) => {
                failure.rx_publication_error()
            }
            BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership::RetryabilityInvariant {
                ..
            } => None,
        }
    }
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
        request: BluetoothPassiveScanWindowRequest,
        failure: BluetoothAlwaysAwakePostEnableTimeBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    ColdRecheck {
        request: BluetoothPassiveScanWindowRequest,
        failure: BluetoothAlwaysAwakePostEnableTimeFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmBegin {
        request: BluetoothPassiveScanWindowRequest,
        failure: BluetoothControllerSchedulerCurrentBeginFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    WarmRecheck {
        request: BluetoothPassiveScanWindowRequest,
        failure: BluetoothControllerSchedulerCurrentFailure<'runtime, S, SCHEDULER_CAPACITY>,
    },
    Recovered {
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: Option<BluetoothPassiveScanEventPhase>,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPassiveScanControllerPreparationError,
    },
    PreparationFailStop {
        request: BluetoothPassiveScanWindowRequest,
        failure: BluetoothPassiveScanControllerPreparationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    },
    PublicationFailStop(
        BluetoothPassiveScanFirstRunnerPublicationFailStop<'runtime, S, SCHEDULER_CAPACITY>,
    ),
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
        Self::begin_request(task, BluetoothPassiveScanWindowRequest::first(scanner))
    }

    /// Begin the next interval-preserving scanner window.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    pub fn begin_recurring(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        scanner: LegacyPassiveScannerEnabled,
        previous_phase: BluetoothPassiveScanEventPhase,
    ) -> Result<Self, BluetoothPassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        Self::begin_request(
            task,
            BluetoothPassiveScanWindowRequest::recurring(scanner, previous_phase),
        )
    }

    #[allow(
        clippy::result_large_err,
        reason = "the no-alloc rejection retains the exact scanner and Controller owner"
    )]
    fn begin_request(
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        request: BluetoothPassiveScanWindowRequest,
    ) -> Result<Self, BluetoothPassiveScanFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>> {
        match task.retain_scheduler_epoch() {
            Ok(epoch) => match epoch.begin_fresh_scheduler_current() {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { request, pending },
                )),
                Err(failure) => {
                    Err(BluetoothPassiveScanFirstRunnerFailure::WarmBegin { request, failure })
                }
            },
            Err(unavailable) => match unavailable
                .into_task_service()
                .begin_always_awake_post_enable_time()
            {
                Ok(pending) => Ok(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { request, pending },
                )),
                Err(failure) => {
                    Err(BluetoothPassiveScanFirstRunnerFailure::ColdBegin { request, failure })
                }
            },
        }
    }

    /// Execute exactly one lower transition.
    pub fn step(self) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        match self.phase {
            BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { request, pending } => {
                match pending.recheck() {
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Waiting(pending)) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::ColdCurrent { request, pending },
                        ))
                    }
                    Ok(BluetoothAlwaysAwakePostEnableTimeStep::Ready(ready)) => {
                        BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::CurrentReady {
                                request,
                                current: ready.initialize_scheduler_epoch(),
                            },
                        ))
                    }
                    Err(failure) => BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::ColdRecheck { request, failure },
                    ),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { request, pending } => {
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::WarmCurrent { request, pending },
                        ))
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::CurrentReady { request, current },
                        ))
                    }
                    Err(failure) => BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::WarmRecheck { request, failure },
                    ),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::CurrentReady { request, current } => {
                let parameters = request.window.parameters();
                let channel = request.window.channel();
                match current.begin_passive_scan_first_event(
                    parameters,
                    channel,
                    request.previous_phase,
                ) {
                    Ok(pending) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::Preparation { request, pending },
                        ))
                    }
                    Err(BluetoothPassiveScanControllerInitialPreparationFailure::Rejected {
                        current,
                        error,
                    }) => Self::recovered_failure(
                        request,
                        current.into_retained_epoch().into_task_service(),
                        error,
                    ),
                    Err(BluetoothPassiveScanControllerInitialPreparationFailure::FailStop(
                        failure,
                    )) => BluetoothPassiveScanFirstRunnerStep::Failed(
                        BluetoothPassiveScanFirstRunnerFailure::PreparationFailStop {
                            request,
                            failure,
                        },
                    ),
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::Preparation { request, pending } => {
                match pending.recheck() {
                    BluetoothPassiveScanControllerPreparationStep::Pending(pending) => {
                        BluetoothPassiveScanFirstRunnerStep::WaitControllerTime(Self::from_phase(
                            BluetoothPassiveScanFirstRunnerPhase::Preparation { request, pending },
                        ))
                    }
                    BluetoothPassiveScanControllerPreparationStep::Terminal(terminal) => {
                        Self::finish_preparation(request, terminal)
                    }
                    BluetoothPassiveScanControllerPreparationStep::FailStop(failure) => {
                        BluetoothPassiveScanFirstRunnerStep::Failed(
                            BluetoothPassiveScanFirstRunnerFailure::PreparationFailStop {
                                request,
                                failure,
                            },
                        )
                    }
                }
            }
            BluetoothPassiveScanFirstRunnerPhase::Prepared {
                window,
                phase,
                mut task,
                merged,
            } => match task.publish_passive_scan_scheduler_head(merged) {
                Ok(head) => BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::Head {
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
                            Some(error) => BluetoothPassiveScanFirstRunnerStep::Failed(
                                BluetoothPassiveScanFirstRunnerFailure::Retryable(
                                    BluetoothPassiveScanFirstRunnerRetry {
                                        cause: BluetoothPassiveScanFirstRunnerRetryCause::HeadPublication(
                                            error,
                                        ),
                                        runner: Self::from_phase(
                                            BluetoothPassiveScanFirstRunnerPhase::Prepared {
                                                window,
                                                phase,
                                                task,
                                                merged,
                                            },
                                        ),
                                    },
                                ),
                            ),
                            None => BluetoothPassiveScanFirstRunnerStep::Failed(
                                BluetoothPassiveScanFirstRunnerFailure::PublicationFailStop(
                                    BluetoothPassiveScanFirstRunnerPublicationFailStop {
                                        _window: window,
                                        _phase: phase,
                                        _task: task,
                                        ownership: BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership::RetryabilityInvariant {
                                            _merged: merged,
                                        },
                                    },
                                ),
                            ),
                        },
                        Err(failure) => BluetoothPassiveScanFirstRunnerStep::Failed(
                            BluetoothPassiveScanFirstRunnerFailure::PublicationFailStop(
                                BluetoothPassiveScanFirstRunnerPublicationFailStop {
                                    _window: window,
                                    _phase: phase,
                                    _task: task,
                                    ownership: BluetoothPassiveScanFirstRunnerPublicationFailStopOwnership::RxPublication(failure),
                                },
                            ),
                        ),
                    }
                }
            },
            BluetoothPassiveScanFirstRunnerPhase::Head {
                window,
                phase,
                mut task,
                head,
            } => match task.start_passive_scan_scheduler(head) {
                Ok(running) => {
                    BluetoothPassiveScanFirstRunnerStep::Running(BluetoothPassiveScanFirstRunning {
                        window,
                        phase,
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
                                        phase,
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
        request: BluetoothPassiveScanWindowRequest,
        terminal: crate::BluetoothPassiveScanControllerPreparationTerminal<
            'runtime,
            S,
            SCHEDULER_CAPACITY,
        >,
    ) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (epoch, outcome) = terminal.into_parts();
        let task = epoch.into_task_service();
        match outcome {
            BluetoothPassiveScanControllerPreparationOutcome::Prepared { merged, phase } => {
                BluetoothPassiveScanFirstRunnerStep::Continue(Self::from_phase(
                    BluetoothPassiveScanFirstRunnerPhase::Prepared {
                        window: request.window,
                        phase,
                        task,
                        merged,
                    },
                ))
            }
            BluetoothPassiveScanControllerPreparationOutcome::Rejected(error) => {
                Self::recovered_failure(request, task, error)
            }
        }
    }

    fn recovered_failure(
        request: BluetoothPassiveScanWindowRequest,
        task: BluetoothControllerPublishedTaskService<'runtime, S, SCHEDULER_CAPACITY>,
        error: BluetoothPassiveScanControllerPreparationError,
    ) -> BluetoothPassiveScanFirstRunnerStep<'runtime, S, SCHEDULER_CAPACITY> {
        let (scanner, previous_phase) = request.cancel();
        BluetoothPassiveScanFirstRunnerStep::Failed(
            BluetoothPassiveScanFirstRunnerFailure::Recovered {
                scanner,
                previous_phase,
                task,
                error,
            },
        )
    }
}
