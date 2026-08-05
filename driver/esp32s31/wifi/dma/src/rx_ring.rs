//! Typed ownership and publication for the ESP32-S31 RX DMA ring.
//!
//! This module contains descriptor-walker mechanics only. 802.11 frame
//! decoding remains in the LMAC crate, while the semantic MMIO operations are
//! defined by [`crate::rx_dma::RxDma`].

use crate::{
    descriptor::{
        BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_MASK, LENGTH_SHIFT, SIZE_MASK,
        descriptor_address_valid, dma_range_valid, length as descriptor_length, rx_armed_word,
        rx_done, rx_rearm_word, size as descriptor_size,
    },
    rx_dma::RxDma,
};

/// Guard value restored by the ROM RX recycler at both DMA-buffer bounds.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: `esp32s31_rev0_rom.elf`,
/// `wDev_AppendRxBlocks` at `0x2f838a7e`, complete size `0x132`, writes
/// `0xdead_beef` at `buffer` and `buffer + descriptor_capacity`.
/// The trailing word lives immediately after the descriptor-advertised
/// capacity, so backing storage must reserve four additional bytes.
pub const RX_BUFFER_SENTINEL: u32 = 0xdead_beef;

const RX_DESCRIPTOR_ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
/// Exact maximum number of `APPEND_DESCRIPTOR_RELOAD` observations recovered
/// from `wDev_AppendRxBlocks`.
///
/// The source-owned driver exposes each observation separately so an Embassy
/// integration can yield between them instead of reproducing the ROM spin.
pub const RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT: u32 = 0x0001_86a1;

#[derive(Clone, Copy, Debug)]
pub struct RxSegment<'a> {
    pub descriptor_address: u32,
    pub descriptor_word0: u32,
    pub buffer: &'a [u8],
    pub next_descriptor_address: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxRingError {
    Empty,
    Count,
    Address,
    Size,
    Overflow,
    Busy,
    Corrupt,
}

/// One descriptor whose completion ownership has moved from the MAC to Rust.
///
/// Taking it through [`RxRingLive::take_completed`] also records that this
/// descriptor must not be exposed a second time before its recycle group has
/// been rearmed. The token is deliberately neither `Copy` nor constructible
/// by callers: consuming it at the staging boundary represents the unique
/// software owner of that completed DMA descriptor.
#[derive(Debug, PartialEq, Eq)]
pub struct RxCompletedDescriptor {
    index: usize,
    descriptor_address: u32,
    word0: u32,
    next_descriptor_address: u32,
}

impl RxCompletedDescriptor {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn word0(&self) -> u32 {
        self.word0
    }

    pub const fn next_descriptor_address(&self) -> u32 {
        self.next_descriptor_address
    }
}

/// Finite snapshot of complete RX units at the live recycle frontier.
///
/// A unit may occupy more than one descriptor. Only the final descriptor has
/// `RX_DONE`; a later terminal descriptor proves that each preceding node in
/// the sequential ring belongs to that same completed unit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxCompletedUnitFrontier {
    pub unit_count: usize,
    pub descriptor_count: usize,
}

/// Unique ownership of one complete, possibly chained, RX unit.
///
/// Segment indices are implicit contiguous ring indices beginning at
/// [`head_index`](Self::head_index). Lengths are captured before recycle so a
/// staging owner can copy every DMA segment without rereading mutable
/// descriptor state. S31 live rings are limited to the 64 bits represented by
/// `RxRingLive::observed_mask`.
#[derive(Debug, Eq, PartialEq)]
pub struct RxCompletedUnit {
    head_index: usize,
    descriptor_count: usize,
    descriptor_address: u32,
    staged_word0: u32,
    total_length: usize,
    segment_lengths: RxCompletedUnitLengths,
}

#[derive(Debug, Eq, PartialEq)]
enum RxCompletedUnitLengths {
    Single(u16),
    Chained([u16; 64]),
}

impl RxCompletedUnit {
    pub const fn head_index(&self) -> usize {
        self.head_index
    }

    pub const fn descriptor_count(&self) -> usize {
        self.descriptor_count
    }

    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    /// Synthetic single-segment metadata for the independent contiguous
    /// staging copy. It preserves the first descriptor's status bits while
    /// describing the complete unit length and terminal ownership.
    pub const fn staged_word0(&self) -> u32 {
        self.staged_word0
    }

    pub const fn total_length(&self) -> usize {
        self.total_length
    }

    pub fn segment_length(&self, step: usize) -> Option<usize> {
        if step >= self.descriptor_count {
            return None;
        }
        match &self.segment_lengths {
            RxCompletedUnitLengths::Single(length) => (step == 0).then_some(usize::from(*length)),
            RxCompletedUnitLengths::Chained(lengths) => Some(usize::from(lengths[step])),
        }
    }
}

/// One live append accepted for publication to the RX descriptor walker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxLiveAppend {
    pub head_index: usize,
    pub head_address: u32,
    pub tail_index: usize,
    pub descriptor_count: usize,
}

/// Result of one finite observation of the live RX append doorbell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReloadObservation {
    Pending,
    Settled,
}

/// Prepared zero-terminated RX ring while the hardware walker is stopped.
///
/// This type owns the right to start the descriptor walker. Consuming
/// [`start`](Self::start) transfers that authority into [`RxRingLive`].
pub struct RxRingStopped<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
    initial_start: usize,
    accepted_tail: usize,
    retained_last_low: u32,
}

/// Sole software owner of one running S31 RX descriptor frontier.
///
/// The owner tracks three distinct states recovered from
/// `wDev_AppendRxBlocks`: descriptors observed as CPU-owned, the last tail
/// accepted by hardware, and a future tail whose reload doorbell is still in
/// flight. No allocator, global `wDevCtrl`, C ABI or vendor callback is needed.
pub struct RxRingLive<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
    observed_mask: u64,
    recycle_start: usize,
    accepted_tail: usize,
    pending_tail: Option<usize>,
}

/// Descriptor storage authority after the hardware walker is confirmed off.
///
/// Live frontier bookkeeping is deliberately discarded at this lifecycle
/// edge. A later association must rebuild and republish the ring through
/// [`prepare`](Self::prepare), never resume descriptors from the previous
/// peer epoch.
pub struct RxRingHalted<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
}

impl<'a, const COUNT: usize> RxRingHalted<'a, COUNT> {
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub const fn descriptors(&self) -> &'a [Descriptor; COUNT] {
        self.descriptors
    }

    pub const fn buffer_addresses(&self) -> &'a [u32; COUNT] {
        self.buffer_addresses
    }

    /// Rebuild the stopped ring for a new ownership epoch.
    ///
    /// Failure returns the halted authority even if hardware observations or
    /// descriptor preparation rejected the attempt. The caller can then
    /// retry after a higher-level reset without stealing the static storage.
    pub fn prepare<M, F>(
        self,
        mmio: &mut M,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<RxRingStopped<'a, COUNT>, (Self, RxRingError)>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        match RxRingStopped::prepare(
            mmio,
            self.descriptors,
            self.descriptor_base,
            self.buffer_addresses,
            buffer_size,
            prepare_buffer,
        ) {
            Ok(stopped) => Ok(stopped),
            Err(error) => Err((self, error)),
        }
    }
}

impl<'a, const COUNT: usize> RxRingStopped<'a, COUNT> {
    /// Discard a prepared-but-never-started epoch while retaining descriptor
    /// storage authority.
    ///
    /// `RxRingStopped` can exist only after the walker was observed disabled.
    /// No descriptor has crossed back to hardware until `try_start` succeeds,
    /// so a higher-level scan failure may safely return to the peer-neutral
    /// halted frontier without manufacturing or re-reading static addresses.
    pub fn into_halted(self) -> RxRingHalted<'a, COUNT> {
        RxRingHalted {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
        }
    }

    /// Stops the walker, prepares all buffers and publishes a rotated cold
    /// list beginning after the descriptor retained by the previous owner.
    ///
    /// `prepare_buffer` must restore any buffer-side DMA contract for `index`;
    /// for the S31 ROM layout this means the two `0xdead_beef` sentinels. It is
    /// invoked only while the walker is confirmed stopped.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
    /// ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR]; the rotated handoff is qualified
    /// by HIL_OPEN_RX_LIVE_APPEND_2026_07_27.
    pub fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a [u32; COUNT],
        buffer_size: u32,
        mut prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        validate_live_ring_geometry::<COUNT>()?;
        let retained_last_low = mmio.last_descriptor_low();
        let initial_start = descriptor_index(retained_last_low, descriptor_base, COUNT)
            .map_or(0, |index| if index + 1 == COUNT { 0 } else { index + 1 });

        if mmio.walker_enabled() {
            disable_receive(mmio)?;
        }
        for index in 0..COUNT {
            prepare_buffer(index)?;
        }
        build_cold_ring(descriptors, descriptor_base, buffer_addresses, buffer_size)?;
        relink_rotated_ring(
            descriptors,
            descriptor_base,
            buffer_addresses,
            initial_start,
        )?;
        let head = descriptor_address(descriptor_base, initial_start)?;
        publish_cold_ring(mmio, head, false)?;

        Ok(Self {
            descriptors,
            descriptor_base,
            buffer_addresses,
            initial_start,
            accepted_tail: wrap_sub_one::<COUNT>(initial_start),
            retained_last_low,
        })
    }

    pub const fn initial_start(&self) -> usize {
        self.initial_start
    }

    pub const fn accepted_tail(&self) -> usize {
        self.accepted_tail
    }

    pub const fn retained_last_low(&self) -> u32 {
        self.retained_last_low
    }

    /// Opens the walker and consumes the stopped-state authority.
    ///
    /// The caller owns any platform-specific settle delay between
    /// [`prepare`](Self::prepare) and this edge.
    pub fn try_start<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingLive<'a, COUNT>, (Self, RxRingError)> {
        if let Err(error) = enable_receive(mmio) {
            return Err((self, error));
        }
        Ok(RxRingLive {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            observed_mask: 0,
            recycle_start: self.initial_start,
            accepted_tail: self.accepted_tail,
            pending_tail: None,
        })
    }

    /// Compatibility form for callers that terminate the complete radio
    /// owner when walker activation fails.
    pub fn start<M: RxDma>(self, mmio: &mut M) -> Result<RxRingLive<'a, COUNT>, RxRingError> {
        self.try_start(mmio).map_err(|(_, error)| error)
    }
}

impl<'a, const COUNT: usize> RxRingLive<'a, COUNT> {
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
    }

    pub(crate) const fn descriptors(&self) -> &'a [Descriptor; COUNT] {
        self.descriptors
    }

    pub(crate) const fn buffer_addresses(&self) -> &'a [u32; COUNT] {
        self.buffer_addresses
    }

    /// Stop the DMA walker and consume the live frontier authority.
    ///
    /// A failed hardware confirmation returns the complete live owner. This
    /// prevents lifecycle code from rebuilding descriptors while DMA may
    /// still own them.
    pub fn try_stop<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingHalted<'a, COUNT>, (Self, RxRingError)> {
        if let Err(error) = disable_receive(mmio) {
            return Err((self, error));
        }
        Ok(RxRingHalted {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
        })
    }

    /// Snapshot the contiguous, newly completed prefix at the current recycle
    /// frontier without transferring ownership or rearming any descriptor.
    ///
    /// The returned count is a finite receive epoch. A caller may subsequently
    /// take and recycle exactly this many descriptors without accidentally
    /// extending the same service pass to descriptors completed after the
    /// snapshot. This is important when recycled descriptors can already be
    /// filled again while the task is still draining an RX-success wake.
    pub fn completed_frontier_len(&self) -> usize {
        let mut completed = 0;
        while completed < COUNT {
            let index = wrap_add::<COUNT>(self.recycle_start, completed);
            let bit = 1_u64 << index;
            if self.observed_mask & bit != 0 || !rx_done(self.descriptors[index].word0()) {
                break;
            }
            completed += 1;
        }
        completed
    }

    /// Snapshot complete RX units, including units split across descriptors.
    ///
    /// A non-terminal descriptor is never reported by itself: the scan only
    /// extends `descriptor_count` after observing a later `RX_DONE` terminal.
    /// Thus an armed or partially filled frontier remains owned by hardware.
    pub fn completed_unit_frontier(&self) -> RxCompletedUnitFrontier {
        self.completed_unit_frontier_with(|_| true)
    }

    /// Variant of [`completed_unit_frontier`](Self::completed_unit_frontier)
    /// that requires buffer-side evidence for each non-terminal descriptor.
    ///
    /// An armed descriptor and a consumed full non-terminal descriptor can
    /// have the same ownership word. The storage owner can disambiguate them
    /// by checking the buffer guard restored at recycle. This also prevents a
    /// terminal left elsewhere in a handoff epoch from being joined to an
    /// untouched frontier descriptor.
    pub fn completed_unit_frontier_with<F>(
        &self,
        mut nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if COUNT == 0 || COUNT > 64 {
            return RxCompletedUnitFrontier::default();
        }
        let first_index = self.recycle_start;
        if self.observed_mask & (1_u64 << first_index) != 0 {
            return RxCompletedUnitFrontier::default();
        }
        if rx_done(self.descriptors[first_index].word0()) {
            let mut completed = 0;
            while completed < COUNT {
                let index = wrap_add::<COUNT>(self.recycle_start, completed);
                let bit = 1_u64 << index;
                if self.observed_mask & bit != 0 || !rx_done(self.descriptors[index].word0()) {
                    break;
                }
                completed += 1;
            }
            return RxCompletedUnitFrontier {
                unit_count: completed,
                descriptor_count: completed,
            };
        }
        if !nonterminal_consumed(first_index) {
            return RxCompletedUnitFrontier::default();
        }
        // Only a later terminal can distinguish a consumed non-terminal
        // segment from an ordinary armed descriptor. Report at most this one
        // chain; a following unit belongs to the next finite service epoch.
        for step in 1..COUNT {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let bit = 1_u64 << index;
            if self.observed_mask & bit != 0 {
                break;
            }
            if rx_done(self.descriptors[index].word0()) {
                return RxCompletedUnitFrontier {
                    unit_count: 1,
                    descriptor_count: step + 1,
                };
            }
            if !nonterminal_consumed(index) {
                break;
            }
        }
        RxCompletedUnitFrontier::default()
    }

    /// Transfer one complete RX unit at the recycle frontier exactly once.
    ///
    /// `descriptor_limit` should come from a prior
    /// [`completed_unit_frontier`](Self::completed_unit_frontier) snapshot. It
    /// prevents a saturated producer from extending this ownership transfer
    /// to a later completion epoch.
    pub fn take_completed_unit(&mut self, descriptor_limit: usize) -> Option<RxCompletedUnit> {
        if COUNT == 0 || COUNT > 64 || descriptor_limit == 0 || descriptor_limit > COUNT {
            return None;
        }
        let first_index = self.recycle_start;
        let first_bit = 1_u64 << first_index;
        if self.observed_mask & first_bit != 0 {
            return None;
        }
        let first_descriptor = &self.descriptors[first_index];
        let first_word0 = first_descriptor.word0();
        let first_length = descriptor_length(first_word0);
        if descriptor_size(first_word0) == 0 || first_length > descriptor_size(first_word0) {
            return None;
        }
        if rx_done(first_word0) {
            self.observed_mask |= first_bit;
            let encoded_length = first_length;
            return Some(RxCompletedUnit {
                head_index: first_index,
                descriptor_count: 1,
                descriptor_address: descriptor_address(self.descriptor_base, first_index).ok()?,
                staged_word0: (first_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length: first_length as usize,
                segment_lengths: RxCompletedUnitLengths::Single(u16::try_from(first_length).ok()?),
            });
        }

        let mut segment_lengths = [0_u16; 64];
        segment_lengths[0] = u16::try_from(first_length).ok()?;
        let mut total_length = first_length as usize;
        for step in 1..descriptor_limit {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let bit = 1_u64 << index;
            if self.observed_mask & bit != 0 {
                return None;
            }
            let descriptor = &self.descriptors[index];
            let word0 = descriptor.word0();
            let length = descriptor_length(word0);
            if descriptor_size(word0) == 0 || length > descriptor_size(word0) {
                return None;
            }
            let previous = wrap_add::<COUNT>(self.recycle_start, step - 1);
            if self.descriptors[previous].next_address()
                != descriptor_address(self.descriptor_base, index).ok()?
            {
                return None;
            }
            segment_lengths[step] = u16::try_from(length).ok()?;
            total_length = total_length.checked_add(length as usize)?;
            if !rx_done(word0) {
                continue;
            }
            if total_length > SIZE_MASK as usize {
                return None;
            }
            let descriptor_count = step + 1;
            let group_mask = recycle_group_mask::<COUNT>(self.recycle_start, descriptor_count);
            self.observed_mask |= group_mask;
            let encoded_length = u32::try_from(total_length).ok()?;
            return Some(RxCompletedUnit {
                head_index: self.recycle_start,
                descriptor_count,
                descriptor_address: descriptor_address(self.descriptor_base, self.recycle_start)
                    .ok()?,
                staged_word0: (first_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length,
                segment_lengths: RxCompletedUnitLengths::Chained(segment_lengths),
            });
        }
        None
    }

    crate::place_rx_hot_path! {
    /// Takes one newly completed descriptor exactly once for this ring epoch.
    ///
    /// Kept in internal SRAM for PSRAM-code profiles: this is invoked once for
    /// every descriptor slot on every receive poll. HIL at HE20 showed that
    /// executing the complete poll/copy path from PSRAM capped useful UDP RX
    /// near 65 Mbit/s.
    #[inline(never)]
    pub fn take_completed(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
        if index >= COUNT {
            return None;
        }
        let bit = 1_u64 << index;
        if self.observed_mask & bit != 0 {
            return None;
        }
        let descriptor = &self.descriptors[index];
        let word0 = descriptor.word0();
        if !rx_done(word0) {
            return None;
        }
        self.observed_mask |= bit;
        Some(RxCompletedDescriptor {
            index,
            descriptor_address: descriptor_address(self.descriptor_base, index).ok()?,
            word0,
            next_descriptor_address: descriptor.next_address(),
        })
    }}

    crate::place_rx_hot_path! {
    /// Settles a prior append and, when the next half is entirely CPU-owned,
    /// rearms and appends it without stopping the live walker.
    ///
    /// This is the allocation-free Rust ownership form of the recovered
    /// `wDevCtrl.head/tail` transaction. The future tail is kept private until
    /// RX_CONTROL bit 0 self-clears. If the walker exhausted the old frontier
    /// during reload, the exact ROM base-repair rule is applied before the new
    /// tail becomes accepted.
    #[inline(never)]
    pub fn recycle_completed_half<M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_group(mmio, COUNT / 2, false, prepare_buffer)
    }}

    crate::place_rx_hot_path! {
    /// Rearm and append one fixed-size, contiguous group of completed slots.
    ///
    /// The complete ROM `wDev_AppendRxBlocks` contract accepts an arbitrary
    /// zero-terminated `head..tail` chain; a half-ring is not a hardware
    /// requirement. Smaller groups let a task replenish the walker before a
    /// peer's next A-MPDU consumes every remaining descriptor. `BATCH` must
    /// divide the ring so `recycle_start` visits each slot exactly once.
    ///
    /// The caller must still drive [`Self::poll_pending_reload`] after each
    /// successful append. This deliberately preserves the ROM doorbell/base
    /// repair ordering without hiding a wait in this finite operation.
    #[inline(never)]
    pub fn recycle_completed_batch<const BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if BATCH == 0 || BATCH > COUNT || !COUNT.is_multiple_of(BATCH) {
            return Err(RxRingError::Count);
        }
        self.recycle_completed_group(mmio, BATCH, false, prepare_buffer)
    }}

    crate::place_rx_hot_path! {
    /// Rearm the longest currently completed prefix, up to `MAX_BATCH`.
    ///
    /// Unlike [`Self::recycle_completed_batch`], this does not wait for a
    /// fixed-size group. It is the closer ownership model for the complete
    /// vendor receive path:
    ///
    /// - `wDev_ProcessFiq` posts PP event `0x19`;
    /// - `ppTask` calls `wdevProcessRxSucDataAll`;
    /// - that walker counts the descriptors in one completed RX unit;
    /// - `wDev_DiscardFrame` immediately transfers `(head, tail, count)` to
    ///   `wDev_AppendRxBlocks`.
    ///
    /// The complete vendor archive places `wDev_AppendRxBlocks` in
    /// `wdev.o:.wifislprxiram.55`, `wDev_DiscardFrame` in
    /// `.wifislpiram.18`, and `wdevProcessRxSucDataAll` in
    /// `.wifislprxiram.28`. This proves that prompt reclaim is an intentional
    /// SRAM-resident software path rather than a background hardware feature.
    ///
    /// Hardware provides completion, last/next and the reload doorbell. The
    /// variable-size reclaim policy is software, not automatic DMA recycling.
    /// `MAX_BATCH` bounds one append transaction without imposing a minimum.
    #[inline(never)]
    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if MAX_BATCH == 0 || MAX_BATCH > COUNT {
            return Err(RxRingError::Count);
        }
        if mmio.reload_pending() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;

        let mut completed = 0;
        while completed < MAX_BATCH {
            let index = wrap_add::<COUNT>(self.recycle_start, completed);
            let bit = 1_u64 << index;
            if self.observed_mask & bit == 0 || !rx_done(self.descriptors[index].word0()) {
                break;
            }
            completed += 1;
        }
        if completed == 0 {
            return Ok(None);
        }
        self.recycle_completed_group(mmio, completed, false, prepare_buffer)
    }}

    /// Rearm and append one observed RX unit, preserving a multi-descriptor
    /// unit's `not-done .. done` completion shape until all of its bytes have
    /// been copied to independent storage.
    pub fn recycle_completed_unit<M, F>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if descriptor_count == 0 || descriptor_count > COUNT {
            return Err(RxRingError::Count);
        }
        self.recycle_completed_group(mmio, descriptor_count, true, prepare_buffer)
    }

    fn recycle_completed_group<M, F>(
        &mut self,
        mmio: &mut M,
        group_size: usize,
        chained_unit: bool,
        mut prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if mmio.reload_pending() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;

        let group_mask = recycle_group_mask::<COUNT>(self.recycle_start, group_size);
        if self.observed_mask & group_mask != group_mask {
            return Ok(None);
        }

        for step in 0..group_size {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let terminal = rx_done(self.descriptors[index].word0());
            let expected_terminal = !chained_unit || step + 1 == group_size;
            if terminal != expected_terminal {
                return Err(RxRingError::Corrupt);
            }
        }
        for step in 0..group_size {
            prepare_buffer(wrap_add::<COUNT>(self.recycle_start, step))?;
        }
        for step in 0..group_size {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let descriptor = &self.descriptors[index];
            let next = if step + 1 < group_size {
                descriptor_address(self.descriptor_base, wrap_add::<COUNT>(index, 1))?
            } else {
                0
            };
            descriptor.publish(
                rx_rearm_word(descriptor.word0()).ok_or(RxRingError::Size)?,
                self.buffer_addresses[index],
                next,
            );
        }

        let head_index = self.recycle_start;
        let head_address = descriptor_address(self.descriptor_base, head_index)?;
        let tail_index = wrap_add::<COUNT>(head_index, group_size - 1);
        let accepted_tail = &self.descriptors[self.accepted_tail];
        if accepted_tail.next_address() != 0 {
            return Err(RxRingError::Corrupt);
        }
        // This type is the sole publication authority. All descriptors in the
        // appended group were observed complete, rearmed and remain
        // unreachable until this old-tail link and the following doorbell.
        accepted_tail.publish_next_address(head_address);
        mmio.fence();
        mmio.request_reload();
        mmio.fence();

        self.pending_tail = Some(tail_index);
        self.observed_mask &= !group_mask;
        self.recycle_start = wrap_add::<COUNT>(self.recycle_start, group_size);
        Ok(Some(RxLiveAppend {
            head_index,
            head_address,
            tail_index,
            descriptor_count: group_size,
        }))
    }

    pub const fn observed_mask(&self) -> u64 {
        self.observed_mask
    }

    /// Whether every descriptor in this ring epoch has returned to software.
    pub const fn all_observed(&self) -> bool {
        let complete_mask = if COUNT == 64 {
            u64::MAX
        } else {
            (1_u64 << COUNT) - 1
        };
        self.observed_mask == complete_mask
    }

    pub const fn recycle_start(&self) -> usize {
        self.recycle_start
    }

    pub const fn accepted_tail(&self) -> usize {
        self.accepted_tail
    }

    pub const fn reload_pending(&self) -> bool {
        self.pending_tail.is_some()
    }

    /// Observe one live-append doorbell edge without waiting.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS]: the complete ROM body at
    /// `0x2f83_8a7e` spins on `RX_CONTROL.APPEND_DESCRIPTOR_RELOAD` with the
    /// exact `0x186a1` bound, then immediately samples `RX_NEXT_DESCRIPTOR`
    /// and repairs `RX_DESCRIPTOR_BASE` from `last->next` when the old
    /// frontier was exhausted. Deferring this suffix until after another RX
    /// processing pass is observably unsafe: the appended half can itself
    /// reach the terminal descriptor first, making `RX_LAST_DESCRIPTOR`
    /// indistinguishable from the no-repair case.
    pub fn poll_pending_reload<M: RxDma>(
        &mut self,
        mmio: &mut M,
    ) -> Result<RxReloadObservation, RxRingError> {
        if self.pending_tail.is_none() {
            return Ok(RxReloadObservation::Settled);
        }
        if mmio.reload_pending() {
            return Ok(RxReloadObservation::Pending);
        }
        self.settle_reload(mmio)?;
        Ok(RxReloadObservation::Settled)
    }

    fn settle_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        let Some(pending_tail) = self.pending_tail else {
            return Ok(());
        };
        if mmio.next_descriptor_low() == 0 {
            let last_low = mmio.last_descriptor_low();
            let last_index = descriptor_index(last_low, self.descriptor_base, COUNT)
                .ok_or(RxRingError::Corrupt)?;
            if last_index != pending_tail {
                let repair_head = self.descriptors[last_index].next_address();
                if repair_head == 0 {
                    return Err(RxRingError::Corrupt);
                }
                mmio.write_descriptor_base(repair_head);
                mmio.fence();
            }
        }
        self.accepted_tail = pending_tail;
        self.pending_tail = None;
        Ok(())
    }
}

fn validate_live_ring_geometry<const COUNT: usize>() -> Result<(), RxRingError> {
    if COUNT < 2 || COUNT > 64 || COUNT % 2 != 0 {
        Err(RxRingError::Count)
    } else {
        Ok(())
    }
}

fn descriptor_address(descriptor_base: u32, index: usize) -> Result<u32, RxRingError> {
    let index = u32::try_from(index).map_err(|_| RxRingError::Overflow)?;
    descriptor_base
        .checked_add(
            index
                .checked_mul(DESCRIPTOR_BYTES)
                .ok_or(RxRingError::Overflow)?,
        )
        .ok_or(RxRingError::Overflow)
}

fn descriptor_index(low_address: u32, descriptor_base: u32, count: usize) -> Option<usize> {
    let base_low = descriptor_base & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
    let offset = low_address.checked_sub(base_low)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    (index < count).then_some(index)
}

fn wrap_add<const COUNT: usize>(index: usize, amount: usize) -> usize {
    (index + amount) % COUNT
}

fn wrap_sub_one<const COUNT: usize>(index: usize) -> usize {
    if index == 0 { COUNT - 1 } else { index - 1 }
}

fn recycle_group_mask<const COUNT: usize>(start: usize, group_size: usize) -> u64 {
    let mut mask = 0_u64;
    for step in 0..group_size {
        mask |= 1_u64 << wrap_add::<COUNT>(start, step);
    }
    mask
}

fn relink_rotated_ring<const COUNT: usize>(
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &[u32; COUNT],
    start: usize,
) -> Result<(), RxRingError> {
    for step in 0..COUNT {
        let index = wrap_add::<COUNT>(start, step);
        let next = if step + 1 < COUNT {
            descriptor_address(descriptor_base, wrap_add::<COUNT>(index, 1))?
        } else {
            0
        };
        let word0 = descriptors[index].word0();
        descriptors[index].publish(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Restores the two guard words required by the recovered RX recycle path.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS] and the preserved Rust transcription
/// in `migration/esp32s31-hybrid-runtime/src/wdev.rs::
/// prepare_rx_recycle_chain`.
///
/// `buffer` is the complete allocation, including the four-byte trailing
/// guard. `capacity` is the byte count published in the DMA descriptor.
pub fn prepare_recycled_buffer(buffer: &mut [u8], capacity: usize) -> Result<(), RxRingError> {
    if capacity < core::mem::size_of::<u32>()
        || capacity
            .checked_add(core::mem::size_of::<u32>())
            .is_none_or(|required| required > buffer.len())
    {
        return Err(RxRingError::Size);
    }
    let sentinel = RX_BUFFER_SENTINEL.to_le_bytes();
    buffer[..sentinel.len()].copy_from_slice(&sentinel);
    buffer[capacity..capacity + sentinel.len()].copy_from_slice(&sentinel);
    Ok(())
}

/// Builds the cold, zero-terminated list used by the recovered S31 RX path.
pub fn build_cold_ring(
    descriptors: &[Descriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    if descriptors.is_empty() {
        return Err(RxRingError::Empty);
    }
    if descriptors.len() != buffer_addresses.len() {
        return Err(RxRingError::Count);
    }
    let word0 = rx_armed_word(buffer_size).ok_or(RxRingError::Size)?;
    let count = u32::try_from(descriptors.len()).map_err(|_| RxRingError::Overflow)?;
    let span = count
        .checked_mul(DESCRIPTOR_BYTES)
        .ok_or(RxRingError::Overflow)?;
    if descriptor_dma_base & 3 != 0 || !dma_range_valid(descriptor_dma_base, span) {
        return Err(RxRingError::Address);
    }
    for &buffer in buffer_addresses {
        if !dma_range_valid(buffer, buffer_size) {
            return Err(RxRingError::Address);
        }
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        let next = if index + 1 < descriptors.len() {
            descriptor_dma_base + (index as u32 + 1) * DESCRIPTOR_BYTES
        } else {
            0
        };
        descriptor.publish(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Publishes a previously built cold ring using the instruction-confirmed
/// fence/high-window/base/enable/fence sequence.
pub fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    if !descriptor_address_valid(descriptor_dma_base) {
        return Err(RxRingError::Address);
    }
    mmio.fence();
    mmio.set_descriptor_high_window(0x02f0);
    mmio.write_descriptor_base(descriptor_dma_base);
    if enable_rx {
        mmio.publish_walker_enable();
    }
    mmio.fence();
    Ok(())
}

/// Opens the RX walker after a cold ring base has already been published.
///
/// The vendor cold path keeps these operations separate:
/// `wDev_AppendRxBlocks` publishes the first descriptor base, while the later
/// `chip_enable` path calls `hal_mac_rx_enable`. Keeping this as a distinct
/// operation preserves that ordering and gives the base register time to
/// settle while the caller completes channel/MAC setup.
pub fn enable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    if mmio.try_enable_walker() {
        Ok(())
    } else {
        Err(RxRingError::Busy)
    }
}

/// Stop the RX walker and confirm that the peripheral released its enable
/// edge before the owner rebuilds descriptor words or links.
///
/// The pinned `hal_mac_rx_disable` body is exactly this bit clear. The fence
/// and readback turn that raw leaf into an explicit Rust ownership boundary:
/// callers may mutate the ring only after this function returns `Ok(())`.
pub fn disable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    if mmio.try_disable_walker() {
        Ok(())
    } else {
        Err(RxRingError::Busy)
    }
}

/// Returns one CPU-owned completed descriptor to the cold/live ring.
pub fn rearm_descriptor(
    descriptor: &Descriptor,
    expected_buffer_address: u32,
    expected_next_address: u32,
) -> Result<(), RxRingError> {
    let word0 = descriptor.word0();
    let capacity = descriptor_size(word0);
    if descriptor.buffer_address() != expected_buffer_address
        || descriptor.next_address() != expected_next_address
        || !dma_range_valid(expected_buffer_address, capacity)
    {
        return Err(RxRingError::Corrupt);
    }
    if !rx_done(word0) {
        return Err(RxRingError::Busy);
    }
    descriptor.write_word0(rx_rearm_word(word0).ok_or(RxRingError::Size)?);
    Ok(())
}
