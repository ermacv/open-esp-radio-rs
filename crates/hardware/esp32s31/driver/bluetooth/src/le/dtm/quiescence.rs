//! Terminal-neutral quiescence for one active LE DTM radio graph.
//!
//! Test End and Controller Reset share this single ownership machine. Before a
//! scheduler head is visible it cancels recurrence and drains an abandoned
//! Controller-time request. Once a head is visible, exactly that event reaches
//! `RUN` and follows the ordinary completion/unlink/recycle path. The sole
//! successful terminal is [`DtmActiveCpuOwned`]; command policy and
//! response ordering remain outside this module.

#![forbid(unsafe_code)]

use crate::{
    controller::SchedulerRunInterruptStorage,
    interrupt::SchedulerWakeCell,
    le::dtm::{
        DtmActiveCompletion, DtmActiveCompletionFault, DtmActiveCompletionFaultCause,
        DtmActiveCompletionStep, DtmActiveCpuOwned, DtmActivePostUnlinkWait,
        DtmActiveSchedulerWait, DtmPostUnlinkWakeCell, DtmRecurringCancellationDrain,
        DtmRecurringCancellationDrainStep, DtmRecurringControllerTimeWait, DtmRecurringFault,
        DtmRecurringFaultCause, DtmRecurringRetry, DtmRecurringRetryCause, DtmRecurringRunner,
        DtmRecurringRunnerCancel, DtmRecurringRunnerStep,
        active::session::DtmActiveRadio,
        quiescence_policy::{
            DtmQuiescenceRetryAction, DtmQuiescenceRetryOwnership,
            bluetooth_dtm_quiescence_retry_action,
        },
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

enum DtmQuiescencePhase<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completion(DtmActiveCompletion<'runtime, S, CAPACITY>),
    SchedulerWait(DtmActiveSchedulerWait<'runtime, S, CAPACITY>),
    PostUnlinkWait(DtmActivePostUnlinkWait<'runtime, S, CAPACITY>),
    CancelRecurring(DtmRecurringRunner<'runtime, S, CAPACITY>),
    CancelRejected(DtmRecurringRunner<'runtime, S, CAPACITY>),
    CancelRetry(DtmRecurringRetry<'runtime, S, CAPACITY>),
    CancellationDrain(DtmRecurringCancellationDrain<'runtime, S, CAPACITY>),
    FinishPublished(DtmRecurringRunner<'runtime, S, CAPACITY>),
    FinishPublishedRetry(DtmRecurringRetry<'runtime, S, CAPACITY>),
}

pub(crate) struct DtmQuiescenceRunner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    phase: DtmQuiescencePhase<'runtime, S, CAPACITY>,
}

pub(crate) enum DtmQuiescenceWait<'runner> {
    Scheduler(&'runner SchedulerWakeCell),
    PostUnlink(&'runner DtmPostUnlinkWakeCell),
    ControllerTime,
}

pub(crate) enum DtmQuiescenceRetryCause<'cause, E> {
    CancellationRejected,
    SchedulerStart(&'cause E),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DtmQuiescenceFaultCause {
    Completion(DtmActiveCompletionFaultCause),
    Recurring(DtmRecurringFaultCause),
    UnexpectedPublishedHeadTransition,
}

enum DtmQuiescenceFaultOwner<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Completion {
        _owner: DtmActiveCompletionFault<'runtime, S, CAPACITY>,
    },
    Recurring {
        _owner: DtmRecurringFault<'runtime, S, CAPACITY>,
    },
    PublishedRunner {
        _owner: DtmRecurringRunner<'runtime, S, CAPACITY>,
    },
    PublishedWait {
        _owner: DtmRecurringControllerTimeWait<'runtime, S, CAPACITY>,
    },
    PublishedRetry {
        _owner: DtmRecurringRetry<'runtime, S, CAPACITY>,
    },
}

pub(crate) struct DtmQuiescenceFault<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    cause: DtmQuiescenceFaultCause,
    _owner: DtmQuiescenceFaultOwner<'runtime, S, CAPACITY>,
}

impl<S, const CAPACITY: usize> DtmQuiescenceFault<'_, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) const fn cause(&self) -> DtmQuiescenceFaultCause {
        self.cause
    }
}

pub(crate) enum DtmQuiescenceStep<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Continue(DtmQuiescenceRunner<'runtime, S, CAPACITY>),
    Waiting(DtmQuiescenceRunner<'runtime, S, CAPACITY>),
    UnrelatedList {
        runner: DtmQuiescenceRunner<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Retryable(DtmQuiescenceRunner<'runtime, S, CAPACITY>),
    CpuOwned(DtmActiveCpuOwned<'runtime, S, CAPACITY>),
    Fault(DtmQuiescenceFault<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize> DtmQuiescenceRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub(crate) fn new(radio: DtmActiveRadio<'runtime, S, CAPACITY>) -> Self {
        let phase = match radio {
            DtmActiveRadio::Completion(completion) => DtmQuiescencePhase::Completion(completion),
            DtmActiveRadio::SchedulerWait(wait) => DtmQuiescencePhase::SchedulerWait(wait),
            DtmActiveRadio::PostUnlinkWait(wait) => DtmQuiescencePhase::PostUnlinkWait(wait),
            DtmActiveRadio::Recurring(recurring) => DtmQuiescencePhase::CancelRecurring(recurring),
            DtmActiveRadio::ControllerTimeWait(wait) => {
                DtmQuiescencePhase::CancelRecurring(wait.resume())
            }
            DtmActiveRadio::Retryable(retry) => {
                let ownership = match retry.cause() {
                    DtmRecurringRetryCause::Preparation(_)
                    | DtmRecurringRetryCause::HeadPublication(_) => {
                        DtmQuiescenceRetryOwnership::BeforeHead
                    }
                    DtmRecurringRetryCause::SchedulerStart(_) => {
                        DtmQuiescenceRetryOwnership::HeadPublished
                    }
                };
                match bluetooth_dtm_quiescence_retry_action(ownership) {
                    DtmQuiescenceRetryAction::CancelBeforeHead => {
                        DtmQuiescencePhase::CancelRetry(retry)
                    }
                    DtmQuiescenceRetryAction::FinishPublishedHead => {
                        DtmQuiescencePhase::FinishPublishedRetry(retry)
                    }
                }
            }
        };
        Self { phase }
    }

    pub(crate) fn wait(&self) -> Option<DtmQuiescenceWait<'_>> {
        match &self.phase {
            DtmQuiescencePhase::SchedulerWait(wait) => {
                Some(DtmQuiescenceWait::Scheduler(wait.wake()))
            }
            DtmQuiescencePhase::PostUnlinkWait(wait) => {
                Some(DtmQuiescenceWait::PostUnlink(wait.wake()))
            }
            DtmQuiescencePhase::CancellationDrain(_) => Some(DtmQuiescenceWait::ControllerTime),
            DtmQuiescencePhase::Completion(_)
            | DtmQuiescencePhase::CancelRecurring(_)
            | DtmQuiescencePhase::CancelRejected(_)
            | DtmQuiescencePhase::CancelRetry(_)
            | DtmQuiescencePhase::FinishPublished(_)
            | DtmQuiescencePhase::FinishPublishedRetry(_) => None,
        }
    }

    pub(crate) fn retry_cause(&self) -> Option<DtmQuiescenceRetryCause<'_, S::Error>> {
        match &self.phase {
            DtmQuiescencePhase::CancelRejected(_) => {
                Some(DtmQuiescenceRetryCause::CancellationRejected)
            }
            DtmQuiescencePhase::FinishPublishedRetry(retry) => match retry.cause() {
                DtmRecurringRetryCause::SchedulerStart(error) => {
                    Some(DtmQuiescenceRetryCause::SchedulerStart(error))
                }
                DtmRecurringRetryCause::Preparation(_)
                | DtmRecurringRetryCause::HeadPublication(_) => None,
            },
            _ => None,
        }
    }

    pub(crate) fn step(self) -> DtmQuiescenceStep<'runtime, S, CAPACITY> {
        match self.phase {
            DtmQuiescencePhase::Completion(completion) => step_completion(completion),
            DtmQuiescencePhase::SchedulerWait(wait) => match wait.wake().take() {
                Some(wake) => step_completion(wait.resume(wake)),
                None => DtmQuiescenceStep::Waiting(runner(DtmQuiescencePhase::SchedulerWait(wait))),
            },
            DtmQuiescencePhase::PostUnlinkWait(wait) => step_completion(wait.resume()),
            DtmQuiescencePhase::CancelRecurring(recurring)
            | DtmQuiescencePhase::CancelRejected(recurring) => {
                finish_cancellation(recurring.cancel())
            }
            DtmQuiescencePhase::CancelRetry(retry) => {
                finish_cancellation(retry.cancel_for_quiescence())
            }
            DtmQuiescencePhase::CancellationDrain(drain) => step_cancellation_drain(drain),
            DtmQuiescencePhase::FinishPublished(recurring) => step_published_head(recurring),
            DtmQuiescencePhase::FinishPublishedRetry(retry) => step_published_head(retry.retry()),
        }
    }
}

fn runner<'runtime, S, const CAPACITY: usize>(
    phase: DtmQuiescencePhase<'runtime, S, CAPACITY>,
) -> DtmQuiescenceRunner<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    DtmQuiescenceRunner { phase }
}

fn step_completion<'runtime, S, const CAPACITY: usize>(
    completion: DtmActiveCompletion<'runtime, S, CAPACITY>,
) -> DtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match completion.step() {
        DtmActiveCompletionStep::Continue(completion) => {
            DtmQuiescenceStep::Continue(runner(DtmQuiescencePhase::Completion(completion)))
        }
        DtmActiveCompletionStep::WaitScheduler(wait) => {
            DtmQuiescenceStep::Waiting(runner(DtmQuiescencePhase::SchedulerWait(wait)))
        }
        DtmActiveCompletionStep::UnrelatedList {
            completion,
            observed,
        } => DtmQuiescenceStep::UnrelatedList {
            runner: runner(DtmQuiescencePhase::Completion(completion)),
            observed,
        },
        DtmActiveCompletionStep::WaitPostUnlink(wait) => {
            DtmQuiescenceStep::Waiting(runner(DtmQuiescencePhase::PostUnlinkWait(wait)))
        }
        DtmActiveCompletionStep::CpuOwned(owner) => DtmQuiescenceStep::CpuOwned(owner),
        DtmActiveCompletionStep::Fault(fault) => DtmQuiescenceStep::Fault(DtmQuiescenceFault {
            cause: DtmQuiescenceFaultCause::Completion(fault.cause()),
            _owner: DtmQuiescenceFaultOwner::Completion { _owner: fault },
        }),
    }
}

fn finish_cancellation<'runtime, S, const CAPACITY: usize>(
    cancelled: DtmRecurringRunnerCancel<'runtime, S, CAPACITY>,
) -> DtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match cancelled {
        DtmRecurringRunnerCancel::CpuOwned(owner) => DtmQuiescenceStep::CpuOwned(owner),
        DtmRecurringRunnerCancel::NeedsControllerTimeDrain(drain) => {
            DtmQuiescenceStep::Continue(runner(DtmQuiescencePhase::CancellationDrain(drain)))
        }
        DtmRecurringRunnerCancel::CancellationRejected(recurring) => {
            DtmQuiescenceStep::Retryable(runner(DtmQuiescencePhase::CancelRejected(recurring)))
        }
        DtmRecurringRunnerCancel::HeadPublished(recurring) => {
            DtmQuiescenceStep::Continue(runner(DtmQuiescencePhase::FinishPublished(recurring)))
        }
        DtmRecurringRunnerCancel::Fault(fault) => DtmQuiescenceStep::Fault(DtmQuiescenceFault {
            cause: DtmQuiescenceFaultCause::Recurring(fault.cause()),
            _owner: DtmQuiescenceFaultOwner::Recurring { _owner: fault },
        }),
    }
}

fn step_cancellation_drain<'runtime, S, const CAPACITY: usize>(
    drain: DtmRecurringCancellationDrain<'runtime, S, CAPACITY>,
) -> DtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match drain.step() {
        DtmRecurringCancellationDrainStep::Waiting(drain) => {
            DtmQuiescenceStep::Waiting(runner(DtmQuiescencePhase::CancellationDrain(drain)))
        }
        DtmRecurringCancellationDrainStep::CpuOwned(owner) => DtmQuiescenceStep::CpuOwned(owner),
        DtmRecurringCancellationDrainStep::Fault(fault) => {
            DtmQuiescenceStep::Fault(DtmQuiescenceFault {
                cause: DtmQuiescenceFaultCause::Recurring(fault.cause()),
                _owner: DtmQuiescenceFaultOwner::Recurring { _owner: fault },
            })
        }
    }
}

fn step_published_head<'runtime, S, const CAPACITY: usize>(
    recurring: DtmRecurringRunner<'runtime, S, CAPACITY>,
) -> DtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    match recurring.step() {
        DtmRecurringRunnerStep::Running(completion) => {
            DtmQuiescenceStep::Continue(runner(DtmQuiescencePhase::Completion(completion)))
        }
        DtmRecurringRunnerStep::Retryable(retry)
            if matches!(retry.cause(), DtmRecurringRetryCause::SchedulerStart(_)) =>
        {
            DtmQuiescenceStep::Retryable(runner(DtmQuiescencePhase::FinishPublishedRetry(retry)))
        }
        DtmRecurringRunnerStep::Fault(fault) => DtmQuiescenceStep::Fault(DtmQuiescenceFault {
            cause: DtmQuiescenceFaultCause::Recurring(fault.cause()),
            _owner: DtmQuiescenceFaultOwner::Recurring { _owner: fault },
        }),
        DtmRecurringRunnerStep::Continue(recurring) => {
            unexpected_published(DtmQuiescenceFaultOwner::PublishedRunner { _owner: recurring })
        }
        DtmRecurringRunnerStep::WaitControllerTime(wait) => {
            unexpected_published(DtmQuiescenceFaultOwner::PublishedWait { _owner: wait })
        }
        DtmRecurringRunnerStep::Retryable(retry) => {
            unexpected_published(DtmQuiescenceFaultOwner::PublishedRetry { _owner: retry })
        }
    }
}

fn unexpected_published<'runtime, S, const CAPACITY: usize>(
    owner: DtmQuiescenceFaultOwner<'runtime, S, CAPACITY>,
) -> DtmQuiescenceStep<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    DtmQuiescenceStep::Fault(DtmQuiescenceFault {
        cause: DtmQuiescenceFaultCause::UnexpectedPublishedHeadTransition,
        _owner: owner,
    })
}
