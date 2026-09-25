//! Cancellation-safe Embassy drive for the first legacy advertising event.

#![forbid(unsafe_code)]

use core::future::Future;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyAdvertisingFirstRunner, LegacyAdvertisingFirstRunnerFailure,
        LegacyAdvertisingFirstRunnerStep, LegacyAdvertisingFirstRunning,
    },
};

#[must_use = "retain the wait, running owner, or exact failure"]
pub enum LegacyAdvertisingFirstDrive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Wait(LegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
    Running(LegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(LegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

pub fn drive_legacy_advertising_first_ready<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut runner: LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
) -> LegacyAdvertisingFirstDrive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            LegacyAdvertisingFirstRunnerStep::WaitControllerTime(runner) => {
                return LegacyAdvertisingFirstDrive::Wait(
                    LegacyAdvertisingFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            LegacyAdvertisingFirstRunnerStep::Continue(next) => runner = next,
            LegacyAdvertisingFirstRunnerStep::Running(running) => {
                return LegacyAdvertisingFirstDrive::Running(running);
            }
            LegacyAdvertisingFirstRunnerStep::Failed(failure) => {
                return LegacyAdvertisingFirstDrive::Failed(failure);
            }
        }
    }
}

#[must_use = "wait once and resume or retain the exact advertising runner"]
pub struct LegacyAdvertisingFirstControllerTimeWait<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: LegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    recheck_ready: bool,
}

#[must_use = "retain a wait whose caller-selected recheck has not completed"]
pub enum LegacyAdvertisingFirstResume<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Ready(LegacyAdvertisingFirstDrive<'runtime, S, SCHEDULER_CAPACITY>),
    NotReady(LegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    LegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>
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

    pub fn resume(self) -> LegacyAdvertisingFirstResume<'runtime, S, SCHEDULER_CAPACITY> {
        if !self.recheck_ready {
            return LegacyAdvertisingFirstResume::NotReady(self);
        }
        LegacyAdvertisingFirstResume::Ready(drive_legacy_advertising_first_ready(self.runner))
    }
}
