use super::*;

const PHASES: [CoexPhase; 2] = [
    CoexPhase::from_reviewed_image([1, 2, 3, 4]),
    CoexPhase::from_reviewed_image([5, 6, 7, 8]),
];
const SCHEDULE: CoexSchedule<'static> = CoexSchedule::new(9, &PHASES);

#[test]
fn interval_is_source_owned_without_mmio_or_rtos_state() {
    let mut scheduler = CoexScheduler::new();
    assert_eq!(scheduler.interval(), 0);
    scheduler.set_interval(0xa5a5_5a5a);
    assert_eq!(scheduler.interval(), 0xa5a5_5a5a);
}

#[test]
fn inactive_schedule_uses_vendor_defaults() {
    let scheduler = CoexScheduler::new();
    assert_eq!(scheduler.current_period(), 1);
    assert_eq!(scheduler.current_phase(), None);
}

#[test]
fn active_period_and_phase_are_bounded_by_the_typed_slice() {
    let mut scheduler = CoexScheduler::new();
    scheduler.activate(&SCHEDULE);
    assert_eq!(scheduler.current_period(), 9);
    assert_eq!(scheduler.current_phase(), Some(&PHASES[0]));

    scheduler.set_phase_index(1);
    assert_eq!(scheduler.current_phase(), Some(&PHASES[1]));

    scheduler.set_phase_index(2);
    assert_eq!(scheduler.current_phase(), None);
}

#[test]
fn deactivation_drops_schedule_and_phase_index_together() {
    let mut scheduler = CoexScheduler::new();
    scheduler.activate(&SCHEDULE);
    scheduler.set_phase_index(1);
    scheduler.deactivate();
    assert_eq!(scheduler.phase_index(), 0);
    assert_eq!(scheduler.current_period(), 1);
    assert_eq!(scheduler.current_phase(), None);
}
