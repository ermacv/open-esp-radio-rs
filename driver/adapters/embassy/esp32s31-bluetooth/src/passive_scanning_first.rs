//! Cancellation-safe Embassy drive for the first passive scan window.

#![forbid(unsafe_code)]

use core::future::Future;

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothPassiveScanHciFirstRunner, BluetoothPassiveScanHciFirstRunnerFailure,
    BluetoothPassiveScanHciFirstRunnerStep, BluetoothPassiveScanHciFirstRunning,
    BluetoothSchedulerRunInterruptStorage,
};

#[must_use = "retain the wait, running owner, or exact failure"]
pub enum EmbassyBluetoothPassiveScanFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Wait(EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
    Running(BluetoothPassiveScanHciFirstRunning<'runtime, S, CAPACITY>),
    Failed(BluetoothPassiveScanHciFirstRunnerFailure<'runtime, S, CAPACITY>),
}

pub fn drive_passive_scan_first_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothPassiveScanFirstDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothPassiveScanHciFirstRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothPassiveScanFirstDrive::Wait(
                    EmbassyBluetoothPassiveScanFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            BluetoothPassiveScanHciFirstRunnerStep::Continue(next) => runner = next,
            BluetoothPassiveScanHciFirstRunnerStep::Running(running) => {
                return EmbassyBluetoothPassiveScanFirstDrive::Running(running);
            }
            BluetoothPassiveScanHciFirstRunnerStep::Failed(failure) => {
                return EmbassyBluetoothPassiveScanFirstDrive::Failed(failure);
            }
        }
    }
}

#[must_use = "wait once and resume or retain the exact scanner runner"]
pub struct EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothPassiveScanHciFirstRunner<'runtime, S, CAPACITY>,
    recheck_ready: bool,
}

#[must_use = "retain a wait whose recheck has not completed"]
pub enum EmbassyBluetoothPassiveScanFirstResume<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Ready(EmbassyBluetoothPassiveScanFirstDrive<'runtime, S, CAPACITY>),
    NotReady(EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>),
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothPassiveScanFirstControllerTimeWait<'runtime, S, CAPACITY>
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

    pub fn resume(self) -> EmbassyBluetoothPassiveScanFirstResume<'runtime, S, CAPACITY> {
        if !self.recheck_ready {
            return EmbassyBluetoothPassiveScanFirstResume::NotReady(self);
        }
        EmbassyBluetoothPassiveScanFirstResume::Ready(drive_passive_scan_first_ready(self.runner))
    }
}
