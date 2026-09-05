//! Cancellation-safe Embassy drive for the first connectable advertisement.

#![forbid(unsafe_code)]

use core::future::Future;

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyConnectableAdvertisingFirstRunner,
    BluetoothLegacyConnectableAdvertisingFirstRunnerFailure,
    BluetoothLegacyConnectableAdvertisingFirstRunnerStep,
    BluetoothLegacyConnectableAdvertisingFirstRunning, BluetoothSchedulerRunInterruptStorage,
};

/// Finite executor disposition of one ready connectable runner.
#[must_use = "retain the wait, running owner, or exact failure"]
#[expect(
    clippy::large_enum_variant,
    reason = "no-alloc variants retain the complete affine connectable owner"
)]
pub enum EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Wait(
        EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
    Running(BluetoothLegacyConnectableAdvertisingFirstRunning<'runtime, S, CAPACITY>),
    Failed(BluetoothLegacyConnectableAdvertisingFirstRunnerFailure<'runtime, S, CAPACITY>),
}

/// Drive only non-blocking lower transitions until an executor wait is needed.
pub fn drive_legacy_connectable_advertising_first_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothLegacyConnectableAdvertisingFirstRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Wait(
                    EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait {
                        runner,
                        recheck_ready: false,
                    },
                );
            }
            BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Continue(next) => runner = next,
            BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Running(running) => {
                return EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Running(running);
            }
            BluetoothLegacyConnectableAdvertisingFirstRunnerStep::Failed(failure) => {
                return EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive::Failed(failure);
            }
        }
    }
}

/// Borrow-safe wait retaining the complete affine runner in actor storage.
#[must_use = "wait once and resume or retain the exact connectable runner"]
pub struct EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait<
    'runtime,
    S,
    const CAPACITY: usize,
> where
    S: BluetoothSchedulerRunInterruptStorage,
{
    runner: BluetoothLegacyConnectableAdvertisingFirstRunner<'runtime, S, CAPACITY>,
    recheck_ready: bool,
}

/// Cancellation-safe resumption result.
#[must_use = "retain a wait whose caller-selected recheck has not completed"]
pub enum EmbassyBluetoothLegacyConnectableAdvertisingFirstResume<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Ready(EmbassyBluetoothLegacyConnectableAdvertisingFirstDrive<'runtime, S, CAPACITY>),
    NotReady(
        EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>,
    ),
}

impl<'runtime, S, const CAPACITY: usize>
    EmbassyBluetoothLegacyConnectableAdvertisingFirstControllerTimeWait<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
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
    pub fn resume(
        self,
    ) -> EmbassyBluetoothLegacyConnectableAdvertisingFirstResume<'runtime, S, CAPACITY> {
        if !self.recheck_ready {
            return EmbassyBluetoothLegacyConnectableAdvertisingFirstResume::NotReady(self);
        }
        EmbassyBluetoothLegacyConnectableAdvertisingFirstResume::Ready(
            drive_legacy_connectable_advertising_first_ready(self.runner),
        )
    }
}
