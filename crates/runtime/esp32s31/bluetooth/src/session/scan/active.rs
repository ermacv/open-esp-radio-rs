//! Finite Embassy drive for active and recurring ESP32-S31 passive scanning.

#![forbid(unsafe_code)]

use oer_esp32s31_bluetooth::{
    controller::SchedulerRunInterruptStorage,
    le::scanning::{
        PassiveScanHciActiveFault, PassiveScanHciActiveSession, PassiveScanHciActiveStep,
        PassiveScanHciRecurringFailure, PassiveScanHciRecurringRunner,
        PassiveScanHciRecurringRunnerStep, PassiveScanHciReportsPending,
    },
    scheduler::BluetoothSchedulerFinishedHardwareListObserved,
};

/// First externally meaningful result after driving every ready radio edge.
#[must_use = "retain the parked scanner, reports, unrelated list, or fail-stop owner"]
pub enum PassiveScanActiveDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Waiting(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Reports(PassiveScanHciReportsPending<'runtime, S, CAPACITY>),
    UnrelatedList {
        session: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
        observed: BluetoothSchedulerFinishedHardwareListObserved,
    },
    Fault(PassiveScanHciActiveFault<'runtime, S, CAPACITY>),
}

/// First externally meaningful result from recurring-window preparation.
#[must_use = "retain the wait, active scanner, retry, or fail-stop owner"]
pub enum PassiveScanRecurringDrive<'runtime, S, const CAPACITY: usize>
where
    S: SchedulerRunInterruptStorage,
{
    Wait(PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>),
    Active(PassiveScanHciActiveSession<'runtime, S, CAPACITY>),
    Failed(PassiveScanHciRecurringFailure<'runtime, S, CAPACITY>),
}

/// Run finite ready scanner-radio transitions; this function never waits.
pub fn drive_passive_scan_active_ready<'runtime, S, const CAPACITY: usize>(
    mut session: PassiveScanHciActiveSession<'runtime, S, CAPACITY>,
) -> PassiveScanActiveDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match session.step_radio() {
            PassiveScanHciActiveStep::Continue(next) => session = next,
            PassiveScanHciActiveStep::Waiting(session) => {
                return PassiveScanActiveDrive::Waiting(session);
            }
            PassiveScanHciActiveStep::UnrelatedList { session, observed } => {
                return PassiveScanActiveDrive::UnrelatedList { session, observed };
            }
            PassiveScanHciActiveStep::CpuOwned(reports) => {
                return PassiveScanActiveDrive::Reports(reports);
            }
            PassiveScanHciActiveStep::Fault(fault) => {
                return PassiveScanActiveDrive::Fault(fault);
            }
        }
    }
}

/// Run recurring-window preparation until time wait, `RUN`, or exact failure.
pub fn drive_passive_scan_recurring_ready<'runtime, S, const CAPACITY: usize>(
    mut runner: PassiveScanHciRecurringRunner<'runtime, S, CAPACITY>,
) -> PassiveScanRecurringDrive<'runtime, S, CAPACITY>
where
    S: SchedulerRunInterruptStorage,
{
    loop {
        match runner.step() {
            PassiveScanHciRecurringRunnerStep::Continue(next) => runner = next,
            PassiveScanHciRecurringRunnerStep::WaitControllerTime(runner) => {
                return PassiveScanRecurringDrive::Wait(runner);
            }
            PassiveScanHciRecurringRunnerStep::Running(active) => {
                return PassiveScanRecurringDrive::Active(active);
            }
            PassiveScanHciRecurringRunnerStep::Failed(failure) => {
                return PassiveScanRecurringDrive::Failed(failure);
            }
        }
    }
}
