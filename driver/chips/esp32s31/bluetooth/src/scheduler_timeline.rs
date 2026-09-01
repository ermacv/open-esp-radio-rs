//! Fixed source-owned timeline for Bluetooth scheduler reservations.
//!
//! The vendor Controller walks intrusive scheduler-item links and mutates the
//! candidate window in controller SRAM while resolving overlaps. This layer
//! keeps the equivalent scheduling decision in bounded Rust-owned state. It
//! does not expose the vendor list ABI, allocate, perform MMIO or publish an
//! item to hardware.

#![forbid(unsafe_code)]

use core::marker::PhantomData;

use open_esp_radio_esp32s31_pac::BluetoothControllerTimeScale;

use crate::{BluetoothControllerTimeSample, BluetoothSchedulerSoftwareConfig};

const MAX_FORWARD_SPAN: u32 = i32::MAX as u32;

/// Raw-tick insertion timing policy derived from one common scheduler epoch.
///
/// Complete overlap admission converts the scheduler environment's first
/// policy delta through the live Controller time scale for its late-start
/// guard. `r_btdm_sched_calc_seq_time` converts the second delta before adding
/// it to every item. The policy contains no role, descriptor or hardware-list
/// identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerTimingPolicy {
    late_start_guard_raw_delta: u32,
    sequence_lead_raw_delta: u32,
}

impl BluetoothSchedulerTimingPolicy {
    /// Derive both raw timing deltas for one initialized scheduler epoch.
    pub const fn from_scheduler_config(
        config: BluetoothSchedulerSoftwareConfig,
        scale: BluetoothControllerTimeScale,
    ) -> Self {
        Self {
            late_start_guard_raw_delta: scale
                .raw_ticks_from_micros(config.late_start_guard_micros())
                .whole_ticks,
            sequence_lead_raw_delta: scale
                .raw_ticks_from_micros(config.sequence_lead_micros())
                .whole_ticks,
        }
    }

    /// Whether one fresh sample still precedes the guarded item start.
    pub(crate) const fn initial_deadline_is_open(
        self,
        sample: BluetoothControllerTimeSample,
        raw_item_start: u32,
    ) -> bool {
        (sample
            .raw_ticks()
            .wrapping_add(self.late_start_guard_raw_delta)
            .wrapping_sub(raw_item_start) as i32)
            < 0
    }

    pub(crate) const fn sequence_lead_raw_delta(self) -> u32 {
        self.sequence_lead_raw_delta
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerRawWindow {
    start: u32,
    end: u32,
}

impl BluetoothSchedulerRawWindow {
    const fn new(start: u32, end: u32) -> Option<Self> {
        let duration = end.wrapping_sub(start);
        if duration == 0 || duration > MAX_FORWARD_SPAN {
            None
        } else {
            Some(Self { start, end })
        }
    }

    /// Bind a projected scheduler window before timeline admission.
    pub(crate) const fn from_projected_scheduler_window(start: u32, end: u32) -> Option<Self> {
        Self::new(start, end)
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn end(self) -> u32 {
        self.end
    }

    pub const fn duration(self) -> u32 {
        self.end.wrapping_sub(self.start)
    }

    const fn strictly_overlaps(self, other: Self) -> bool {
        (other.end.wrapping_sub(self.start) as i32) > 0
            && (self.end.wrapping_sub(other.start) as i32) > 0
    }

    const fn delayed_after(self, occupied: Self) -> Self {
        let start = occupied.end;
        Self {
            start,
            end: start.wrapping_add(self.duration()),
        }
    }
}

#[derive(Clone, Copy)]
struct BluetoothSchedulerTimelineSlot {
    window: Option<BluetoothSchedulerRawWindow>,
    generation: u32,
}

impl BluetoothSchedulerTimelineSlot {
    const EMPTY: Self = Self {
        window: None,
        generation: 0,
    };
}

/// Why one item could not acquire a source-owned scheduler reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerReservationError {
    /// The first common-scheduler guard reached the requested start.
    InitialDeadlineExpired,
    /// The requested duration cannot be ordered in one wrapping half-domain.
    WindowOutsideForwardHalfRange,
    /// Overlap displacement left the unambiguous forward half-domain.
    OverlapResolutionOutsideForwardHalfRange,
    /// A recurring exact window collided with an occupied interval.
    RecurringOverlapUnsupported,
    /// Every fixed reservation slot is occupied.
    TimelineFull,
    /// Reusing every free slot could make an old identity name a new item.
    GenerationExhausted,
}

/// Initial-event reservation after the admission sample and overlap resolution.
#[derive(Debug)]
pub(crate) enum BluetoothSchedulerInitialAdmissionResolved {}

/// Exact recurring-event reservation formed without initial admission or displacement.
#[derive(Debug)]
pub(crate) enum BluetoothSchedulerRecurringReserved {}

/// Reservation state after the phase-appropriate sequence deadline remains open.
#[derive(Debug)]
pub enum BluetoothSchedulerSequenceReady {}

/// Why a retained reservation cannot authorize sequence-time formation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerSequenceAuthorizationError {
    /// The fresh sequence sample reached the guarded retained start.
    DeadlineExpired,
}

/// Why a source-owned reservation could not be released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerReservationReleaseError {
    /// The reservation names no slot in this timeline instance.
    SlotOutOfRange,
    /// The slot generation or window belongs to another reservation epoch.
    IdentityMismatch,
}

/// Affine identity of one source-owned scheduler reservation.
///
/// Dropping this value does not silently remove the occupied interval. The
/// scheduler lifecycle must return it to the same timeline on cancellation or
/// completion.
#[must_use = "the scheduler reservation must be retained until cancellation or completion"]
pub struct BluetoothSchedulerWindowReservation<State> {
    slot: usize,
    generation: u32,
    window: BluetoothSchedulerRawWindow,
    timing_policy: BluetoothSchedulerTimingPolicy,
    _state: PhantomData<State>,
}

impl<State> BluetoothSchedulerWindowReservation<State> {
    /// Return the phase-bound raw-tick window retained by this reservation.
    pub const fn window(&self) -> BluetoothSchedulerRawWindow {
        self.window
    }

    pub(crate) const fn timing_policy(&self) -> BluetoothSchedulerTimingPolicy {
        self.timing_policy
    }

    #[cfg(test)]
    const fn generation(&self) -> u32 {
        self.generation
    }
}

impl<State> core::fmt::Debug for BluetoothSchedulerWindowReservation<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerWindowReservation")
            .field("slot", &self.slot)
            .field("generation", &self.generation)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

/// Lossless scheduler-reservation release rejection.
#[must_use = "a rejected reservation remains occupied and must be retained"]
pub struct BluetoothSchedulerReservationReleaseFailure<State> {
    error: BluetoothSchedulerReservationReleaseError,
    reservation: BluetoothSchedulerWindowReservation<State>,
}

/// Validated exact reservation release retaining an exclusive borrow of its
/// occupied slot until the caller commits.
#[must_use = "the prepared reservation release must be committed"]
pub(crate) struct BluetoothSchedulerReservationReleasePrepared<'timeline, State> {
    window: &'timeline mut Option<BluetoothSchedulerRawWindow>,
    _reservation: BluetoothSchedulerWindowReservation<State>,
}

impl<State> BluetoothSchedulerReservationReleasePrepared<'_, State> {
    /// Clear the already-validated slot without another fallible identity
    /// check.
    pub(crate) fn commit(self) {
        *self.window = None;
    }
}

impl<State> BluetoothSchedulerReservationReleaseFailure<State> {
    /// Exact reason the timeline rejected this reservation.
    pub const fn error(&self) -> BluetoothSchedulerReservationReleaseError {
        self.error
    }

    /// Recover the unchanged affine reservation.
    pub fn into_reservation(self) -> BluetoothSchedulerWindowReservation<State> {
        self.reservation
    }
}

impl<State> core::fmt::Debug for BluetoothSchedulerReservationReleaseFailure<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerReservationReleaseFailure")
            .field("error", &self.error)
            .field("reservation", &self.reservation)
            .finish()
    }
}

/// Rejected sequence deadline retaining the exact pre-sequence reservation.
pub(crate) struct BluetoothSchedulerSequenceAuthorizationFailure<State> {
    reservation: BluetoothSchedulerWindowReservation<State>,
    error: BluetoothSchedulerSequenceAuthorizationError,
}

impl<State> BluetoothSchedulerSequenceAuthorizationFailure<State> {
    /// Borrow the finite authorization failure reason.
    pub(crate) const fn error(&self) -> BluetoothSchedulerSequenceAuthorizationError {
        self.error
    }

    /// Recover the unchanged pre-sequence reservation for explicit release.
    pub(crate) fn into_reservation(self) -> BluetoothSchedulerWindowReservation<State> {
        self.reservation
    }
}

impl<State> core::fmt::Debug for BluetoothSchedulerSequenceAuthorizationFailure<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerSequenceAuthorizationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved> {
    /// Consume the second fresh sample after initial admission and overlap traversal.
    pub(crate) fn authorize_sequence(
        self,
        sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
        BluetoothSchedulerSequenceAuthorizationFailure<BluetoothSchedulerInitialAdmissionResolved>,
    > {
        authorize_sequence(self, sample)
    }
}

impl BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved> {
    /// Consume the sole fresh deadline sample used by recurring insertion.
    pub(crate) fn authorize_sequence(
        self,
        sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
        BluetoothSchedulerSequenceAuthorizationFailure<BluetoothSchedulerRecurringReserved>,
    > {
        authorize_sequence(self, sample)
    }
}

fn authorize_sequence<State>(
    reservation: BluetoothSchedulerWindowReservation<State>,
    sample: BluetoothControllerTimeSample,
) -> Result<
    BluetoothSchedulerWindowReservation<BluetoothSchedulerSequenceReady>,
    BluetoothSchedulerSequenceAuthorizationFailure<State>,
> {
    if !reservation
        .timing_policy
        .initial_deadline_is_open(sample, reservation.window.start)
    {
        return Err(BluetoothSchedulerSequenceAuthorizationFailure {
            reservation,
            error: BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
        });
    }

    Ok(BluetoothSchedulerWindowReservation {
        slot: reservation.slot,
        generation: reservation.generation,
        window: reservation.window,
        timing_policy: reservation.timing_policy,
        _state: PhantomData,
    })
}

/// Bounded non-allocating owner of pending Bluetooth scheduler windows.
///
/// Its strict overlap predicate and duration-preserving displacement match the
/// recovered common scheduler behavior, including wrapping signed ordering.
/// Slots are implementation storage only; their indices and generations never
/// become controller-SRAM links.
pub(crate) struct BluetoothSchedulerTimeline<const CAPACITY: usize> {
    slots: [BluetoothSchedulerTimelineSlot; CAPACITY],
}

impl<const CAPACITY: usize> BluetoothSchedulerTimeline<CAPACITY> {
    /// Construct one empty fixed-capacity timeline.
    pub(crate) const fn new() -> Self {
        Self {
            slots: [BluetoothSchedulerTimelineSlot::EMPTY; CAPACITY],
        }
    }

    /// Reserve one already projected initial window.
    ///
    /// The initial guarded deadline is checked before any overlap mutation.
    /// Each strict overlap moves the candidate to the occupied end while
    /// preserving its wrapping duration. Touching boundaries do not overlap.
    pub(crate) fn reserve_initial_window(
        &mut self,
        raw_start: u32,
        raw_end: u32,
        timing_policy: BluetoothSchedulerTimingPolicy,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
        BluetoothSchedulerReservationError,
    > {
        if !timing_policy.initial_deadline_is_open(admission_sample, raw_start) {
            return Err(BluetoothSchedulerReservationError::InitialDeadlineExpired);
        }
        self.reserve_initial_raw_window(raw_start, raw_end, timing_policy)
    }

    /// Reserve one exact recurring event without initial admission or displacement.
    ///
    /// The reviewed recurring helper bypasses the delay-if-overlap path. Until
    /// its removal policy has an affine model, any occupied collision rejects
    /// the candidate without changing either window.
    pub(crate) fn reserve_recurring_window(
        &mut self,
        raw_start: u32,
        raw_end: u32,
        timing_policy: BluetoothSchedulerTimingPolicy,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>,
        BluetoothSchedulerReservationError,
    > {
        self.reserve_recurring_raw_window(raw_start, raw_end, timing_policy)
    }

    fn reserve_initial_raw_window(
        &mut self,
        raw_start: u32,
        raw_end: u32,
        timing_policy: BluetoothSchedulerTimingPolicy,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerInitialAdmissionResolved>,
        BluetoothSchedulerReservationError,
    > {
        let Some(mut candidate) = BluetoothSchedulerRawWindow::new(raw_start, raw_end) else {
            return Err(BluetoothSchedulerReservationError::WindowOutsideForwardHalfRange);
        };

        let original_start = candidate.start;
        let mut displaced_by = [false; CAPACITY];
        while let Some((slot_index, occupied)) =
            self.slots
                .iter()
                .enumerate()
                .find_map(|(slot_index, slot)| {
                    slot.window
                        .filter(|occupied| candidate.strictly_overlaps(*occupied))
                        .map(|occupied| (slot_index, occupied))
                })
        {
            if displaced_by[slot_index] {
                return Err(
                    BluetoothSchedulerReservationError::OverlapResolutionOutsideForwardHalfRange,
                );
            }
            displaced_by[slot_index] = true;
            candidate = candidate.delayed_after(occupied);
            if candidate.start.wrapping_sub(original_start) > MAX_FORWARD_SPAN {
                return Err(
                    BluetoothSchedulerReservationError::OverlapResolutionOutsideForwardHalfRange,
                );
            }
        }

        self.reserve_window(candidate, timing_policy)
    }

    fn reserve_recurring_raw_window(
        &mut self,
        raw_start: u32,
        raw_end: u32,
        timing_policy: BluetoothSchedulerTimingPolicy,
    ) -> Result<
        BluetoothSchedulerWindowReservation<BluetoothSchedulerRecurringReserved>,
        BluetoothSchedulerReservationError,
    > {
        let Some(candidate) = BluetoothSchedulerRawWindow::new(raw_start, raw_end) else {
            return Err(BluetoothSchedulerReservationError::WindowOutsideForwardHalfRange);
        };
        if self.slots.iter().any(|slot| {
            slot.window
                .is_some_and(|occupied| candidate.strictly_overlaps(occupied))
        }) {
            return Err(BluetoothSchedulerReservationError::RecurringOverlapUnsupported);
        }

        self.reserve_window(candidate, timing_policy)
    }

    fn reserve_window<State>(
        &mut self,
        candidate: BluetoothSchedulerRawWindow,
        timing_policy: BluetoothSchedulerTimingPolicy,
    ) -> Result<BluetoothSchedulerWindowReservation<State>, BluetoothSchedulerReservationError>
    {
        let mut generation_exhausted = false;
        for (slot_index, slot) in self.slots.iter_mut().enumerate() {
            if slot.window.is_some() {
                continue;
            }
            let Some(generation) = slot.generation.checked_add(1) else {
                generation_exhausted = true;
                continue;
            };
            slot.generation = generation;
            slot.window = Some(candidate);
            return Ok(BluetoothSchedulerWindowReservation {
                slot: slot_index,
                generation,
                window: candidate,
                timing_policy,
                _state: PhantomData,
            });
        }

        Err(if generation_exhausted {
            BluetoothSchedulerReservationError::GenerationExhausted
        } else {
            BluetoothSchedulerReservationError::TimelineFull
        })
    }

    /// Release exactly the reservation named by the affine identity.
    ///
    /// Rejection returns the unchanged affine reservation. A stale generation
    /// can never release a later occupant of the same slot or disappear into a
    /// lossy boolean result.
    pub(crate) fn release<State>(
        &mut self,
        reservation: BluetoothSchedulerWindowReservation<State>,
    ) -> Result<(), BluetoothSchedulerReservationReleaseFailure<State>> {
        self.prepare_release(reservation).map(|prepared| {
            prepared.commit();
        })
    }

    /// Validate one exact release and retain the matching slot exclusively.
    ///
    /// The prepared release lets a composed owner finish earlier memory
    /// cleanup before the timeline becomes reusable, without repeating an
    /// identity check or introducing a post-cleanup failure path.
    pub(crate) fn prepare_release<State>(
        &mut self,
        reservation: BluetoothSchedulerWindowReservation<State>,
    ) -> Result<
        BluetoothSchedulerReservationReleasePrepared<'_, State>,
        BluetoothSchedulerReservationReleaseFailure<State>,
    > {
        let Some(slot) = self.slots.get_mut(reservation.slot) else {
            return Err(BluetoothSchedulerReservationReleaseFailure {
                error: BluetoothSchedulerReservationReleaseError::SlotOutOfRange,
                reservation,
            });
        };
        if slot.generation != reservation.generation || slot.window != Some(reservation.window) {
            return Err(BluetoothSchedulerReservationReleaseFailure {
                error: BluetoothSchedulerReservationReleaseError::IdentityMismatch,
                reservation,
            });
        }
        Ok(BluetoothSchedulerReservationReleasePrepared {
            window: &mut slot.window,
            _reservation: reservation,
        })
    }

    /// Whether no scheduler window remains reserved.
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.window.is_none())
    }
}

impl<const CAPACITY: usize> Default for BluetoothSchedulerTimeline<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_pac::{
        BluetoothControllerHalInitConfig, BluetoothControllerTimeScale,
    };

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
}
