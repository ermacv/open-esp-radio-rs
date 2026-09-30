//! The receive reorder buffer of one Block Ack agreement.
//!
//! IEEE Std 802.11-2020 10.25.6 makes the recipient of a Block Ack
//! agreement release MSDUs to the upper layer in sequence order. Reordering
//! is software above the lower-MAC port unless a backend reports
//! `HardwareServices::RX_REORDER`. [`RxReorderBuffer`] is that software: it
//! holds the window of one agreement and decides which buffered MPDUs a
//! received MPDU, a BlockAckReq, a gap timeout or a teardown releases.
//!
//! The buffer never holds frame bytes. Every retained MPDU is a checked
//! caller-chosen slot index ([`RxReorderMpdu::slot`]) into the caller's own
//! frame storage; the caller recycles every slot a [`RxReorderRelease`]
//! returns. The buffer never reads time: the caller decides when a gap has
//! waited long enough and calls [`RxReorderBuffer::expire_gap`].
//!
//! `WINDOW_CAPACITY` bounds the negotiated window (at most
//! [`MAX_RX_REORDER_WINDOW`], the width of the occupancy bitmap) and sizes
//! the retained-MPDU table; `SLOT_CAPACITY` is the size of the caller's slot
//! domain. A backend whose memory profile needs a smaller window chooses a
//! smaller `WINDOW_CAPACITY`.

use crate::sequence::SequenceNumber;

/// The widest window a [`RxReorderBuffer`] holds: its occupancy bitmap has
/// one bit per window position.
pub const MAX_RX_REORDER_WINDOW: u16 = 64;

/// One received MPDU of the agreement: its sequence number and the slot of
/// the caller's storage that holds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderMpdu {
    pub sequence: SequenceNumber,
    pub slot: u8,
}

/// Why the buffer refused an operation; the buffer did not change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderError {
    /// A window of zero, or wider than the buffer's `WINDOW_CAPACITY`.
    InvalidWindow(u16),
    /// A slot outside the caller's `SLOT_CAPACITY` domain.
    InvalidSlot(u8),
    /// An agreement names a reorder bank outside its owner's fixed set. The
    /// buffer itself never returns it; an owner of several buffers keyed by
    /// bank reports it through the same type.
    InvalidBank(u8),
    /// The sequence is already buffered.
    DuplicateSequence(SequenceNumber),
    /// The slot already holds a buffered MPDU.
    SlotAlreadyOwned(u8),
}

/// The MPDUs one operation hands back to the caller, in sequence order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderRelease<const WINDOW_CAPACITY: usize> {
    pub frames: [Option<RxReorderMpdu>; WINDOW_CAPACITY],
    pub count: u8,
    /// Sequence numbers the window moved past without an MPDU.
    pub missing: u16,
    /// A received MPDU behind the window: the caller discards it.
    pub rejected: Option<RxReorderMpdu>,
    /// Whether the received MPDU stays buffered after the operation.
    pub buffered: bool,
}

impl<const WINDOW_CAPACITY: usize> RxReorderRelease<WINDOW_CAPACITY> {
    const fn empty() -> Self {
        Self {
            frames: [None; WINDOW_CAPACITY],
            count: 0,
            missing: 0,
            rejected: None,
            buffered: false,
        }
    }

    /// The released MPDUs in sequence order.
    pub fn iter(&self) -> impl Iterator<Item = RxReorderMpdu> + '_ {
        self.frames[..usize::from(self.count)]
            .iter()
            .copied()
            .flatten()
    }

    fn push(&mut self, frame: RxReorderMpdu) {
        let index = usize::from(self.count);
        debug_assert!(index < self.frames.len());
        self.frames[index] = Some(frame);
        self.count += 1;
    }
}

/// The reorder window of one receive Block Ack agreement.
pub struct RxReorderBuffer<const WINDOW_CAPACITY: usize, const SLOT_CAPACITY: usize> {
    starting_sequence: SequenceNumber,
    next_sequence: SequenceNumber,
    window: u16,
    occupied: u64,
    frames: [Option<RxReorderMpdu>; WINDOW_CAPACITY],
}

impl<const WINDOW_CAPACITY: usize, const SLOT_CAPACITY: usize>
    RxReorderBuffer<WINDOW_CAPACITY, SLOT_CAPACITY>
{
    /// An empty window of `window` positions starting at the agreement's
    /// starting sequence.
    pub const fn new(
        starting_sequence: SequenceNumber,
        window: u16,
    ) -> Result<Self, RxReorderError> {
        assert!(
            WINDOW_CAPACITY != 0 && WINDOW_CAPACITY <= MAX_RX_REORDER_WINDOW as usize,
            "the occupancy bitmap holds at most 64 window positions"
        );
        if window == 0 || window as usize > WINDOW_CAPACITY {
            return Err(RxReorderError::InvalidWindow(window));
        }
        Ok(Self {
            starting_sequence,
            next_sequence: starting_sequence,
            window,
            occupied: 0,
            frames: [None; WINDOW_CAPACITY],
        })
    }

    /// The sequence the window waits for (WinStartB).
    pub const fn next_sequence(&self) -> SequenceNumber {
        self.next_sequence
    }

    pub const fn window(&self) -> u16 {
        self.window
    }

    /// Buffered MPDUs.
    pub const fn occupied(&self) -> u32 {
        self.occupied.count_ones()
    }

    /// Rebase the window on the first aggregate after a new agreement when
    /// that aggregate lies behind it, releasing every buffered MPDU.
    ///
    /// A newly negotiated agreement normally accepts the first aggregate in
    /// the forward half of its sequence space. If that aggregate is instead
    /// behind the current window start, the window moves: with
    /// `use_received_sequence` it starts at the received sequence; otherwise
    /// it falls back to the negotiated starting sequence, and only an
    /// aggregate also behind that moves the window to end at the received
    /// sequence. `None` when the sequence is not behind the window.
    ///
    /// This is a recovery edge the caller chooses to apply; the Espressif
    /// reorder path applies it with `use_received_sequence` for formats newer
    /// than HT (`ESP32-S31` receive runtime).
    pub fn resynchronize_stale_initial_ampdu(
        &mut self,
        sequence: SequenceNumber,
        use_received_sequence: bool,
    ) -> Option<(RxReorderRelease<WINDOW_CAPACITY>, SequenceNumber)> {
        if self.next_sequence.forward_distance(sequence) < SequenceNumber::HALF_SPACE {
            return None;
        }

        let mut next_sequence = self.starting_sequence;
        if use_received_sequence
            || self.starting_sequence.forward_distance(sequence) >= SequenceNumber::HALF_SPACE
        {
            next_sequence = sequence.wrapping_sub(self.window - 1);
            if use_received_sequence {
                next_sequence = sequence;
            }
        }

        let released = self.stop();
        self.next_sequence = next_sequence;
        Some((released, next_sequence))
    }

    /// Decide whether this sequence stays buffered after a successful
    /// [`Self::ingest`].
    ///
    /// The next expected sequence is released immediately. Every forward
    /// sequence has at least that first gap in front of it, including after a
    /// bounded window advance, and is therefore buffered. A stale MPDU is
    /// rejected by `ingest` and is never buffered.
    pub fn retains_on_ingest(&self, sequence: SequenceNumber) -> Result<bool, RxReorderError> {
        let distance = self.next_sequence.forward_distance(sequence);
        if distance >= SequenceNumber::HALF_SPACE {
            return Ok(false);
        }
        if distance < self.window && self.has_sequence(sequence) {
            return Err(RxReorderError::DuplicateSequence(sequence));
        }
        let distance_after_advance = if distance >= self.window {
            self.window - 1
        } else {
            distance
        };
        Ok(distance_after_advance != 0)
    }

    /// Consume the common in-order MPDU without building a release list.
    ///
    /// A buffered successor still requires [`Self::ingest`], because the
    /// missing MPDU at the window start must release the complete run in
    /// sequence order. A gap, a stale MPDU or a window advance likewise
    /// returns `None` without changing the buffer.
    pub fn try_ingest_immediate(
        &mut self,
        frame: RxReorderMpdu,
    ) -> Result<Option<RxReorderMpdu>, RxReorderError> {
        self.check_slot(frame.slot)?;
        if frame.sequence != self.next_sequence {
            return Ok(None);
        }
        if self.has_sequence(frame.sequence) {
            return Err(RxReorderError::DuplicateSequence(frame.sequence));
        }
        let successor = self.next_sequence.wrapping_add(1);
        if self.has_sequence(successor) {
            return Ok(None);
        }
        self.next_sequence = successor;
        Ok(Some(frame))
    }

    /// Accept one received MPDU (IEEE Std 802.11-2020 10.25.6.4): release
    /// what the window passes and the run from its start.
    #[inline(always)]
    pub fn ingest(
        &mut self,
        frame: RxReorderMpdu,
    ) -> Result<RxReorderRelease<WINDOW_CAPACITY>, RxReorderError> {
        self.check_slot(frame.slot)?;
        if self
            .frames
            .iter()
            .flatten()
            .any(|owned| owned.slot == frame.slot)
        {
            return Err(RxReorderError::SlotAlreadyOwned(frame.slot));
        }

        let mut release = RxReorderRelease::empty();
        let distance = self.next_sequence.forward_distance(frame.sequence);
        if distance >= SequenceNumber::HALF_SPACE {
            release.rejected = Some(frame);
            return Ok(release);
        }
        if distance >= self.window {
            let advance = distance - self.window + 1;
            self.advance(advance, &mut release);
        }

        let index = Self::slot_index(frame.sequence);
        if self.occupied & (1_u64 << index) != 0 {
            return Err(RxReorderError::DuplicateSequence(frame.sequence));
        }
        self.frames[index] = Some(frame);
        self.occupied |= 1_u64 << index;
        self.release_contiguous(&mut release);
        release.buffered = self.occupied & (1_u64 << index) != 0;
        Ok(release)
    }

    /// Release the first buffered run after the caller's gap timer expired.
    ///
    /// The scan is bounded by the negotiated window. It never reads time or
    /// waits; the caller decides when this edge is due.
    pub fn expire_gap(&mut self) -> RxReorderRelease<WINDOW_CAPACITY> {
        let mut release = RxReorderRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = self.next_sequence.wrapping_add(distance);
            if self.has_sequence(sequence) {
                self.advance(distance, &mut release);
                self.release_contiguous(&mut release);
                return release;
            }
            distance += 1;
        }
        release
    }

    /// Move the window start to a received BlockAckReq's starting sequence.
    ///
    /// Every buffered MPDU before `starting_sequence` is released in order,
    /// then the contiguous run from the new start. A starting sequence equal
    /// to the current start or behind it (in the backward half of the
    /// sequence space) is ignored and returns `None`.
    ///
    /// SOURCE: `libnet80211.a::ieee80211_process_bar_info` ignores those two
    /// cases and otherwise calls `ampdu_dispatch_upto(ni, rx, ssn)` (blobray
    /// RX BlockAckReq answer). The contiguous release after the move is the
    /// recipient behaviour of IEEE Std 802.11-2020 10.25.6.6.
    pub fn move_window_to(
        &mut self,
        starting_sequence: SequenceNumber,
    ) -> Option<RxReorderRelease<WINDOW_CAPACITY>> {
        let distance = self.next_sequence.forward_distance(starting_sequence);
        if distance == 0 || distance >= SequenceNumber::HALF_SPACE {
            return None;
        }
        let mut release = RxReorderRelease::empty();
        self.advance(distance, &mut release);
        self.release_contiguous(&mut release);
        Some(release)
    }

    /// End the agreement: release every buffered MPDU in sequence order.
    pub fn stop(&mut self) -> RxReorderRelease<WINDOW_CAPACITY> {
        let mut release = RxReorderRelease::empty();
        let mut distance = 0_u16;
        while distance < self.window {
            let sequence = self.next_sequence.wrapping_add(distance);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            distance += 1;
        }
        self.occupied = 0;
        release
    }

    const fn check_slot(&self, slot: u8) -> Result<(), RxReorderError> {
        if SLOT_CAPACITY == 0
            || SLOT_CAPACITY > u8::MAX as usize + 1
            || slot as usize >= SLOT_CAPACITY
        {
            Err(RxReorderError::InvalidSlot(slot))
        } else {
            Ok(())
        }
    }

    #[inline(always)]
    fn advance(&mut self, count: u16, release: &mut RxReorderRelease<WINDOW_CAPACITY>) {
        let retained_span = count.min(self.window);
        let released_before = release.count;
        let mut offset = 0_u16;
        while offset < retained_span {
            let sequence = self.next_sequence.wrapping_add(offset);
            if let Some(frame) = self.take_sequence(sequence) {
                release.push(frame);
            }
            offset += 1;
        }
        let released_while_advancing = release.count - released_before;
        release.missing = release
            .missing
            .saturating_add(count.saturating_sub(u16::from(released_while_advancing)));
        self.next_sequence = self.next_sequence.wrapping_add(count);
    }

    #[inline(always)]
    fn release_contiguous(&mut self, release: &mut RxReorderRelease<WINDOW_CAPACITY>) {
        let mut count = 0_u16;
        while count < self.window {
            let Some(frame) = self.take_sequence(self.next_sequence) else {
                break;
            };
            release.push(frame);
            self.next_sequence = self.next_sequence.wrapping_add(1);
            count += 1;
        }
    }

    fn has_sequence(&self, sequence: SequenceNumber) -> bool {
        let index = Self::slot_index(sequence);
        self.occupied & (1_u64 << index) != 0
            && self.frames[index].is_some_and(|frame| frame.sequence == sequence)
    }

    fn take_sequence(&mut self, sequence: SequenceNumber) -> Option<RxReorderMpdu> {
        let index = Self::slot_index(sequence);
        if !self.has_sequence(sequence) {
            return None;
        }
        self.occupied &= !(1_u64 << index);
        self.frames[index].take()
    }

    const fn slot_index(sequence: SequenceNumber) -> usize {
        sequence.get() as usize % WINDOW_CAPACITY
    }
}

#[cfg(test)]
mod tests;
