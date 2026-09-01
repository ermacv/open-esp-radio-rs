//! Cancellation-safe Embassy drive for the first legacy advertising event.

#![forbid(unsafe_code)]

use core::future::Future;

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyAdvertisingFirstRunner, BluetoothLegacyAdvertisingFirstRunnerFailure,
    BluetoothLegacyAdvertisingFirstRunnerStep, BluetoothLegacyAdvertisingFirstRunning,
    BluetoothSchedulerRunInterruptStorage,
};

#[must_use = "retain the wait, running owner, or exact failure"]
pub enum EmbassyBluetoothLegacyAdvertisingFirstDrive<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Wait(EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>),
    Running(BluetoothLegacyAdvertisingFirstRunning<'runtime, S, SCHEDULER_CAPACITY>),
    Failed(BluetoothLegacyAdvertisingFirstRunnerFailure<'runtime, S, SCHEDULER_CAPACITY>),
}

pub fn drive_legacy_advertising_first_ready<'runtime, S, const SCHEDULER_CAPACITY: usize>(
    mut runner: BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
) -> EmbassyBluetoothLegacyAdvertisingFirstDrive<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothLegacyAdvertisingFirstRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothLegacyAdvertisingFirstDrive::Wait(
                    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            BluetoothLegacyAdvertisingFirstRunnerStep::Continue(next) => runner = next,
            BluetoothLegacyAdvertisingFirstRunnerStep::Running(running) => {
                return EmbassyBluetoothLegacyAdvertisingFirstDrive::Running(running);
            }
            BluetoothLegacyAdvertisingFirstRunnerStep::Failed(failure) => {
                return EmbassyBluetoothLegacyAdvertisingFirstDrive::Failed(failure);
            }
        }
    }
}

#[must_use = "wait once and resume or retain the exact advertising runner"]
pub struct EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait<
    'runtime,
    S,
    const SCHEDULER_CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothLegacyAdvertisingFirstRunner<'runtime, S, SCHEDULER_CAPACITY>,
    recheck_ready: bool,
}

#[must_use = "retain a wait whose caller-selected recheck has not completed"]
pub enum EmbassyBluetoothLegacyAdvertisingFirstResume<'runtime, S, const SCHEDULER_CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Ready(EmbassyBluetoothLegacyAdvertisingFirstDrive<'runtime, S, SCHEDULER_CAPACITY>),
    NotReady(
        EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>,
    ),
}

impl<'runtime, S, const SCHEDULER_CAPACITY: usize>
    EmbassyBluetoothLegacyAdvertisingFirstControllerTimeWait<'runtime, S, SCHEDULER_CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    pub async fn wait_for_recheck<R>(&mut self, recheck: R)
    where
        R: Future<Output = ()>,
    {
        recheck.await;
        self.recheck_ready = true;
    }

    pub fn resume(
        self,
    ) -> EmbassyBluetoothLegacyAdvertisingFirstResume<'runtime, S, SCHEDULER_CAPACITY> {
        if !self.recheck_ready {
            return EmbassyBluetoothLegacyAdvertisingFirstResume::NotReady(self);
        }
        EmbassyBluetoothLegacyAdvertisingFirstResume::Ready(drive_legacy_advertising_first_ready(
            self.runner,
        ))
    }
}
