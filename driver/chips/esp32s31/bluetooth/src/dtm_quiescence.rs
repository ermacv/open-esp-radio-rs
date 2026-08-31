//! Terminal-neutral quiescence for one active LE DTM radio graph.
//!
//! Test End and Controller Reset share this single ownership machine. Before a
//! scheduler head is visible it cancels recurrence and drains an abandoned
//! Controller-time request. Once a head is visible, exactly that event reaches
//! `RUN` and follows the ordinary completion/unlink/recycle path. The sole
//! successful terminal is [`BluetoothDtmActiveCpuOwned`]; command policy and
//! response ordering remain outside this module.

#![forbid(unsafe_code)]

use crate::dtm_active_session::BluetoothDtmActiveRadio;
use crate::dtm_quiescence_policy::{
    BluetoothDtmQuiescenceRetryAction, BluetoothDtmQuiescenceRetryOwnership,
    bluetooth_dtm_quiescence_retry_action,
};
use crate::{
    BluetoothDtmActiveCompletion, BluetoothDtmActiveCompletionFault,
    BluetoothDtmActiveCompletionFaultCause, BluetoothDtmActiveCompletionStep,
    BluetoothDtmActiveCpuOwned, BluetoothDtmActivePostUnlinkWait, BluetoothDtmActiveSchedulerWait,
    BluetoothDtmPostUnlinkWakeCell, BluetoothDtmRecurringCancellationDrain,
    BluetoothDtmRecurringCancellationDrainStep, BluetoothDtmRecurringControllerTimeWait,
    BluetoothDtmRecurringFault, BluetoothDtmRecurringFaultCause, BluetoothDtmRecurringRetry,
    BluetoothDtmRecurringRetryCause, BluetoothDtmRecurringRunner,
    BluetoothDtmRecurringRunnerCancel, BluetoothDtmRecurringRunnerStep,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
    BluetoothSchedulerWakeCell,
};

enum BluetoothDtmQuiescencePhase<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completion(BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>),
    SchedulerWait(BluetoothDtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    PostUnlinkWait(BluetoothDtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    CancelRecurring(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    CancelRejected(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    CancelRetry(BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>),
    CancellationDrain(BluetoothDtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    FinishPublished(BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>),
    FinishPublishedRetry(BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>),
}

pub(crate) struct BluetoothDtmQuiescenceRunner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    phase: BluetoothDtmQuiescencePhase<'runtime, S, CAPACITY>,
}

pub(crate) enum BluetoothDtmQuiescenceWait<'runner> {
    Scheduler(&'runner BluetoothSchedulerWakeCell),
    PostUnlink(&'runner BluetoothDtmPostUnlinkWakeCell),
    ControllerTime,
}

pub(crate) enum BluetoothDtmQuiescenceRetryCause<'cause, E> {
    CancellationRejected,
    SchedulerStart(&'cause E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothDtmQuiescenceFaultCause {
    Completion(BluetoothDtmActiveCompletionFaultCause),
    Recurring(BluetoothDtmRecurringFaultCause),
    UnexpectedPublishedHeadTransition,
}

enum BluetoothDtmQuiescenceFaultOwner<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Completion {
        _owner: BluetoothDtmActiveCompletionFault<'runtime, S, CAPACITY>,
    },
    Recurring {
        _owner: BluetoothDtmRecurringFault<'runtime, S, CAPACITY>,
    },
    PublishedRunner {
        _owner: BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>,
    },
    PublishedWait {
        _owner: BluetoothDtmRecurringControllerTimeWait<'runtime, S, CAPACITY>,
    },
    PublishedRetry {
        _owner: BluetoothDtmRecurringRetry<'runtime, S, CAPACITY>,
    },
}

pub(crate) struct BluetoothDtmQuiescenceFault<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    cause: BluetoothDtmQuiescenceFaultCause,
    _owner: BluetoothDtmQuiescenceFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> BluetoothDtmQuiescenceFault<'_, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) const fn cause(&self) -> BluetoothDtmQuiescenceFaultCause {
        self.cause
    }
}

pub(crate) enum BluetoothDtmQuiescenceStep<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Continue(BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>),
    Waiting(BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>),
    UnrelatedList {
        runner: BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Retryable(BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>),
    CpuOwned(BluetoothDtmActiveCpuOwned<'runtime, S, CAPACITY>),
    Fault(BluetoothDtmQuiescenceFault<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize> BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub(crate) fn new(radio: BluetoothDtmActiveRadio<'runtime, S, CAPACITY>) -> Self {
        let phase = match radio {
            BluetoothDtmActiveRadio::Completion(completion) => {
                BluetoothDtmQuiescencePhase::Completion(completion)
            }
            BluetoothDtmActiveRadio::SchedulerWait(wait) => {
                BluetoothDtmQuiescencePhase::SchedulerWait(wait)
            }
            BluetoothDtmActiveRadio::PostUnlinkWait(wait) => {
                BluetoothDtmQuiescencePhase::PostUnlinkWait(wait)
            }
            BluetoothDtmActiveRadio::Recurring(recurring) => {
                BluetoothDtmQuiescencePhase::CancelRecurring(recurring)
            }
            BluetoothDtmActiveRadio::ControllerTimeWait(wait) => {
                BluetoothDtmQuiescencePhase::CancelRecurring(wait.resume())
            }
            BluetoothDtmActiveRadio::Retryable(retry) => {
                let ownership = match retry.cause() {
                    BluetoothDtmRecurringRetryCause::Preparation(_)
                    | BluetoothDtmRecurringRetryCause::HeadPublication(_) => {
                        BluetoothDtmQuiescenceRetryOwnership::BeforeHead
                    }
                    BluetoothDtmRecurringRetryCause::SchedulerStart(_) => {
                        BluetoothDtmQuiescenceRetryOwnership::HeadPublished
                    }
                };
                match bluetooth_dtm_quiescence_retry_action(ownership) {
                    BluetoothDtmQuiescenceRetryAction::CancelBeforeHead => {
                        BluetoothDtmQuiescencePhase::CancelRetry(retry)
                    }
                    BluetoothDtmQuiescenceRetryAction::FinishPublishedHead => {
                        BluetoothDtmQuiescencePhase::FinishPublishedRetry(retry)
                    }
                }
            }
        };
        Self { phase }
    }

    pub(crate) fn wait(&self) -> Option<BluetoothDtmQuiescenceWait<'_>> {
        match &self.phase {
            BluetoothDtmQuiescencePhase::SchedulerWait(wait) => {
                Some(BluetoothDtmQuiescenceWait::Scheduler(wait.wake()))
            }
            BluetoothDtmQuiescencePhase::PostUnlinkWait(wait) => {
                Some(BluetoothDtmQuiescenceWait::PostUnlink(wait.wake()))
            }
            BluetoothDtmQuiescencePhase::CancellationDrain(_) => {
                Some(BluetoothDtmQuiescenceWait::ControllerTime)
            }
            BluetoothDtmQuiescencePhase::Completion(_)
            | BluetoothDtmQuiescencePhase::CancelRecurring(_)
            | BluetoothDtmQuiescencePhase::CancelRejected(_)
            | BluetoothDtmQuiescencePhase::CancelRetry(_)
            | BluetoothDtmQuiescencePhase::FinishPublished(_)
            | BluetoothDtmQuiescencePhase::FinishPublishedRetry(_) => None,
        }
    }

    pub(crate) fn retry_cause(&self) -> Option<BluetoothDtmQuiescenceRetryCause<'_, S::Error>> {
        match &self.phase {
            BluetoothDtmQuiescencePhase::CancelRejected(_) => {
                Some(BluetoothDtmQuiescenceRetryCause::CancellationRejected)
            }
            BluetoothDtmQuiescencePhase::FinishPublishedRetry(retry) => match retry.cause() {
                BluetoothDtmRecurringRetryCause::SchedulerStart(error) => {
                    Some(BluetoothDtmQuiescenceRetryCause::SchedulerStart(error))
                }
                BluetoothDtmRecurringRetryCause::Preparation(_)
                | BluetoothDtmRecurringRetryCause::HeadPublication(_) => None,
            },
            _ => None,
        }
    }

    pub(crate) fn step(self) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY> {
        match self.phase {
            BluetoothDtmQuiescencePhase::Completion(completion) => step_completion(completion),
            BluetoothDtmQuiescencePhase::SchedulerWait(wait) => {
                let _observed = wait.wake().take();
                step_completion(wait.resume())
            }
            BluetoothDtmQuiescencePhase::PostUnlinkWait(wait) => step_completion(wait.resume()),
            BluetoothDtmQuiescencePhase::CancelRecurring(recurring)
            | BluetoothDtmQuiescencePhase::CancelRejected(recurring) => {
                finish_cancellation(recurring.cancel())
            }
            BluetoothDtmQuiescencePhase::CancelRetry(retry) => {
                finish_cancellation(retry.cancel_for_quiescence())
            }
            BluetoothDtmQuiescencePhase::CancellationDrain(drain) => step_cancellation_drain(drain),
            BluetoothDtmQuiescencePhase::FinishPublished(recurring) => {
                step_published_head(recurring)
            }
            BluetoothDtmQuiescencePhase::FinishPublishedRetry(retry) => {
                step_published_head(retry.retry())
            }
        }
    }
}

fn runner<'runtime, S, const CAPACITY: usize>(
    phase: BluetoothDtmQuiescencePhase<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceRunner<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    BluetoothDtmQuiescenceRunner { phase }
}

fn step_completion<'runtime, S, const CAPACITY: usize>(
    completion: BluetoothDtmActiveCompletion<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match completion.step() {
        BluetoothDtmActiveCompletionStep::Continue(completion) => {
            BluetoothDtmQuiescenceStep::Continue(runner(BluetoothDtmQuiescencePhase::Completion(
                completion,
            )))
        }
        BluetoothDtmActiveCompletionStep::WaitScheduler(wait) => {
            BluetoothDtmQuiescenceStep::Waiting(runner(BluetoothDtmQuiescencePhase::SchedulerWait(
                wait,
            )))
        }
        BluetoothDtmActiveCompletionStep::UnrelatedList {
            completion,
            observed,
        } => BluetoothDtmQuiescenceStep::UnrelatedList {
            runner: runner(BluetoothDtmQuiescencePhase::Completion(completion)),
            observed,
        },
        BluetoothDtmActiveCompletionStep::WaitPostUnlink(wait) => {
            BluetoothDtmQuiescenceStep::Waiting(runner(
                BluetoothDtmQuiescencePhase::PostUnlinkWait(wait),
            ))
        }
        BluetoothDtmActiveCompletionStep::CpuOwned(owner) => {
            BluetoothDtmQuiescenceStep::CpuOwned(owner)
        }
        BluetoothDtmActiveCompletionStep::Fault(fault) => {
            BluetoothDtmQuiescenceStep::Fault(BluetoothDtmQuiescenceFault {
                cause: BluetoothDtmQuiescenceFaultCause::Completion(fault.cause()),
                _owner: BluetoothDtmQuiescenceFaultOwner::Completion { _owner: fault },
            })
        }
    }
}

fn finish_cancellation<'runtime, S, const CAPACITY: usize>(
    cancelled: BluetoothDtmRecurringRunnerCancel<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match cancelled {
        BluetoothDtmRecurringRunnerCancel::CpuOwned(owner) => {
            BluetoothDtmQuiescenceStep::CpuOwned(owner)
        }
        BluetoothDtmRecurringRunnerCancel::NeedsControllerTimeDrain(drain) => {
            BluetoothDtmQuiescenceStep::Continue(runner(
                BluetoothDtmQuiescencePhase::CancellationDrain(drain),
            ))
        }
        BluetoothDtmRecurringRunnerCancel::CancellationRejected(recurring) => {
            BluetoothDtmQuiescenceStep::Retryable(runner(
                BluetoothDtmQuiescencePhase::CancelRejected(recurring),
            ))
        }
        BluetoothDtmRecurringRunnerCancel::HeadPublished(recurring) => {
            BluetoothDtmQuiescenceStep::Continue(runner(
                BluetoothDtmQuiescencePhase::FinishPublished(recurring),
            ))
        }
        BluetoothDtmRecurringRunnerCancel::Fault(fault) => {
            BluetoothDtmQuiescenceStep::Fault(BluetoothDtmQuiescenceFault {
                cause: BluetoothDtmQuiescenceFaultCause::Recurring(fault.cause()),
                _owner: BluetoothDtmQuiescenceFaultOwner::Recurring { _owner: fault },
            })
        }
    }
}

fn step_cancellation_drain<'runtime, S, const CAPACITY: usize>(
    drain: BluetoothDtmRecurringCancellationDrain<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match drain.step() {
        BluetoothDtmRecurringCancellationDrainStep::Waiting(drain) => {
            BluetoothDtmQuiescenceStep::Waiting(runner(
                BluetoothDtmQuiescencePhase::CancellationDrain(drain),
            ))
        }
        BluetoothDtmRecurringCancellationDrainStep::CpuOwned(owner) => {
            BluetoothDtmQuiescenceStep::CpuOwned(owner)
        }
        BluetoothDtmRecurringCancellationDrainStep::Fault(fault) => {
            BluetoothDtmQuiescenceStep::Fault(BluetoothDtmQuiescenceFault {
                cause: BluetoothDtmQuiescenceFaultCause::Recurring(fault.cause()),
                _owner: BluetoothDtmQuiescenceFaultOwner::Recurring { _owner: fault },
            })
        }
    }
}

fn step_published_head<'runtime, S, const CAPACITY: usize>(
    recurring: BluetoothDtmRecurringRunner<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    match recurring.step() {
        BluetoothDtmRecurringRunnerStep::Running(completion) => {
            BluetoothDtmQuiescenceStep::Continue(runner(BluetoothDtmQuiescencePhase::Completion(
                completion,
            )))
        }
        BluetoothDtmRecurringRunnerStep::Retryable(retry)
            if matches!(
                retry.cause(),
                BluetoothDtmRecurringRetryCause::SchedulerStart(_)
            ) =>
        {
            BluetoothDtmQuiescenceStep::Retryable(runner(
                BluetoothDtmQuiescencePhase::FinishPublishedRetry(retry),
            ))
        }
        BluetoothDtmRecurringRunnerStep::Fault(fault) => {
            BluetoothDtmQuiescenceStep::Fault(BluetoothDtmQuiescenceFault {
                cause: BluetoothDtmQuiescenceFaultCause::Recurring(fault.cause()),
                _owner: BluetoothDtmQuiescenceFaultOwner::Recurring { _owner: fault },
            })
        }
        BluetoothDtmRecurringRunnerStep::Continue(recurring) => {
            unexpected_published(BluetoothDtmQuiescenceFaultOwner::PublishedRunner {
                _owner: recurring,
            })
        }
        BluetoothDtmRecurringRunnerStep::WaitControllerTime(wait) => {
            unexpected_published(BluetoothDtmQuiescenceFaultOwner::PublishedWait { _owner: wait })
        }
        BluetoothDtmRecurringRunnerStep::Retryable(retry) => {
            unexpected_published(BluetoothDtmQuiescenceFaultOwner::PublishedRetry { _owner: retry })
        }
    }
}

fn unexpected_published<'runtime, S, const CAPACITY: usize>(
    owner: BluetoothDtmQuiescenceFaultOwner<'runtime, S, CAPACITY>,
) -> BluetoothDtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    BluetoothDtmQuiescenceStep::Fault(BluetoothDtmQuiescenceFault {
        cause: BluetoothDtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition,
        _owner: owner,
    })
}
