//! Bounded successor scheduling for active legacy advertising.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_hci::LeControllerCommandReady;
use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddress;

use crate::controller_start::{
    BluetoothLegacyAdvertisingRecurringCandidateFailure,
    BluetoothLegacyAdvertisingRecurringSequenceCompletion,
};
use crate::{
    BluetoothControllerPublishedTaskService, BluetoothControllerSchedulerCurrentBeginError,
    BluetoothControllerSchedulerCurrentError, BluetoothControllerSchedulerCurrentPending,
    BluetoothControllerSchedulerCurrentStep, BluetoothLegacyAdvertisingActiveSession,
    BluetoothLegacyAdvertisingEmptySchedulerMergePrepared, BluetoothLegacyAdvertisingEventCpuOwned,
    BluetoothLegacyAdvertisingEventPrepared, BluetoothLegacyAdvertisingNextEventScheduled,
    BluetoothLegacyAdvertisingRecurringEventCandidate,
    BluetoothLegacyAdvertisingRecurringEventPreparationError,
    BluetoothLegacyAdvertisingRecurringPreSequence,
    BluetoothLegacyAdvertisingRecurringPreparationError,
    BluetoothLegacyAdvertisingRecurringPreparationFailure,
    BluetoothLegacyAdvertisingSchedulerHeadPublished, BluetoothSchedulerEmptyListMergeError,
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerHeadPublicationError,
    BluetoothSchedulerRunInterruptStorage,
};

type Task<'runtime, S, const CAPACITY: usize> =
    BluetoothControllerPublishedTaskService<'runtime, S, CAPACITY>;
type Order<'runtime> = LeControllerCommandReady<'runtime, ()>;

struct BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, const CAPACITY: usize> {
    task: Option<Task<'runtime, S, CAPACITY>>,
    order: Order<'runtime>,
    previous_scheduler_item_address: BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

enum BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, const CAPACITY: usize> {
    Scheduled(BluetoothLegacyAdvertisingNextEventScheduled<'static>),
    CandidatePreparationFailure(BluetoothLegacyAdvertisingRecurringPreparationFailure<'static>),
    Candidate(BluetoothLegacyAdvertisingRecurringEventCandidate<'static>),
    SequenceBegin(BluetoothLegacyAdvertisingRecurringPreSequence<'static>),
    SequenceWait {
        pending: BluetoothControllerSchedulerCurrentPending<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
    Merge(BluetoothLegacyAdvertisingEventPrepared<'static>),
    Merged(BluetoothLegacyAdvertisingEmptySchedulerMergePrepared<'static>),
    Head(BluetoothLegacyAdvertisingSchedulerHeadPublished<'static>),
}

/// One successor from a completed event through the next scheduler `RUN`.
#[must_use = "drive, retry, or retain the exact recurring advertising owner"]
pub struct BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    axes: Option<BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>>,
    phase: BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
}

/// Result of attaching the caller's fresh advertising delay.
#[must_use = "retain the recurring runner or sequence-exhausted CPU owner"]
pub enum BluetoothLegacyAdvertisingRecurringStart<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Runner(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    SequenceExhausted(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
}

/// One finite recurring transition.
#[must_use = "retain the runner, wait, running graph, retry, or fail-stop owner"]
pub enum BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    WaitControllerTime(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Running(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    Retryable(BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// Finite reason a lossless recurring phase asks its supervisor to retry.
#[derive(Debug)]
pub enum BluetoothLegacyAdvertisingRecurringRetryCause<E> {
    Preparation(BluetoothLegacyAdvertisingRecurringPreparationError),
    Event(BluetoothLegacyAdvertisingRecurringEventPreparationError),
    ControllerTimeBegin(BluetoothControllerSchedulerCurrentBeginError),
    EmptyList(BluetoothSchedulerEmptyListMergeError),
    HeadPublication(BluetoothSchedulerHeadPublicationError),
    SchedulerStart(E),
}

/// Exact retryable recurring phase.
#[must_use = "inspect and retry the unchanged recurring runner"]
pub struct BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingRecurringRetryCause<S::Error>,
    runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> &BluetoothLegacyAdvertisingRecurringRetryCause<S::Error> {
        &self.cause
    }

    pub fn retry(self) -> BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY> {
        self.runner
    }
}

/// Non-retryable recurring ownership failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothLegacyAdvertisingRecurringFaultCause {
    SchedulerEpochUnavailable,
    ControllerTime(BluetoothControllerSchedulerCurrentError),
}

#[allow(
    dead_code,
    reason = "the fail-stop owner intentionally retains every recurrence input"
)]
enum BluetoothLegacyAdvertisingRecurringFaultOwner<'runtime, S, const CAPACITY: usize> {
    Scheduled {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        scheduled: BluetoothLegacyAdvertisingNextEventScheduled<'static>,
    },
    SequenceBegin {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
    SequenceRecheck {
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        admitted: BluetoothLegacyAdvertisingRecurringPreSequence<'static>,
    },
}

/// Opaque fail-stop owner for a recurring event.
#[must_use = "retain the exact failed recurrence for diagnostic shutdown"]
pub struct BluetoothLegacyAdvertisingRecurringFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothLegacyAdvertisingRecurringFaultCause,
    _owner: BluetoothLegacyAdvertisingRecurringFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothLegacyAdvertisingRecurringFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub const fn cause(&self) -> BluetoothLegacyAdvertisingRecurringFaultCause {
        self.cause
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Attach one source-owned random delay without hiding entropy policy.
    pub fn begin_recurring(
        self,
        delay: AdvertisingDelay,
    ) -> BluetoothLegacyAdvertisingRecurringStart<'runtime, S, CAPACITY> {
        let (task, order, address, index, completed) = self.into_parts();
        match completed.schedule_next(delay) {
            Ok(scheduled) => BluetoothLegacyAdvertisingRecurringStart::Runner(
                BluetoothLegacyAdvertisingRecurringRunner {
                    axes: Some(BluetoothLegacyAdvertisingRecurringAxes {
                        task: Some(task),
                        order,
                        previous_scheduler_item_address: address,
                        hardware_list_index: index,
                    }),
                    phase: BluetoothLegacyAdvertisingRecurringPhase::Scheduled(scheduled),
                },
            ),
            Err(failure) => BluetoothLegacyAdvertisingRecurringStart::SequenceExhausted(
                BluetoothLegacyAdvertisingEventCpuOwned::from_parts(
                    task,
                    order,
                    address,
                    index,
                    failure.into_completed(),
                ),
            ),
        }
    }
}

impl<'runtime, S, const CAPACITY: usize>
    BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    fn from_parts(
        axes: BluetoothLegacyAdvertisingRecurringAxes<'runtime, S, CAPACITY>,
        phase: BluetoothLegacyAdvertisingRecurringPhase<'runtime, S, CAPACITY>,
    ) -> Self {
        Self {
            axes: Some(axes),
            phase,
        }
    }

    fn retryable(
        self,
        cause: BluetoothLegacyAdvertisingRecurringRetryCause<S::Error>,
    ) -> BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        BluetoothLegacyAdvertisingRecurringRunnerStep::Retryable(
            BluetoothLegacyAdvertisingRecurringRetry {
                cause,
                runner: self,
            },
        )
    }

    /// Execute exactly one recurrence edge.
    pub fn step(mut self) -> BluetoothLegacyAdvertisingRecurringRunnerStep<'runtime, S, CAPACITY> {
        let axes = self
            .axes
            .take()
            .expect("a recurring runner retains its exact Controller axes");
        match self.phase {
            BluetoothLegacyAdvertisingRecurringPhase::Scheduled(scheduled) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a scheduled recurrence retains its task service")
                    .prepare_legacy_advertising_recurring_candidate(scheduled)
                {
                    Ok(candidate) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ),
                    ),
                    Err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation(
                        failure,
                    )) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Preparation(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(
                                failure,
                            ),
                        )
                        .retryable(cause)
                    }
                    Err(
                        BluetoothLegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                            scheduled,
                        ),
                    ) => BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                        BluetoothLegacyAdvertisingRecurringFault {
                            cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(failure) => {
                match axes
                    .task
                    .as_ref()
                    .expect("a preparation retry retains its task service")
                    .retry_legacy_advertising_recurring_candidate(failure)
                {
                    Ok(candidate) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate),
                        ),
                    ),
                    Err(BluetoothLegacyAdvertisingRecurringCandidateFailure::Preparation(
                        failure,
                    )) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Preparation(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::CandidatePreparationFailure(
                                failure,
                            ),
                        )
                        .retryable(cause)
                    }
                    Err(
                        BluetoothLegacyAdvertisingRecurringCandidateFailure::SchedulerEpochUnavailable(
                            scheduled,
                        ),
                    ) => BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                        BluetoothLegacyAdvertisingRecurringFault {
                            cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                            _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::Scheduled {
                                axes,
                                scheduled,
                            },
                        },
                    ),
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Candidate(candidate) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a candidate recurrence retains its task service")
                    .admit_legacy_advertising_recurring_candidate(candidate)
                {
                    Ok(admitted) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Event(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Candidate(
                                failure.into_candidate(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted) => {
                let BluetoothLegacyAdvertisingRecurringAxes {
                    task,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                let task = task.expect("a sequence-begin recurrence retains its task service");
                let epoch = match task.retain_scheduler_epoch() {
                    Ok(epoch) => epoch,
                    Err(unavailable) => {
                        return BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                            BluetoothLegacyAdvertisingRecurringFault {
                                cause: BluetoothLegacyAdvertisingRecurringFaultCause::SchedulerEpochUnavailable,
                                _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::SequenceBegin {
                                    axes: BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(unavailable.into_task_service()),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    admitted,
                                },
                            },
                        );
                    }
                };
                match epoch.begin_fresh_scheduler_current() {
                    Ok(pending) => BluetoothLegacyAdvertisingRecurringRunnerStep::WaitControllerTime(
                        Self::from_parts(
                            BluetoothLegacyAdvertisingRecurringAxes {
                                task: None,
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceWait {
                                pending,
                                admitted,
                            },
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::ControllerTimeBegin(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        Self::from_parts(
                            BluetoothLegacyAdvertisingRecurringAxes {
                                task: Some(task),
                                order,
                                previous_scheduler_item_address,
                                hardware_list_index,
                            },
                            BluetoothLegacyAdvertisingRecurringPhase::SequenceBegin(admitted),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::SequenceWait { pending, admitted } => {
                let BluetoothLegacyAdvertisingRecurringAxes {
                    task: _,
                    order,
                    previous_scheduler_item_address,
                    hardware_list_index,
                } = axes;
                match pending.recheck() {
                    Ok(BluetoothControllerSchedulerCurrentStep::Waiting(pending)) => {
                        BluetoothLegacyAdvertisingRecurringRunnerStep::WaitControllerTime(
                            Self::from_parts(
                                BluetoothLegacyAdvertisingRecurringAxes {
                                    task: None,
                                    order,
                                    previous_scheduler_item_address,
                                    hardware_list_index,
                                },
                                BluetoothLegacyAdvertisingRecurringPhase::SequenceWait {
                                    pending,
                                    admitted,
                                },
                            ),
                        )
                    }
                    Ok(BluetoothControllerSchedulerCurrentStep::Ready(current)) => {
                        match current.finish_legacy_advertising_recurring_event(admitted) {
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::Prepared {
                                task,
                                merged,
                            } => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Merged(merged),
                                ),
                            ),
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::EventRejected {
                                task,
                                failure,
                            } => {
                                let cause = BluetoothLegacyAdvertisingRecurringRetryCause::Event(
                                    failure.error(),
                                );
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Candidate(
                                        failure.into_candidate(),
                                    ),
                                )
                                .retryable(cause)
                            }
                            BluetoothLegacyAdvertisingRecurringSequenceCompletion::EmptyListRejected {
                                task,
                                failure,
                            } => {
                                let cause = BluetoothLegacyAdvertisingRecurringRetryCause::EmptyList(
                                    failure.error(),
                                );
                                Self::from_parts(
                                    BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    BluetoothLegacyAdvertisingRecurringPhase::Merge(
                                        failure.into_prepared(),
                                    ),
                                )
                                .retryable(cause)
                            }
                        }
                    }
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringFaultCause::ControllerTime(
                            failure.error(),
                        );
                        let task = failure.into_parts().0.into_task_service();
                        BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(
                            BluetoothLegacyAdvertisingRecurringFault {
                                cause,
                                _owner: BluetoothLegacyAdvertisingRecurringFaultOwner::SequenceRecheck {
                                    axes: BluetoothLegacyAdvertisingRecurringAxes {
                                        task: Some(task),
                                        order,
                                        previous_scheduler_item_address,
                                        hardware_list_index,
                                    },
                                    admitted,
                                },
                            },
                        )
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merge(prepared) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merge retry retains its task service")
                    .merge_legacy_advertising_recurring_event(prepared)
                {
                    Ok(merged) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merged(merged),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::EmptyList(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merge(
                                failure.into_prepared(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Merged(merged) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a merged recurrence retains its task service")
                    .publish_legacy_advertising_scheduler_head(merged)
                {
                    Ok(head) => BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Head(head),
                        ),
                    ),
                    Err(failure) => {
                        let cause = BluetoothLegacyAdvertisingRecurringRetryCause::HeadPublication(
                            failure.error(),
                        );
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Merged(
                                failure.into_merged(),
                            ),
                        )
                        .retryable(cause)
                    }
                }
            }
            BluetoothLegacyAdvertisingRecurringPhase::Head(head) => {
                let mut axes = axes;
                match axes
                    .task
                    .as_mut()
                    .expect("a published recurrence retains its task service")
                    .start_legacy_advertising_scheduler(head)
                {
                    Ok(running) => BluetoothLegacyAdvertisingRecurringRunnerStep::Running(
                        BluetoothLegacyAdvertisingActiveSession::from_recurring_running(
                            axes
                                .task
                                .take()
                                .expect("the running recurrence consumes its task service"),
                            axes.order,
                            running,
                        ),
                    ),
                    Err(failure) => {
                        let (error, head) = failure.into_parts();
                        Self::from_parts(
                            axes,
                            BluetoothLegacyAdvertisingRecurringPhase::Head(head),
                        )
                        .retryable(
                            BluetoothLegacyAdvertisingRecurringRetryCause::SchedulerStart(error),
                        )
                    }
                }
            }
        }
    }
}
