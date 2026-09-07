//! Cancellation-safe Embassy composition for the first legacy LE DTM event.

#![forbid(unsafe_code)]

use core::future::Future;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::dtm::{
        DtmFirstRunner, DtmFirstRunnerCancel, DtmFirstRunnerFailure, DtmFirstRunnerStep,
        DtmResponsePendingSession,
    },
};

/// Executor disposition after driving every immediately available first-event step.
#[must_use = "retain the wait, response, or exact failed Controller owner"]
pub enum DtmFirstDrive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// Controller time must be rechecked after yielding to the executor.
    Wait(DtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
    /// Hardware reached `RUN`; radio and pending-response axes now progress independently.
    Active(DtmResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite core transition failed while retaining its owner.
    Failed(DtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Drive all finite non-waiting transitions to the next executor boundary.
///
/// The loop advances only monotonic `Continue` phases. A hardware-owned time
/// request, terminal failure or first scheduler `RUN` always returns control.
pub fn drive_dtm_first_ready<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut runner: DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
) -> DtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            DtmFirstRunnerStep::WaitControllerTime(runner) => {
                return DtmFirstDrive::Wait(DtmFirstControllerTimeWait {
                    runner,
                    recheck_ready: false,
                });
            }
            DtmFirstRunnerStep::Continue(next) => runner = next,
            DtmFirstRunnerStep::Running(running) => {
                return DtmFirstDrive::Active(running.into_active_session());
            }
            DtmFirstRunnerStep::Failed(failure) => {
                return DtmFirstDrive::Failed(failure);
            }
        }
    }
}

/// First-event runner parked outside any awaited future.
///
/// Keeping the affine runner in this synchronous object makes cancellation of
/// the caller-provided recheck future harmless.
#[must_use = "wait once, resume, or explicitly cancel the retained runner"]
pub struct DtmFirstControllerTimeWait<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: DtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    recheck_ready: bool,
}

/// Attempt to resume one exact parked controller-time owner.
#[must_use = "retain an owner whose caller-selected recheck future has not completed"]
pub enum DtmFirstResume<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    /// This exact wait observed its recheck future and advanced the runner.
    Ready(DtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>),
    /// No recheck opportunity completed for this exact wait object.
    NotReady(DtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    DtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Await one caller-selected cooperative recheck opportunity.
    ///
    /// Completion proves only that another bounded MMIO observation may run;
    /// it does not claim causal association with an unproven interrupt source.
    pub async fn wait_for_recheck<R>(&mut self, recheck: R)
    where
        R: Future<Output = ()>,
    {
        let _retained_owner = &self.runner;
        recheck.await;
        self.recheck_ready = true;
    }

    /// Consume the parked owner and drive from its next bounded observation.
    pub fn resume(self) -> DtmFirstResume<'runtime, S, SCHEDULER_CAPACITY> {
        if self.recheck_ready {
            DtmFirstResume::Ready(drive_dtm_first_ready(self.runner))
        } else {
            DtmFirstResume::NotReady(self)
        }
    }

    /// Explicitly cancel without relying on dropping an affine hardware request.
    pub fn cancel(self) -> DtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner.cancel()
    }
}
