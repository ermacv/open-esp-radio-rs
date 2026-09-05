use super::*;

#[test]
fn reference_restores_only_the_final_retained_baseline() {
    let mut state = SharedModemClockState::new();
    assert!(state.retain(Requirement::Coexistence, false));
    assert!(!state.retain(Requirement::Coexistence, true));
    assert_eq!(state.release(Requirement::Coexistence), None);
    assert_eq!(state.release(Requirement::Coexistence), Some(false));

    assert!(!state.retain(Requirement::Coexistence, true));
    assert_eq!(state.release(Requirement::Coexistence), Some(true));
}
