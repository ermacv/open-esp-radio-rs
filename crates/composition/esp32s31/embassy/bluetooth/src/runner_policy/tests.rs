use super::{
    CommandBoundaryAction, CommandBoundaryClass, HardwareRunnerSchedule, ModemTimerTransitionClass,
    modem_timer_requires_quarantine, reduce_command_boundary,
};

#[test]
fn unrelated_progress_and_retry_cannot_finish_restoration() {
    for class in [
        CommandBoundaryClass::Progress,
        CommandBoundaryClass::Retryable,
        CommandBoundaryClass::Terminal,
        CommandBoundaryClass::UnownedFinishedList,
    ] {
        assert_ne!(
            reduce_command_boundary(class),
            CommandBoundaryAction::CompleteRestoration
        );
    }
    for class in [
        CommandBoundaryClass::PhyRestored,
        CommandBoundaryClass::IdleRestored,
    ] {
        assert_eq!(
            reduce_command_boundary(class),
            CommandBoundaryAction::CompleteRestoration
        );
    }
}

#[test]
fn terminal_maintenance_errors_use_the_same_immediate_disposition() {
    use super::{PhyMaintenanceError as Error, SharedPhyFailStop as Reason};
    for (error, reason) in [
        (Error::HardDeadline, Reason::MaintenanceHardDeadlineExceeded),
        (Error::Clock, Reason::MaintenanceFailed),
        (Error::TimelineOverflow, Reason::MaintenanceFailed),
        (Error::Restoration, Reason::MaintenanceFailed),
        (Error::OwnerUnavailable, Reason::MaintenanceFailed),
        (Error::LowerAdmission, Reason::MaintenanceFailed),
    ] {
        // The direct loop check and actor-boundary path share this reason.
        let direct = super::terminal_maintenance_reason(error);
        let boundary = reduce_command_boundary(CommandBoundaryClass::SharedPhyFailed(direct));
        assert_eq!(direct, reason);
        assert_eq!(boundary, CommandBoundaryAction::FailStopSharedPhy(reason));
    }
}

#[test]
fn restoration_expiry_escalates_without_waiting_for_the_next_phy_due_time() {
    let class = CommandBoundaryClass::SharedPhyFailed(super::terminal_maintenance_reason(
        super::PhyMaintenanceError::Restoration,
    ));
    // This disposition has no next-maintenance deadline: neither an absent nor
    // a newly refreshed schedule may turn failed restoration into a wait.
    assert_eq!(
        reduce_command_boundary(class),
        CommandBoundaryAction::FailStopSharedPhy(super::SharedPhyFailStop::MaintenanceFailed)
    );
}

#[test]
fn other_terminal_faults_do_not_become_shared_phy_failures() {
    for class in [
        CommandBoundaryClass::Terminal,
        CommandBoundaryClass::UnownedFinishedList,
    ] {
        assert_eq!(
            reduce_command_boundary(class),
            CommandBoundaryAction::Quarantine
        );
    }
}

#[test]
fn only_idle_completion_continues_and_retry_arms_a_gate() {
    assert_eq!(
        reduce_command_boundary(CommandBoundaryClass::Progress),
        CommandBoundaryAction::Continue
    );
    assert_eq!(
        reduce_command_boundary(CommandBoundaryClass::IdleRestored),
        CommandBoundaryAction::CompleteRestoration
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
