//! Finite Embassy drive for active and recurring ESP32-S31 passive scanning.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_bluetooth::{
    BluetoothPassiveScanHciActiveFault, BluetoothPassiveScanHciActiveSession,
    BluetoothPassiveScanHciActiveStep, BluetoothPassiveScanHciRecurringFailure,
    BluetoothPassiveScanHciRecurringRunner, BluetoothPassiveScanHciRecurringRunnerStep,
    BluetoothPassiveScanHciReportsPending, BluetoothSchedulerFinishedHardwareListObserved,
    BluetoothSchedulerRunInterruptStorage,
};

/// First externally meaningful result after driving every ready radio edge.
#[must_use = "retain the parked scanner, reports, unrelated list, or fail-stop owner"]
pub enum EmbassyBluetoothPassiveScanActiveDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Waiting(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Reports(BluetoothPassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Fault(BluetoothPassiveScanHciActiveFault<'runtime, S, CAPACITY>),
}

/// First externally meaningful result from recurring-window preparation.
#[must_use = "retain the wait, active scanner, retry, or fail-stop owner"]
pub enum EmbassyBluetoothPassiveScanRecurringDrive<'runtime, S, const CAPACITY: usize>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    Wait(BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Active(BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Failed(BluetoothPassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
}

/// Run finite ready scanner-radio transitions; this function never waits.
pub fn drive_passive_scan_active_ready<'runtime, S, const CAPACITY: usize>(
    mut session: BluetoothPassiveScanHciActiveSession<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothPassiveScanActiveDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match session.step_radio() {
            BluetoothPassiveScanHciActiveStep::Continue(next) => session = next,
            BluetoothPassiveScanHciActiveStep::Waiting(session) => {
                return EmbassyBluetoothPassiveScanActiveDrive::Waiting(session);
            }
            BluetoothPassiveScanHciActiveStep::UnrelatedList { session, observed } => {
                return EmbassyBluetoothPassiveScanActiveDrive::UnrelatedList { session, observed };
            }
            BluetoothPassiveScanHciActiveStep::CpuOwned(reports) => {
                return EmbassyBluetoothPassiveScanActiveDrive::Reports(reports);
            }
            BluetoothPassiveScanHciActiveStep::Fault(fault) => {
                return EmbassyBluetoothPassiveScanActiveDrive::Fault(fault);
            }
        }
    }
}

/// Run recurring-window preparation until time wait, `RUN`, or exact failure.
pub fn drive_passive_scan_recurring_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: BluetoothPassiveScanHciRecurringRunner<'runtime, S, CAPACITY>,
) -> EmbassyBluetoothPassiveScanRecurringDrive<'runtime, S, CAPACITY>
where
    S: BluetoothSchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            BluetoothPassiveScanHciRecurringRunnerStep::Continue(next) => runner = next,
            BluetoothPassiveScanHciRecurringRunnerStep::WaitControllerTime(runner) => {
                return EmbassyBluetoothPassiveScanRecurringDrive::Wait(runner);
            }
            BluetoothPassiveScanHciRecurringRunnerStep::Running(active) => {
                return EmbassyBluetoothPassiveScanRecurringDrive::Active(active);
            }
            BluetoothPassiveScanHciRecurringRunnerStep::Failed(failure) => {
                return EmbassyBluetoothPassiveScanRecurringDrive::Failed(failure);
            }
        }
    }
}
