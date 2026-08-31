//! Executor-neutral policy for the final Bluetooth hardware runner.

/// Semantic class of one complete command-actor boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandBoundaryClass {
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
        CommandBoundaryClass::IdleRestored => CommandBoundaryAction::Continue,
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
mod tests {
    use super::{
        CommandBoundaryAction, CommandBoundaryClass, HardwareRunnerSchedule,
        ModemTimerTransitionClass, modem_timer_requires_quarantine, reduce_command_boundary,
    };

    #[test]
    fn only_idle_completion_continues_and_retry_arms_a_gate() {
        assert_eq!(
            reduce_command_boundary(CommandBoundaryClass::IdleRestored),
            CommandBoundaryAction::Continue
        );
        assert_eq!(
            reduce_command_boundary(CommandBoundaryClass::Retryable),
            CommandBoundaryAction::GateRetry
        );
        assert_eq!(
            reduce_command_boundary(CommandBoundaryClass::UnownedFinishedList),
            CommandBoundaryAction::Quarantine
        );
        assert_eq!(
            reduce_command_boundary(CommandBoundaryClass::Terminal),
            CommandBoundaryAction::Quarantine
        );
    }

    #[test]
    fn source_127_policy_keeps_only_the_empty_rearm_path_live() {
        for class in [
            ModemTimerTransitionClass::BeginNotReady,
            ModemTimerTransitionClass::BeginStarted,
            ModemTimerTransitionClass::StepRecheck,
            ModemTimerTransitionClass::StepRearmPending,
            ModemTimerTransitionClass::Rearmed,
        ] {
            assert!(!modem_timer_requires_quarantine(class));
        }
        for class in [
            ModemTimerTransitionClass::BeginRejected,
            ModemTimerTransitionClass::StepUnsupported,
            ModemTimerTransitionClass::RearmRejected,
        ] {
            assert!(modem_timer_requires_quarantine(class));
        }
    }

    #[test]
    fn retry_gate_survives_rotation_until_a_completed_recheck() {
        let mut schedule = HardwareRunnerSchedule::new();
        assert!(!schedule.retry_gate());
        assert!(schedule.begin_iteration());
        assert!(!schedule.begin_iteration());

        schedule.arm_retry();
        assert!(schedule.retry_gate());
        assert!(schedule.begin_iteration());
        assert!(schedule.retry_gate());

        schedule.complete_recheck();
        assert!(!schedule.retry_gate());
        assert!(!schedule.begin_iteration());
    }
}
