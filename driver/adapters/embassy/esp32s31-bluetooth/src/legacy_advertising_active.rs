//! Finite drive for the ESP32-S31 active legacy-advertising radio axis.

#![forbid(unsafe_code)]

use open_esp_radio_bluetooth_ll::advertising::AdvertisingDelay;
use open_esp_radio_esp32s31_bluetooth::{
    BluetoothLegacyAdvertisingActiveFault, BluetoothLegacyAdvertisingActiveResponsePending,
    BluetoothLegacyAdvertisingActiveSession, BluetoothLegacyAdvertisingActiveStep,
    BluetoothLegacyAdvertisingEventCpuOwned, BluetoothLegacyAdvertisingRecurringFault,
    BluetoothLegacyAdvertisingRecurringRetry, BluetoothLegacyAdvertisingRecurringRunner,
    BluetoothLegacyAdvertisingRecurringRunnerStep, BluetoothLegacyAdvertisingStopping,
    BluetoothSchedulerFinishedHardwareListObserved, BluetoothSchedulerRunInterruptStorage,
};

/// Source-owned entropy policy for the Link Layer's fresh 0..=10 ms delay.
pub trait EmbassyBluetoothLegacyAdvertisingDelaySource {
    fn next_advertising_delay(&mut self) -> AdvertisingDelay;
}

/// First externally meaningful result after driving every immediately ready edge.
#[must_use = "retain the parked, completed, unrelated-list, or fail-stop owner"]
pub enum EmbassyBluetoothLegacyAdvertisingActiveDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Waiting(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    CpuOwned(BluetoothLegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Fault(BluetoothLegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
}

/// First externally meaningful recurring-runner result.
#[must_use = "retain the wait, active session, retry, or fail-stop owner"]
pub enum EmbassyBluetoothLegacyAdvertisingRecurringDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Wait(BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Active(BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    ActiveResponsePending(BluetoothLegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(BluetoothLegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Retryable(BluetoothLegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    Fault(BluetoothLegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// Run finite successor preparation until controller time, `RUN`, or failure.
pub fn drive_legacy_advertising_recurring_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: BluetoothLegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyAdvertisingRecurringDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothLegacyAdvertisingRecurringRunnerStep::Continue(next) => runner = next,
            BluetoothLegacyAdvertisingRecurringRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::Wait(runner);
            }
            BluetoothLegacyAdvertisingRecurringRunnerStep::Running(active) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::Active(active);
            }
            BluetoothLegacyAdvertisingRecurringRunnerStep::RunningResponsePending(pending) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::ActiveResponsePending(
                    pending,
                );
            }
            BluetoothLegacyAdvertisingRecurringRunnerStep::RunningStopping(stopping) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::Stopping(stopping);
            }
            BluetoothLegacyAdvertisingRecurringRunnerStep::Retryable(retry) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::Retryable(retry);
            }
            BluetoothLegacyAdvertisingRecurringRunnerStep::Fault(fault) => {
                return EmbassyBluetoothLegacyAdvertisingRecurringDrive::Fault(fault);
            }
        }
    }
}

/// Run only finite ready transitions; this function never polls or waits.
pub fn drive_legacy_advertising_active_ready<'runtime, S, const CAPACITY: usize>(
    mut session: BluetoothLegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothLegacyAdvertisingActiveDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match session.step_radio() {
            BluetoothLegacyAdvertisingActiveStep::Continue(next) => session = next,
            BluetoothLegacyAdvertisingActiveStep::Waiting(session) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::Waiting(session);
            }
            BluetoothLegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::UnrelatedList {
                    session,
                    observed,
                };
            }
            BluetoothLegacyAdvertisingActiveStep::CpuOwned(owner) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::CpuOwned(owner);
            }
            BluetoothLegacyAdvertisingActiveStep::Fault(fault) => {
                return EmbassyBluetoothLegacyAdvertisingActiveDrive::Fault(fault);
            }
        }
    }
}
