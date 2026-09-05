use open_esp_radio_esp32s31_pac::{BluetoothControllerHalInitConfig, BluetoothControllerTimeScale};

use super::{
    BluetoothSchedulerInitialAdmissionResolved, BluetoothSchedulerRawWindow,
    BluetoothSchedulerRecurringReserved, BluetoothSchedulerReservationError,
    BluetoothSchedulerReservationReleaseError, BluetoothSchedulerTimeline,
    BluetoothSchedulerTimelineSlot, BluetoothSchedulerTimingPolicy,
    BluetoothSchedulerWindowReservation,
};
use crate::{BluetoothControllerTimeSample, BluetoothSchedulerSoftwareConfig};

fn scale() -> BluetoothControllerTimeScale {
    BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
}

fn timing_policy() -> BluetoothSchedulerTimingPolicy {
    BluetoothSchedulerTimingPolicy::from_scheduler_config(
        BluetoothSchedulerSoftwareConfig::reviewed_standalone(),
        scale(),
    )
}

fn sample(raw_time: u32) -> BluetoothControllerTimeSample {
    BluetoothControllerTimeSample::for_validation(raw_time)
}

fn reserve_raw<const CAPACITY: usize>(
    timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
    start: u32,
    end: u32,
) -> Result<
    BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
    BluetoothSchedulerReservationError,
> {
    timeline.reserve_initial_raw_window(start, end, timing_policy())
}

fn reserve_recurring_raw<const CAPACITY: usize>(
    timeline: &mut BluetoothSchedulerTimeline<CAPACITY>,
    start: u32,
    end: u32,
) -> Result<
    BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>,
    BluetoothSchedulerReservationError,
> {
    timeline.reserve_recurring_window(start, end, timing_policy())
}

#[test]
fn insertion_policy_uses_one_initialized_scheduler_epoch_scale() {
    let policy = timing_policy();

    assert_eq!(policy.sequence_lead_raw_delta(), 11);
    assert!(policy.initial_deadline_is_open(sample(92), 103));
    assert!(!policy.initial_deadline_is_open(sample(93), 103));
}

#[test]
fn non_overlapping_window_keeps_its_position() {
    let mut timeline = BluetoothSchedulerTimeline::<2>::new();
    let reservation = reserve_raw(&mut timeline, 30, 40).expect("window is admissible");

    assert_eq!(reservation.window().start(), 30);
    assert_eq!(reservation.window().end(), 40);
    assert_eq!(reservation.window().duration(), 10);
}

#[test]
fn empty_window_is_not_a_scheduler_candidate() {
    assert!(BluetoothSchedulerRawWindow::from_projected_scheduler_window(30, 30).is_none());
}

#[test]
fn overlap_chain_delays_once_per_occupied_interval_and_preserves_duration() {
    let mut timeline = BluetoothSchedulerTimeline::<3>::new();
    let later = reserve_raw(&mut timeline, 40, 50).expect("later window is admissible");
    let _earlier = reserve_raw(&mut timeline, 20, 30).expect("earlier window is admissible");
    let delayed = reserve_raw(&mut timeline, 25, 45).expect("overlaps are resolvable");

    assert_eq!(delayed.window().start(), 50);
    assert_eq!(delayed.window().end(), 70);
    assert_eq!(delayed.window().duration(), 20);
    assert_eq!(later.window().end(), delayed.window().start());
}

#[test]
fn recurring_collision_rejects_without_displacing_or_mutating_the_timeline() {
    let mut timeline = BluetoothSchedulerTimeline::<3>::new();
    let occupied = reserve_raw(&mut timeline, 20, 30).expect("first window is admissible");
    let delayed = reserve_raw(&mut timeline, 25, 35)
        .expect("initial overlap follows the bounded displacement path");

    assert_eq!(delayed.window().start(), 30);
    assert_eq!(delayed.window().end(), 40);
    assert!(matches!(
        reserve_recurring_raw(&mut timeline, 25, 35),
        Err(BluetoothSchedulerReservationError::RecurringOverlapUnsupported)
    ));
    assert_eq!(occupied.window().start(), 20);
    assert_eq!(occupied.window().end(), 30);
    assert_eq!(delayed.window().start(), 30);
    assert_eq!(delayed.window().end(), 40);

    assert!(timeline.release(delayed).is_ok());
    assert!(timeline.release(occupied).is_ok());
    let recurring = reserve_recurring_raw(&mut timeline, 25, 35)
        .expect("the rejected exact candidate did not occupy or move a slot");
    assert_eq!(recurring.window().start(), 25);
    assert_eq!(recurring.window().end(), 35);
    assert!(timeline.release(recurring).is_ok());
    assert!(timeline.is_empty());
}

#[test]
fn phase_locked_initial_window_rejects_overlap_without_moving_scan_phase() {
    let mut timeline = BluetoothSchedulerTimeline::<2>::new();
    let occupied = reserve_raw(&mut timeline, 200, 300).expect("first window is admissible");

    assert!(matches!(
        timeline.reserve_phase_locked_initial_window(250, 350, timing_policy(), sample(100)),
        Err(BluetoothSchedulerReservationError::RecurringOverlapUnsupported)
    ));
    assert_eq!(occupied.window().start(), 200);
    assert_eq!(occupied.window().end(), 300);
    assert!(timeline.release(occupied).is_ok());
    assert!(timeline.is_empty());
}

#[test]
fn touching_boundaries_do_not_overlap() {
    let mut timeline = BluetoothSchedulerTimeline::<2>::new();
    let first = reserve_raw(&mut timeline, 20, 30).expect("first window is admissible");
    let second = reserve_raw(&mut timeline, 30, 40).expect("touching window is admissible");

    assert_eq!(first.window().end(), second.window().start());
}

#[test]
fn overlap_resolution_preserves_wrapping_time_semantics() {
    let mut timeline = BluetoothSchedulerTimeline::<2>::new();
    let occupied = reserve_raw(&mut timeline, u32::MAX - 15, u32::MAX - 5)
        .expect("occupied window is ahead across the wrapping epoch");
    let delayed = reserve_raw(&mut timeline, u32::MAX - 19, u32::MAX - 10)
        .expect("wrapping overlap is resolvable");

    assert_eq!(delayed.window().start(), occupied.window().end());
    assert_eq!(delayed.window().end(), 3);
    assert_eq!(delayed.window().duration(), 9);
}

#[test]
fn full_timeline_applies_backpressure_without_replacing_an_owner() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let retained = reserve_raw(&mut timeline, 20, 30).expect("one slot is free");

    assert!(matches!(
        reserve_raw(&mut timeline, 40, 50),
        Err(BluetoothSchedulerReservationError::TimelineFull)
    ));
    assert_eq!(retained.window().start(), 20);
}

#[test]
fn release_reuses_storage_with_a_new_identity() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    let first = reserve_raw(&mut timeline, 20, 30).expect("one slot is free");
    let first_generation = first.generation();
    assert!(timeline.release(first).is_ok());
    assert!(timeline.is_empty());

    let second = reserve_raw(&mut timeline, 40, 50).expect("released slot is reusable");
    assert_ne!(second.generation(), first_generation);
}

#[test]
fn rejected_release_returns_the_exact_reservation_and_preserves_both_timelines() {
    let mut source = BluetoothSchedulerTimeline::<1>::new();
    let reservation = reserve_raw(&mut source, 20, 30).expect("source slot is available");
    let mut other = BluetoothSchedulerTimeline::<1>::new();
    let _other_reservation = reserve_raw(&mut other, 40, 50).expect("other slot is available");

    let failure = other
        .release(reservation)
        .expect_err("a different occupied identity must reject release");
    assert_eq!(
        failure.error(),
        BluetoothSchedulerReservationReleaseError::IdentityMismatch
    );
    let reservation = failure.into_reservation();
    assert_eq!(reservation.window().start(), 20);
    assert!(!source.is_empty());
    assert!(!other.is_empty());
    assert!(source.release(reservation).is_ok());
}

#[test]
fn invalid_window_fails_before_occupying_storage() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();
    assert!(matches!(
        reserve_raw(&mut timeline, 20, 0x8000_0014),
        Err(BluetoothSchedulerReservationError::WindowOutsideForwardHalfRange)
    ));
    assert!(timeline.is_empty());
}

#[test]
fn initial_admission_and_recurring_sequence_are_distinct_gates() {
    let mut timeline = BluetoothSchedulerTimeline::<1>::new();

    assert!(matches!(
        timeline.reserve_initial_window(310, 586, timing_policy(), sample(1_000)),
        Err(BluetoothSchedulerReservationError::InitialDeadlineExpired)
    ));
    assert!(timeline.is_empty());

    let recurring = timeline
        .reserve_recurring_window(310, 586, timing_policy())
        .expect("recurring reservation has no initial-admission sample");
    let failure = recurring
        .authorize_sequence(sample(1_000))
        .expect_err("recurring insertion still requires its sequence sample");
    assert!(timeline.release(failure.into_reservation()).is_ok());
    assert!(timeline.is_empty());
}

#[test]
fn exhausted_free_slot_does_not_wrap_its_generation() {
    let mut timeline = BluetoothSchedulerTimeline::<1> {
        slots: [BluetoothSchedulerTimelineSlot {
            window: None,
            generation: u32::MAX,
        }],
    };

    assert!(matches!(
        reserve_raw(&mut timeline, 20, 30),
        Err(BluetoothSchedulerReservationError::GenerationExhausted)
    ));
    assert!(timeline.is_empty());
}
