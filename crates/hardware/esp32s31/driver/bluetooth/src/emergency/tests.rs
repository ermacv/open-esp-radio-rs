use super::*;
use core::cell::Cell;

#[test]
fn admission_closes_before_routes_and_registers_wait_for_routes() {
    let gate = EmergencyGate::new();
    let masked = Cell::new(false);
    let mut stop = StopState::Armed;
    assert_eq!(
        stop.stop(
            &gate,
            || {
                assert!(gate.enter().is_err());
                masked.set(true);
            },
            |drained| {
                assert!(masked.get());
                assert!(drained.belongs_to(&gate));
                EmergencyOutcome::Stopped
            }
        ),
        EmergencyOutcome::Stopped
    );
}

#[test]
fn an_entered_operation_prevents_register_access_but_not_route_masking() {
    let gate = EmergencyGate::new();
    let entered = gate.enter().unwrap();
    let masked = Cell::new(false);
    let mut stop = StopState::Armed;
    assert_eq!(
        stop.stop(&gate, || masked.set(true), |_| panic!("no CPU exclusion")),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::EnteredOperation)
    );
    assert!(masked.get());
    drop(entered);
    // Leaving late cannot turn a quarantined installation into a fresh one.
    assert_eq!(
        stop.stop(
            &gate,
            || panic!("no second route transaction"),
            |_| panic!("no retry")
        ),
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::EnteredOperation)
    );
    assert!(gate.enter().is_err());
}

#[test]
fn both_scheduler_outcomes_quarantine_once_and_never_retry_registers() {
    for outcome in [
        EmergencyOutcome::Stopped,
        EmergencyOutcome::Unconfirmed(EmergencyUnconfirmed::SchedulerBusy),
    ] {
        let gate = EmergencyGate::new();
        let mut stop = StopState::Armed;
        assert_eq!(stop.stop(&gate, || {}, |_| outcome), outcome);
        assert_eq!(
            stop.stop(
                &gate,
                || panic!("no second mask"),
                |_| panic!("no second stop")
            ),
            outcome
        );
        assert!(gate.enter().is_err());
    }
}
