//! Cancellation-safe Embassy composition for the first legacy LE DTM event.

#![forbid(unsafe_code)]

use core::future::Future;

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothDtmFirstRunner, BluetoothDtmFirstRunnerCancel, BluetoothDtmFirstRunnerFailure,
    BluetoothDtmFirstRunnerStep, BluetoothDtmStartResponsePendingSession,
    BluetoothSchedulerRunInterruptStorage,
};

/// Executor disposition after driving every immediately available first-event step.
#[must_use = "retain the wait, response, or exact failed Controller owner"]
pub enum EmbassyBluetoothDtmFirstDrive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// Controller time must be rechecked after yielding to the executor.
    Wait(EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
    /// Hardware reached `RUN`; radio and pending-response axes now progress independently.
    Active(BluetoothDtmStartResponsePendingSession<'runtime, S, SCHEDULER_CAPACITY>),
    /// A finite core transition failed while retaining its owner.
    Failed(BluetoothDtmFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

/// Drive all finite non-waiting transitions to the next executor boundary.
///
/// The loop advances only monotonic `Continue` phases. A hardware-owned time
/// request, terminal failure or first scheduler `RUN` always returns control.
pub fn drive_dtm_first_ready<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
) -> EmbassyBluetoothDtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothDtmFirstRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothDtmFirstDrive::Wait(
                    EmbassyBluetoothDtmFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            BluetoothDtmFirstRunnerStep::Continue(next) => runner = next,
            BluetoothDtmFirstRunnerStep::Running(running) => {
                return EmbassyBluetoothDtmFirstDrive::Active(running.into_active_session());
            }
            BluetoothDtmFirstRunnerStep::Failed(failure) => {
                return EmbassyBluetoothDtmFirstDrive::Failed(failure);
            }
        }
    }
}

/// First-event runner parked outside any awaited future.
///
/// Keeping the affine runner in this synchronous object makes cancellation of
/// the caller-provided recheck future harmless.
#[must_use = "wait once, resume, or explicitly cancel the retained runner"]
pub struct EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothDtmFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    recheck_ready: bool,
}

/// Attempt to resume one exact parked controller-time owner.
#[must_use = "retain an owner whose caller-selected recheck future has not completed"]
pub enum EmbassyBluetoothDtmFirstResume<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    /// This exact wait observed its recheck future and advanced the runner.
    Ready(EmbassyBluetoothDtmFirstDrive<'runtime, S, SCHEDULER_CAPACITY>),
    /// No recheck opportunity completed for this exact wait object.
    NotReady(EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    EmbassyBluetoothDtmFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    pub fn resume(self) -> EmbassyBluetoothDtmFirstResume<'runtime, S, SCHEDULER_CAPACITY> {
        if self.recheck_ready {
            EmbassyBluetoothDtmFirstResume::Ready(drive_dtm_first_ready(self.runner))
        } else {
            EmbassyBluetoothDtmFirstResume::NotReady(self)
        }
    }

    /// Explicitly cancel without relying on dropping an affine hardware request.
    pub fn cancel(self) -> BluetoothDtmFirstRunnerCancel<'runtime, S, SCHEDULER_CAPACITY> {
        self.runner.cancel()
    }
}
