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

    /// Reserve an exact initial window whose protocol phase cannot move.
    ///
    /// Passive scan recurrence is defined by its start-to-start interval. A
    /// collision must therefore reject the selected phase instead of silently
    /// displacing it and corrupting all following channel windows.
    pub(crate) fn reserve_phase_locked_initial_window(
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
mod tests;
