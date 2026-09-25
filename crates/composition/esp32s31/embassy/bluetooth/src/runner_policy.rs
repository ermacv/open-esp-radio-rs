//! Executor-neutral policy for the final Bluetooth hardware runner.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth::le::peripheral::PeripheralConnectionActiveFaultCause;
use oer_esp32s31_bluetooth_runtime::controller::maintenance::PhyMaintenanceError;
use oer_esp32s31_phy::tracking::fail_stop::SharedPhyFailStop;

/// Semantic class of one complete command-actor boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBoundaryClass {
    Progress,
    IdleRestored,
    PhyRestored,
    Retryable,
    UnownedFinishedList,
    Terminal,
    /// The retained owner cannot resume and requires shared-PHY escalation now.
    SharedPhyFailed(SharedPhyFailStop),
}

/// Required runner action for a command boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBoundaryAction {
    Continue,
    CompleteRestoration,
    GateRetry,
    Quarantine,
    FailStopSharedPhy(SharedPhyFailStop),
}

pub(crate) const fn reduce_command_boundary(class: CommandBoundaryClass) -> CommandBoundaryAction {
    match class {
        CommandBoundaryClass::Progress => CommandBoundaryAction::Continue,
        CommandBoundaryClass::IdleRestored | CommandBoundaryClass::PhyRestored => {
            CommandBoundaryAction::CompleteRestoration
        }
        CommandBoundaryClass::Retryable => CommandBoundaryAction::GateRetry,
        CommandBoundaryClass::UnownedFinishedList => CommandBoundaryAction::Quarantine,
        CommandBoundaryClass::Terminal => CommandBoundaryAction::Quarantine,
        CommandBoundaryClass::SharedPhyFailed(reason) => {
            CommandBoundaryAction::FailStopSharedPhy(reason)
        }
    }
}

/// Classify an already-terminal maintenance result, not a configuration or
/// admission rejection that returns a runnable owner to its caller.
pub(crate) const fn terminal_maintenance_reason(error: PhyMaintenanceError) -> SharedPhyFailStop {
    match error {
        PhyMaintenanceError::HardDeadline => SharedPhyFailStop::MaintenanceHardDeadlineExceeded,
        _ => SharedPhyFailStop::MaintenanceFailed,
    }
}

/// Restoration expiry belongs to the unfinished maintenance transaction, not
/// to a potentially refreshed next-maintenance schedule. Other active faults
/// retain their existing quarantine contract; they are not PHY failures merely
/// because the protocol owner is terminal.
#[cfg(target_arch = "riscv32")]
pub(crate) const fn classify_peripheral_fault(
    cause: PeripheralConnectionActiveFaultCause,
) -> CommandBoundaryClass {
    match cause {
        PeripheralConnectionActiveFaultCause::MaintenanceRestorationExpired => {
            CommandBoundaryClass::SharedPhyFailed(SharedPhyFailStop::MaintenanceFailed)
        }
        _ => CommandBoundaryClass::Terminal,
    }
}

/// Semantic class of one finite source-127 task result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ModemTimerTransitionClass {
    BeginNotReady,
    BeginStarted,
    BeginRejected,
    StepRecheck,
    StepRearmPending,
    StepUnsupported,
    Rearmed,
    RearmRejected,
}

pub(crate) const fn modem_timer_requires_quarantine(class: ModemTimerTransitionClass) -> bool {
    match class {
        ModemTimerTransitionClass::BeginNotReady
        | ModemTimerTransitionClass::BeginStarted
        | ModemTimerTransitionClass::StepRecheck
        | ModemTimerTransitionClass::StepRearmPending
        | ModemTimerTransitionClass::Rearmed => false,
        ModemTimerTransitionClass::BeginRejected
        | ModemTimerTransitionClass::StepUnsupported
        | ModemTimerTransitionClass::RearmRejected => true,
    }
}

/// Fair-selection and retry-gate state retained across executor cancellation.
pub(crate) struct HardwareRunnerSchedule {
    retry_gate: bool,
    primary_first: bool,
}

impl HardwareRunnerSchedule {
    pub(crate) const fn new() -> Self {
        Self {
            retry_gate: false,
            primary_first: true,
        }
    }

    pub(crate) const fn retry_gate(&self) -> bool {
        self.retry_gate
    }

    /// Choose this iteration's inner priority and rotate the next one.
    pub(crate) fn begin_iteration(&mut self) -> bool {
        let primary_first = self.primary_first;
        self.primary_first = !self.primary_first;
        primary_first
    }

    pub(crate) fn arm_retry(&mut self) {
        self.retry_gate = true;
    }

    pub(crate) fn complete_recheck(&mut self) {
        self.retry_gate = false;
    }
}

#[cfg(test)]
mod tests;
