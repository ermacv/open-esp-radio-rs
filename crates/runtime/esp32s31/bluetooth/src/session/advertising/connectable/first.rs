//! Cancellation-safe Embassy drive for the first connectable advertisement.

#![forbid(unsafe_code)]

use core::future::Future;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyConnectableAdvertisingFirstRunner, LegacyConnectableAdvertisingFirstRunnerFailure,
        LegacyConnectableAdvertisingFirstRunnerStep, LegacyConnectableAdvertisingFirstRunning,
    },
};

/// Finite executor disposition of one ready connectable runner.
#[must_use = "retain the wait, running owner, or exact failure"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain the complete affine connectable owner"
)]
pub enum LegacyConnectableAdvertisingFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Wait(LegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>),
    Running(LegacyConnectableAdvertisingFirstRunning<'runtime, S, CAPACITY>),
    Failed(LegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>),
}

/// Drive only non-blocking lower transitions until an executor wait is needed.
pub fn drive_legacy_connectable_advertising_first_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: LegacyConnectableAdvertisingFirstRunner<'runtime, S, CAPACITY>,
) -> LegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            LegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(runner) => {
                return LegacyConnectableAdvertisingFirstDrive::Wait(
                    LegacyConnectableAdvertisingFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            LegacyConnectableAdvertisingFirstRunnerStep::Continue(next) => runner = next,
            LegacyConnectableAdvertisingFirstRunnerStep::Running(running) => {
                return LegacyConnectableAdvertisingFirstDrive::Running(running);
            }
            LegacyConnectableAdvertisingFirstRunnerStep::Failed(failure) => {
                return LegacyConnectableAdvertisingFirstDrive::Failed(failure);
            }
        }
    }
}

/// Borrow-safe wait retaining the complete affine runner in actor storage.
#[must_use = "wait once and resume or retain the exact connectable runner"]
pub struct LegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    runner: LegacyConnectableAdvertisingFirstRunner<'runtime, S, CAPACITY>,
    recheck_ready: bool,
}

/// Cancellation-safe resumption result.
#[must_use = "retain a wait whose caller-selected recheck has not completed"]
pub enum LegacyConnectableAdvertisingFirstResume<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Ready(LegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>),
    NotReady(LegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize>
    LegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    /// Await a caller-owned durable recheck source without moving the runner.
    pub async fn wait_for_recheck<R>(&mut self, recheck: R)
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        self.recheck_ready = true;
    }

    /// Resume only after the selected recheck future actually completed.
    pub fn resume(self) -> LegacyConnectableAdvertisingFirstResume<'runtime, S, CAPACITY> {
        if !self.recheck_ready {
            return LegacyConnectableAdvertisingFirstResume::NotReady(self);
        }
        LegacyConnectableAdvertisingFirstResume::Ready(
            drive_legacy_connectable_advertising_first_ready(self.runner),
        )
    }
}
