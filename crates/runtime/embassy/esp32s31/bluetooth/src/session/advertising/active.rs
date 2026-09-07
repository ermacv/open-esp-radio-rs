//! Finite drive for the ESP32-S31 active legacy-advertising radio axis.

#![forbid(unsafe_code)]

use oer_bluetooth_ll::advertising::AdvertisingDelay;

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::advertising::{
        LegacyAdvertisingActiveFault, LegacyAdvertisingActiveResponsePending,
        LegacyAdvertisingActiveSession, LegacyAdvertisingActiveStep,
        LegacyAdvertisingEventCpuOwned, LegacyAdvertisingRecurringFault,
        LegacyAdvertisingRecurringRetry, LegacyAdvertisingRecurringRunner,
        LegacyAdvertisingRecurringRunnerStep, LegacyAdvertisingStopping,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

/// Source-owned entropy policy for the Link Layer's fresh 0..=10 ms delay.
pub trait LegacyAdvertisingDelaySource {
    fn next_advertising_delay(&mut self) -> AdvertisingDelay;
}

/// First externally meaningful result after driving every immediately ready edge.
#[must_use = "retain the parked, completed, unrelated-list, or fail-stop owner"]
pub enum LegacyAdvertisingActiveDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Waiting(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    CpuOwned(LegacyAdvertisingEventCpuOwned<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Fault(LegacyAdvertisingActiveFault<'runtime, S, CAPACITY>),
}

/// First externally meaningful recurring-runner result.
#[must_use = "retain the wait, active session, retry, or fail-stop owner"]
pub enum LegacyAdvertisingRecurringDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Wait(LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>),
    Active(LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>),
    ActiveResponsePending(LegacyAdvertisingActiveResponsePending<'runtime, S, CAPACITY>),
    Stopping(LegacyAdvertisingStopping<'runtime, S, CAPACITY>),
    Retryable(LegacyAdvertisingRecurringRetry<'runtime, S, CAPACITY>),
    Fault(LegacyAdvertisingRecurringFault<'runtime, S, CAPACITY>),
}

/// Run finite successor preparation until controller time, `RUN`, or failure.
pub fn drive_legacy_advertising_recurring_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: LegacyAdvertisingRecurringRunner<'runtime, S, CAPACITY>,
) -> LegacyAdvertisingRecurringDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            LegacyAdvertisingRecurringRunnerStep::Continue(next) => runner = next,
            LegacyAdvertisingRecurringRunnerStep::WaitControllerTime(runner) => {
                return LegacyAdvertisingRecurringDrive::Wait(runner);
            }
            LegacyAdvertisingRecurringRunnerStep::Running(active) => {
                return LegacyAdvertisingRecurringDrive::Active(active);
            }
            LegacyAdvertisingRecurringRunnerStep::RunningResponsePending(pending) => {
                return LegacyAdvertisingRecurringDrive::ActiveResponsePending(pending);
            }
            LegacyAdvertisingRecurringRunnerStep::RunningStopping(stopping) => {
                return LegacyAdvertisingRecurringDrive::Stopping(stopping);
            }
            LegacyAdvertisingRecurringRunnerStep::Retryable(retry) => {
                return LegacyAdvertisingRecurringDrive::Retryable(retry);
            }
            LegacyAdvertisingRecurringRunnerStep::Fault(fault) => {
                return LegacyAdvertisingRecurringDrive::Fault(fault);
            }
        }
    }
}

/// Run only finite ready transitions; this function never polls or waits.
pub fn drive_legacy_advertising_active_ready<'runtime, S, const CAPACITY: usize>(
    mut session: LegacyAdvertisingActiveSession<'runtime, S, CAPACITY>,
) -> LegacyAdvertisingActiveDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match session.step_radio() {
            LegacyAdvertisingActiveStep::Continue(next) => session = next,
            LegacyAdvertisingActiveStep::Waiting(session) => {
                return LegacyAdvertisingActiveDrive::Waiting(session);
            }
            LegacyAdvertisingActiveStep::UnrelatedList { session, observed } => {
                return LegacyAdvertisingActiveDrive::UnrelatedList { session, observed };
            }
            LegacyAdvertisingActiveStep::CpuOwned(owner) => {
                return LegacyAdvertisingActiveDrive::CpuOwned(owner);
            }
            LegacyAdvertisingActiveStep::Fault(fault) => {
                return LegacyAdvertisingActiveDrive::Fault(fault);
            }
        }
    }
}
