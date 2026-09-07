//! Cancellation-safe Embassy drive for the first passive scan window.

#![forbid(unsafe_code)]

use core::future::Future;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::scanning::{
        PassiveScanHciFirstRunner, PassiveScanHciFirstRunnerFailure, PassiveScanHciFirstRunnerStep,
        PassiveScanHciFirstRunning,
    },
};

#[must_use = "retain the wait, running owner, or exact failure"]
pub enum PassiveScanFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Wait(PassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
    Running(PassiveScanHciFirstRunning<'runtime, S, CAPACITY>),
    Failed(PassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
}

pub fn drive_passive_scan_first_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: PassiveScanHciFirstRunner<'runtime, S, CAPACITY>,
) -> PassiveScanFirstDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            PassiveScanHciFirstRunnerStep::WaitControllerTime(runner) => {
                return PassiveScanFirstDrive::Wait(PassiveScanFirstControllerTimeWait {
                    runner,
                    recheck_ready: false,
                });
            }
            PassiveScanHciFirstRunnerStep::Continue(next) => runner = next,
            PassiveScanHciFirstRunnerStep::Running(running) => {
                return PassiveScanFirstDrive::Running(running);
            }
            PassiveScanHciFirstRunnerStep::Failed(failure) => {
                return PassiveScanFirstDrive::Failed(failure);
            }
        }
    }
}

#[must_use = "wait once and resume or retain the exact scanner runner"]
pub struct PassiveScanFirstControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: PassiveScanHciFirstRunner<'runtime, S, CAPACITY>,
    recheck_ready: bool,
}

#[must_use = "retain a wait whose recheck has not completed"]
pub enum PassiveScanFirstResume<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Ready(PassiveScanFirstDrive<'runtime, S, CAPACITY>),
    NotReady(PassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize> PassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    pub async fn wait_for_recheck<R>(&mut self, recheck: R)
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        self.recheck_ready = true;
    }

    pub fn resume(self) -> PassiveScanFirstResume<'runtime, S, CAPACITY> {
        if !self.recheck_ready {
            return PassiveScanFirstResume::NotReady(self);
        }
        PassiveScanFirstResume::Ready(drive_passive_scan_first_ready(self.runner))
    }
}
