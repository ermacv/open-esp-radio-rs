//! Executor-neutral policy for the final Bluetooth hardware runner.

/// Semantic class of one complete command-actor boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBoundaryClass {
    Progress,
    IdleRestored,
    Retryable,
    UnownedFinishedList,
    Terminal,
}

/// Required runner action for a command boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBoundaryAction {
    Continue,
    GateRetry,
    Quarantine,
}

pub(crate) const fn reduce_command_boundary(class: CommandBoundaryClass) -> CommandBoundaryAction {
    match class {
        CommandBoundaryClass::Progress | CommandBoundaryClass::IdleRestored => {
            CommandBoundaryAction::Continue
        }
        CommandBoundaryClass::Retryable => CommandBoundaryAction::GateRetry,
        CommandBoundaryClass::UnownedFinishedList => CommandBoundaryAction::Quarantine,
        CommandBoundaryClass::Terminal => CommandBoundaryAction::Quarantine,
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
