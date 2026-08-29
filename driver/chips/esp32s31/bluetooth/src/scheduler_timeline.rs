//! Fixed source-owned timeline for Bluetooth scheduler reservations.
//!
//! The vendor Controller walks intrusive scheduler-item links and mutates the
//! candidate window in controller SRAM while resolving overlaps. This layer
//! keeps the equivalent scheduling decision in bounded Rust-owned state. It
//! does not expose the vendor list ABI, allocate, perform MMIO or publish an
//! item to hardware.

#![forbid(unsafe_code)]

use core::marker::PhantomData;

use crate::{
    BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample,
    BluetoothDtmSchedulerItemEvent, BluetoothDtmSchedulerTimingPolicy,
};

const MAX_FORWARD_SPAN: u32 = i32::MAX as u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerRawWindow {
    start: u32,
    end: u32,
}

impl BluetoothSchedulerRawWindow {
    const fn new(start: u32, end: u32) -> Option<Self> {
        if end.wrapping_sub(start) > MAX_FORWARD_SPAN {
            None
        } else {
            Some(Self { start, end })
        }
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

/// Why one DTM item could not acquire a source-owned scheduler reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerReservationError {
    /// The first common-scheduler guard reached the requested start.
    InitialDeadlineExpired,
    /// The requested duration cannot be ordered in one wrapping half-domain.
    WindowOutsideForwardHalfRange,
    /// Overlap displacement left the unambiguous forward half-domain.
    OverlapResolutionOutsideForwardHalfRange,
    /// Every fixed reservation slot is occupied.
    TimelineFull,
    /// Reusing every free slot could make an old identity name a new item.
    GenerationExhausted,
}

/// Reservation state after strict overlap resolution.
#[derive(Debug)]
pub enum BluetoothSchedulerOverlapResolved {}

/// Reservation state after the post-overlap deadline remains open.
#[derive(Debug)]
pub enum BluetoothSchedulerSequenceReady {}

/// Why a resolved reservation cannot authorize sequence-time formation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerSequenceAuthorizationError {
    /// The second fresh sample reached the guarded resolved start.
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
pub struct BluetoothSchedulerReservation<State = BluetoothSchedulerOverlapResolved> {
    slot: usize,
    generation: u32,
    window: BluetoothSchedulerRawWindow,
    event: BluetoothDtmSchedulerItemEvent,
    epoch: BluetoothControllerSchedulerEpoch,
    timing_policy: BluetoothDtmSchedulerTimingPolicy,
    _state: PhantomData<State>,
}

impl<State> BluetoothSchedulerReservation<State> {
    /// Return the resolved raw-time window retained by this reservation.
    pub const fn window(&self) -> BluetoothSchedulerRawWindow {
        self.window
    }

    pub(crate) const fn event(&self) -> BluetoothDtmSchedulerItemEvent {
        self.event
    }

    pub(crate) const fn epoch(&self) -> BluetoothControllerSchedulerEpoch {
        self.epoch
    }

    pub(crate) const fn timing_policy(&self) -> BluetoothDtmSchedulerTimingPolicy {
        self.timing_policy
    }

    #[cfg(test)]
    const fn generation(&self) -> u32 {
        self.generation
    }
}

impl<State> core::fmt::Debug for BluetoothSchedulerReservation<State> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerReservation")
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
    reservation: BluetoothSchedulerReservation<State>,
}

/// Validated exact reservation release retaining an exclusive borrow of its
/// occupied slot until the caller commits.
#[must_use = "the prepared reservation release must be committed"]
pub(crate) struct BluetoothSchedulerReservationReleasePrepared<'timeline, State> {
    window: &'timeline mut Option<BluetoothSchedulerRawWindow>,
    _reservation: BluetoothSchedulerReservation<State>,
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
    pub fn into_reservation(self) -> BluetoothSchedulerReservation<State> {
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

/// Rejected second deadline retaining the exact overlap reservation.
pub struct BluetoothSchedulerSequenceAuthorizationFailure {
    reservation: BluetoothSchedulerReservation<BluetoothSchedulerOverlapResolved>,
    error: BluetoothSchedulerSequenceAuthorizationError,
}

impl BluetoothSchedulerSequenceAuthorizationFailure {
    /// Borrow the finite authorization failure reason.
    pub const fn error(&self) -> BluetoothSchedulerSequenceAuthorizationError {
        self.error
    }

    /// Recover the unchanged overlap reservation for explicit release.
    pub fn into_reservation(
        self,
    ) -> BluetoothSchedulerReservation<BluetoothSchedulerOverlapResolved> {
        self.reservation
    }
}

impl core::fmt::Debug for BluetoothSchedulerSequenceAuthorizationFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothSchedulerSequenceAuthorizationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl BluetoothSchedulerReservation<BluetoothSchedulerOverlapResolved> {
    /// Consume the second fresh time sample required after overlap traversal.
    pub fn authorize_sequence(
        self,
        sample: BluetoothControllerTimeSample,
    ) -> Result<
        BluetoothSchedulerReservation<BluetoothSchedulerSequenceReady>,
        BluetoothSchedulerSequenceAuthorizationFailure,
    > {
        if !self
            .timing_policy
            .initial_deadline_is_open(sample, self.window.start)
        {
            return Err(BluetoothSchedulerSequenceAuthorizationFailure {
                reservation: self,
                error: BluetoothSchedulerSequenceAuthorizationError::DeadlineExpired,
            });
        }

        Ok(BluetoothSchedulerReservation {
            slot: self.slot,
            generation: self.generation,
            window: self.window,
            event: self.event,
            epoch: self.epoch,
            timing_policy: self.timing_policy,
            _state: PhantomData,
        })
    }
}

/// Bounded non-allocating owner of pending Bluetooth scheduler windows.
///
/// Its strict overlap predicate and duration-preserving displacement match the
/// recovered common scheduler behavior, including wrapping signed ordering.
/// Slots are implementation storage only; their indices and generations never
/// become controller-SRAM links.
pub struct BluetoothSchedulerTimeline<const CAPACITY: usize> {
    slots: [BluetoothSchedulerTimelineSlot; CAPACITY],
}

impl<const CAPACITY: usize> BluetoothSchedulerTimeline<CAPACITY> {
    /// Construct one empty fixed-capacity timeline.
    pub const fn new() -> Self {
        Self {
            slots: [BluetoothSchedulerTimelineSlot::EMPTY; CAPACITY],
        }
    }

    /// Reserve the epoch-projected window of one validated DTM event.
    ///
    /// The initial guarded deadline is checked before any overlap mutation.
    /// Each strict overlap moves the candidate to the occupied end while
    /// preserving its wrapping duration. Touching boundaries do not overlap.
    pub fn reserve_dtm_event(
        &mut self,
        event: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
        timing_policy: BluetoothDtmSchedulerTimingPolicy,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<BluetoothSchedulerReservation, BluetoothSchedulerReservationError> {
        self.reserve_raw_window(
            event.raw_start(epoch),
            event.raw_end(epoch),
            event,
            epoch,
            timing_policy,
            admission_sample,
        )
    }

    fn reserve_raw_window(
        &mut self,
        raw_start: u32,
        raw_end: u32,
        event: BluetoothDtmSchedulerItemEvent,
        epoch: BluetoothControllerSchedulerEpoch,
        timing_policy: BluetoothDtmSchedulerTimingPolicy,
        admission_sample: BluetoothControllerTimeSample,
    ) -> Result<BluetoothSchedulerReservation, BluetoothSchedulerReservationError> {
        let Some(mut candidate) = BluetoothSchedulerRawWindow::new(raw_start, raw_end) else {
            return Err(BluetoothSchedulerReservationError::WindowOutsideForwardHalfRange);
        };
        if !timing_policy.initial_deadline_is_open(admission_sample, raw_start) {
            return Err(BluetoothSchedulerReservationError::InitialDeadlineExpired);
        }

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
            return Ok(BluetoothSchedulerReservation {
                slot: slot_index,
                generation,
                window: candidate,
                event,
                epoch,
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
    pub fn release<State>(
        &mut self,
        reservation: BluetoothSchedulerReservation<State>,
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
        reservation: BluetoothSchedulerReservation<State>,
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
    pub fn is_empty(&self) -> bool {
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
        BluetoothSchedulerReservationError, BluetoothSchedulerReservationReleaseError,
        BluetoothSchedulerTimeline, BluetoothSchedulerTimelineSlot,
    };
    use crate::{
        BluetoothControllerSchedulerEpoch, BluetoothControllerTimeSample, BluetoothDtmChannel,
        BluetoothDtmPhy, BluetoothDtmRole, BluetoothDtmSchedulerItemEvent,
        BluetoothDtmSchedulerTimingPolicy, BluetoothSchedulerSoftwareConfig,
    };

    fn scale() -> BluetoothControllerTimeScale {
        BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
    }

    fn timing_policy() -> BluetoothDtmSchedulerTimingPolicy {
        BluetoothDtmSchedulerTimingPolicy::from_scheduler_config(
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
        now: u32,
    ) -> Result<super::BluetoothSchedulerReservation, BluetoothSchedulerReservationError> {
        let epoch = BluetoothControllerSchedulerEpoch::new(sample(0), 0, scale());
        let event = BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(0).expect("channel zero is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRole::Receiver,
            0,
            0,
        )
        .expect("placeholder event is valid");
        timeline.reserve_raw_window(start, end, event, epoch, timing_policy(), sample(now))
    }

    #[test]
    fn non_overlapping_window_keeps_its_position() {
        let mut timeline = BluetoothSchedulerTimeline::<2>::new();
        let reservation = reserve_raw(&mut timeline, 30, 40, 0).expect("window is admissible");

        assert_eq!(reservation.window().start(), 30);
        assert_eq!(reservation.window().end(), 40);
        assert_eq!(reservation.window().duration(), 10);
    }

    #[test]
    fn overlap_chain_delays_once_per_occupied_interval_and_preserves_duration() {
        let mut timeline = BluetoothSchedulerTimeline::<3>::new();
        let later = reserve_raw(&mut timeline, 40, 50, 0).expect("later window is admissible");
        let _earlier = reserve_raw(&mut timeline, 20, 30, 0).expect("earlier window is admissible");
        let delayed = reserve_raw(&mut timeline, 25, 45, 0).expect("overlaps are resolvable");

        assert_eq!(delayed.window().start(), 50);
        assert_eq!(delayed.window().end(), 70);
        assert_eq!(delayed.window().duration(), 20);
        assert_eq!(later.window().end(), delayed.window().start());
    }

    #[test]
    fn touching_boundaries_do_not_overlap() {
        let mut timeline = BluetoothSchedulerTimeline::<2>::new();
        let first = reserve_raw(&mut timeline, 20, 30, 0).expect("first window is admissible");
        let second = reserve_raw(&mut timeline, 30, 40, 0).expect("touching window is admissible");

        assert_eq!(first.window().end(), second.window().start());
    }

    #[test]
    fn overlap_resolution_preserves_wrapping_time_semantics() {
        let mut timeline = BluetoothSchedulerTimeline::<2>::new();
        let now = u32::MAX - 30;
        let occupied = reserve_raw(&mut timeline, u32::MAX - 15, u32::MAX - 5, now)
            .expect("occupied window is ahead across the wrapping epoch");
        let delayed = reserve_raw(&mut timeline, u32::MAX - 19, u32::MAX - 10, now)
            .expect("wrapping overlap is resolvable");

        assert_eq!(delayed.window().start(), occupied.window().end());
        assert_eq!(delayed.window().end(), 3);
        assert_eq!(delayed.window().duration(), 9);
    }

    #[test]
    fn full_timeline_applies_backpressure_without_replacing_an_owner() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let retained = reserve_raw(&mut timeline, 20, 30, 0).expect("one slot is free");

        assert!(matches!(
            reserve_raw(&mut timeline, 40, 50, 0),
            Err(BluetoothSchedulerReservationError::TimelineFull)
        ));
        assert_eq!(retained.window().start(), 20);
    }

    #[test]
    fn release_reuses_storage_with_a_new_identity() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        let first = reserve_raw(&mut timeline, 20, 30, 0).expect("one slot is free");
        let first_generation = first.generation();
        assert!(timeline.release(first).is_ok());
        assert!(timeline.is_empty());

        let second = reserve_raw(&mut timeline, 40, 50, 0).expect("released slot is reusable");
        assert_ne!(second.generation(), first_generation);
    }

    #[test]
    fn rejected_release_returns_the_exact_reservation_and_preserves_both_timelines() {
        let mut source = BluetoothSchedulerTimeline::<1>::new();
        let reservation = reserve_raw(&mut source, 20, 30, 0).expect("source slot is available");
        let mut other = BluetoothSchedulerTimeline::<1>::new();
        let _other_reservation =
            reserve_raw(&mut other, 40, 50, 0).expect("other slot is available");

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
    fn invalid_window_and_expired_guard_fail_before_occupying_storage() {
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();
        assert!(matches!(
            reserve_raw(&mut timeline, 20, 0x8000_0014, 0),
            Err(BluetoothSchedulerReservationError::WindowOutsideForwardHalfRange)
        ));
        assert!(matches!(
            reserve_raw(&mut timeline, 20, 30, 10),
            Err(BluetoothSchedulerReservationError::InitialDeadlineExpired)
        ));
        assert!(timeline.is_empty());
    }

    #[test]
    fn dtm_event_uses_the_same_epoch_projected_window() {
        let epoch = BluetoothControllerSchedulerEpoch::new(sample(100), 1_000, scale());
        let event = BluetoothDtmSchedulerItemEvent::new(
            BluetoothDtmChannel::new(5).expect("channel is valid"),
            BluetoothDtmPhy::Le1M,
            BluetoothDtmRole::Receiver,
            1_012,
            1_020,
        )
        .expect("receiver event is valid");
        let mut timeline = BluetoothSchedulerTimeline::<1>::new();

        let reservation = timeline
            .reserve_dtm_event(event, epoch, timing_policy(), sample(92))
            .expect("projected event passes its initial deadline");

        assert_eq!(reservation.window().start(), 103);
        assert_eq!(reservation.window().end(), 105);
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
            reserve_raw(&mut timeline, 20, 30, 0),
            Err(BluetoothSchedulerReservationError::GenerationExhausted)
        ));
        assert!(timeline.is_empty());
    }
}
