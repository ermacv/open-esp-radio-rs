//! Pure established-link decision for an unsubmitted recurring event window.

use crate::SchedulerInstant;
use oer_bluetooth_ll::connection::{LePeripheralConnectionEventDelta, LePeripheralConnectionState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeripheralMissedAnchorDecision {
    Run,
    EstablishmentPolicyRequired,
    Skip(LePeripheralConnectionEventDelta),
    DeltaUnavailable,
    MaintenanceWindowMissed,
}

/// Select a later event when executor latency has closed the guarded start.
///
/// `attempted_delta` is measured from the same completed event retained by the
/// provisional scheduler transaction, so the returned delta remains suitable
/// for cancelling and rebuilding that exact transaction.
pub(crate) const fn decide_missed_anchor(
    state: LePeripheralConnectionState,
    now: SchedulerInstant,
    event_start: SchedulerInstant,
    late_start_guard_micros: u32,
    interval_micros: u32,
    attempted_delta: LePeripheralConnectionEventDelta,
    restoring_maintenance: bool,
) -> PeripheralMissedAnchorDecision {
    let guarded_now = now.wrapping_add(late_start_guard_micros);
    if guarded_now.is_before(event_start) {
        return PeripheralMissedAnchorDecision::Run;
    }
    if restoring_maintenance {
        return PeripheralMissedAnchorDecision::MaintenanceWindowMissed;
    }
    if matches!(state, LePeripheralConnectionState::Created) {
        return PeripheralMissedAnchorDecision::EstablishmentPolicyRequired;
    }
    let elapsed = guarded_now.image().wrapping_sub(event_start.image());
    if elapsed > i32::MAX as u32 || interval_micros == 0 {
        return PeripheralMissedAnchorDecision::DeltaUnavailable;
    }
    let additional = elapsed / interval_micros + 1;
    if additional > u16::MAX as u32 {
        return PeripheralMissedAnchorDecision::DeltaUnavailable;
    }
    let additional = additional as u16;
    let Some(delta) = attempted_delta.get().checked_add(additional) else {
        return PeripheralMissedAnchorDecision::DeltaUnavailable;
    };
    match LePeripheralConnectionEventDelta::new(delta) {
        Some(delta) => PeripheralMissedAnchorDecision::Skip(delta),
        None => PeripheralMissedAnchorDecision::DeltaUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u32) -> SchedulerInstant {
        SchedulerInstant::from_image(micros)
    }

    const ESTABLISHED: LePeripheralConnectionState = LePeripheralConnectionState::Established {
        establishment_event_counter: 0,
        supervision_anchor_event_counter: 7,
    };

    fn delta(value: u16) -> LePeripheralConnectionEventDelta {
        LePeripheralConnectionEventDelta::new(value).unwrap()
    }

    #[test]
    fn open_guarded_window_runs_without_changing_delta() {
        assert_eq!(
            decide_missed_anchor(ESTABLISHED, at(900), at(1_000), 40, 30_000, delta(1), false),
            PeripheralMissedAnchorDecision::Run
        );
    }

    #[test]
    fn closed_established_window_jumps_over_all_elapsed_intervals() {
        assert_eq!(
            decide_missed_anchor(
                ESTABLISHED,
                at(61_000),
                at(1_000),
                40,
                30_000,
                delta(1),
                false
            ),
            PeripheralMissedAnchorDecision::Skip(delta(4))
        );
    }

    #[test]
    fn recovery_accumulates_from_the_cancelled_candidate_delta() {
        assert_eq!(
            decide_missed_anchor(
                ESTABLISHED,
                at(31_000),
                at(1_000),
                40,
                30_000,
                delta(4),
                false
            ),
            PeripheralMissedAnchorDecision::Skip(delta(6))
        );
    }

    #[test]
    fn wrapping_positions_recover_without_aliasing_an_old_window() {
        assert_eq!(
            decide_missed_anchor(
                ESTABLISHED,
                at(20_000),
                at(u32::MAX - 9_999),
                40,
                30_000,
                delta(1),
                false
            ),
            PeripheralMissedAnchorDecision::Skip(delta(3))
        );
    }

    #[test]
    fn created_link_exposes_the_unresolved_establishment_policy() {
        assert_eq!(
            decide_missed_anchor(
                LePeripheralConnectionState::Created,
                at(1_000),
                at(1_000),
                40,
                30_000,
                delta(1),
                false
            ),
            PeripheralMissedAnchorDecision::EstablishmentPolicyRequired
        );
    }

    #[test]
    fn maintenance_successor_never_uses_late_recovery_or_establishment_fallback() {
        for state in [ESTABLISHED, LePeripheralConnectionState::Created] {
            for start in [1000, u32::MAX - 10] {
                let start = at(start);
                assert_eq!(
                    decide_missed_anchor(
                        state,
                        start.wrapping_add(u32::MAX - 100),
                        start,
                        40,
                        30_000,
                        delta(2),
                        true
                    ),
                    PeripheralMissedAnchorDecision::Run
                );
                for late in [0, 1, 60_000] {
                    assert_eq!(
                        decide_missed_anchor(
                            state,
                            start.wrapping_add(late),
                            start,
                            40,
                            30_000,
                            delta(2),
                            true
                        ),
                        PeripheralMissedAnchorDecision::MaintenanceWindowMissed
                    );
                }
            }
        }
    }
}
