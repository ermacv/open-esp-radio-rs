//! Typed ownership and publication for the ESP32-S31 RX DMA ring.
//!
//! This module contains descriptor-walker mechanics only. 802.11 frame
//! decoding remains in the MAC crate, while the semantic MMIO operations are
//! defined by [`crate::rx_dma::RxDma`].

use core::sync::atomic::{AtomicU8, Ordering};

use crate::{
    descriptor::{
        BIT_29, BIT_30, BIT_31, DESCRIPTOR_BYTES, Descriptor, LENGTH_MASK, LENGTH_SHIFT, SIZE_MASK,
        descriptor_address_valid, dma_range_valid, length as descriptor_length, rx_armed_word,
        rx_done, rx_rearm_word, size as descriptor_size,
    },
    rx_dma::{RxDma, RxDmaBinding},
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
const RX_EXHAUSTED_REPUBLICATION_ACCEPTED: u32 = 0x8000_0000;
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
    ResetRequired,
}

/// One allocation-free observation of an RX descriptor.
///
/// The snapshot copies volatile words once. It is intended for qualification
/// and fault reporting without exposing the descriptor for mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxDescriptorSnapshot {
    pub index: usize,
    pub address: u32,
    pub word0: u32,
    pub buffer_address: u32,
    pub next_address: u32,
}

/// One ordered, newly completed descriptor interval at the software frontier.
///
/// Descriptors already observed but not yet recycled are skipped, while a
/// not-yet-completed descriptor terminates the snapshot. Consumers must walk
/// `start_index..descriptor_count` in ring order; scanning physical indices
/// from zero reorders packets whenever the live frontier wraps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxCompletedDescriptorFrontier {
    pub start_index: usize,
    pub descriptor_count: usize,
}

/// Compact proof that a zero-terminated RX list is complete and coherent.
///
/// `valid` means the walk starting at `start_index` visited every descriptor
/// exactly once, followed the expected rotated order, found exactly one
/// terminal node at `tail_index`, retained the configured buffer binding and,
/// for a stopped ring, observed an armed ownership word on every node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxRingTopologySnapshot {
    pub descriptor_base: u32,
    pub start_index: usize,
    pub head_address: u32,
    pub head_next_address: u32,
    pub tail_index: usize,
    pub tail_address: u32,
    pub visited_descriptors: usize,
    pub terminal_descriptors: usize,
    pub valid: bool,
}

/// Persistent state of the static RX arena, independent of a movable ring
/// capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RxDmaArenaState {
    Reusable = 0,
    Prepared = 1,
    Live = 2,
    ResetRequired = 3,
}

impl RxDmaArenaState {
    pub(crate) fn load(state: &AtomicU8) -> Self {
        match state.load(Ordering::Acquire) {
            0 => Self::Reusable,
            1 => Self::Prepared,
            2 => Self::Live,
            _ => Self::ResetRequired,
        }
    }

    pub(crate) fn store(self, state: &AtomicU8) {
        state.store(self as u8, Ordering::Release);
    }

    pub(crate) fn claim_prepared(state: &AtomicU8) -> bool {
        state
            .compare_exchange(
                Self::Reusable as u8,
                Self::Prepared as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
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
    buffer_size: u16,
    initial_start: usize,
    accepted_tail: usize,
    retained_next_low: u32,
    retained_last_low: u32,
    binding: RxDmaBinding<'a>,
    lifecycle: Option<&'a AtomicU8>,
}

/// Sole software owner of one running S31 RX descriptor frontier.
///
/// The owner tracks three distinct states recovered from
/// `wDev_AppendRxBlocks`: descriptors observed as CPU-owned, the last tail
/// accepted by hardware, and a future tail whose reload doorbell is still in
/// flight. No allocator, global `wDevCtrl`, C ABI or vendor callback is needed.
///
/// Production backing is static. Dropping or forgetting this movable token
/// therefore never releases memory still visible to DMA, but it also does not
/// stop the walker. Normal role owners must consume it through [`Self::try_stop`];
/// an abandoned token poisons the arena until radio reset.
pub struct RxRingLive<'a, const COUNT: usize> {
    descriptors: &'a [Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &'a [u32; COUNT],
    buffer_size: u16,
    observed_mask: u64,
    recycle_start: usize,
    accepted_tail: usize,
    pending_tail: Option<usize>,
    completion_release_probe_pending: bool,
    // Zero count means no proof. S31 rings are already bounded to 64
    // descriptors, so byte-sized indices avoid inflating every async owner
    // which retains a live ring.
    released_recycle_start: u8,
    released_recycle_count: u8,
    exhausted_republication_head_low: Option<u32>,
    binding: RxDmaBinding<'a>,
    lifecycle: Option<&'a AtomicU8>,
    requires_stop: bool,
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
    buffer_size: u16,
    lifecycle: Option<&'a AtomicU8>,
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

    /// Observe one halted descriptor without recovering mutation authority.
    ///
    /// A halted ring can contain the terminal image of the preceding DMA
    /// epoch. This snapshot is useful for fault evidence before the next
    /// [`Self::prepare`] rebuilds every descriptor.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Rebuild the stopped ring for a new ownership epoch.
    ///
    /// Failure returns the halted authority even if hardware observations or
    /// descriptor preparation rejected the attempt. The caller can then
    /// retry after a higher-level reset without stealing the static storage.
    pub(crate) fn prepare_owned<M, F>(
        self,
        mmio: &mut M,
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<RxRingStopped<'a, COUNT>, (Self, RxRingError)>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        if u32::from(self.buffer_size) != buffer_size {
            return Err((self, RxRingError::Size));
        }
        let lifecycle = self.lifecycle;
        match RxRingStopped::prepare_inner(
            mmio,
            self.descriptors,
            self.descriptor_base,
            self.buffer_addresses,
            buffer_size,
            prepare_buffer,
            self.lifecycle,
        ) {
            Ok(stopped) => Ok(stopped),
            Err(error) => {
                if mmio.walker_enabled()
                    && let Some(lifecycle) = lifecycle
                {
                    // Preparation failed without proving that the walker is
                    // stopped. The returned token preserves arena ownership
                    // for reset diagnostics, but must never authorize another
                    // descriptor rebuild in this radio epoch.
                    RxDmaArenaState::ResetRequired.store(lifecycle);
                }
                Err((self, error))
            }
        }
    }

    /// Raw/model rebuild with caller-provided buffer preparation.
    ///
    /// Production code must use `RxDmaStorage::prepare_halted`, which binds
    /// every descriptor to the matching arena buffer and restores its guard
    /// words before publication.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
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
        self.prepare_owned(mmio, buffer_size, prepare_buffer)
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
            buffer_size: self.buffer_size,
            lifecycle: self.lifecycle,
        }
    }

    /// Stops the walker, prepares all buffers and publishes one physical-order
    /// cold list beginning at the retained hardware cursor, or descriptor zero
    /// when the preceding list was exhausted.
    ///
    /// `prepare_buffer` must restore any buffer-side DMA contract for `index`;
    /// for the S31 ROM layout this means the two `0xdead_beef` sentinels. It is
    /// invoked only while the walker is confirmed stopped.
    ///
    /// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
    /// ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR]. Live append/base repair is
    /// qualified by HIL_OPEN_RX_LIVE_APPEND_2026_07_27. A retained nonzero
    /// NEXT cursor is authoritative because WALKER_ENABLE does not clear it;
    /// the rebuilt zero-terminated list is rotated to that exact descriptor.
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a [u32; COUNT],
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        Self::prepare_inner(
            mmio,
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            prepare_buffer,
            None,
        )
    }

    /// Build a ring from raw addresses for a target-only validation harness.
    ///
    /// # Safety
    ///
    /// Every descriptor and addressed buffer must remain allocated, aligned
    /// and exclusively DMA-owned until the returned ring is confirmed halted.
    /// Shipping code must use [`crate::rx_storage::RxDmaStorage::prepare_ring`]
    /// instead, which establishes that proof from a static arena.
    #[cfg(all(target_pointer_width = "32", feature = "validation-raw-dma"))]
    #[allow(
        unsafe_code,
        reason = "raw target constructor makes its DMA lifetime contract explicit"
    )]
    pub unsafe fn prepare<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a [u32; COUNT],
        buffer_size: u32,
        prepare_buffer: F,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        Self::prepare_inner(
            mmio,
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            prepare_buffer,
            None,
        )
    }

    pub(crate) fn prepare_inner<M, F>(
        mmio: &mut M,
        descriptors: &'a [Descriptor; COUNT],
        descriptor_base: u32,
        buffer_addresses: &'a [u32; COUNT],
        buffer_size: u32,
        mut prepare_buffer: F,
        lifecycle: Option<&'a AtomicU8>,
    ) -> Result<Self, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        validate_live_ring_geometry::<COUNT>()?;
        let buffer_size_u16 = u16::try_from(buffer_size).map_err(|_| RxRingError::Size)?;
        if rx_armed_word(buffer_size).is_none() {
            return Err(RxRingError::Size);
        }
        let binding = RxDmaBinding::new(descriptors, descriptor_base).ok_or(RxRingError::Count)?;
        if mmio.walker_enabled() {
            disable_receive_inner(mmio)?;
        }
        // NEXT and LAST belong to the walker. They are stable handoff
        // frontiers only after WALKER_ENABLE has been confirmed clear.
        mmio.fence();
        let retained_next_low = mmio.next_descriptor_low();
        let retained_last_low = mmio.last_descriptor_low();
        // Disabling WALKER_ENABLE does not reset RX_NEXT_DESCRIPTOR. A later
        // enable resumes that cursor when it is nonzero, even if software has
        // written a different BASE. Rebuild the zero-terminated list from the
        // retained cursor so hardware and the Rust recycle frontier name the
        // same first descriptor. An exhausted cursor is zero; in that case a
        // fresh BASE publication legitimately starts from physical slot zero.
        let initial_start = if retained_next_low == 0 {
            0
        } else {
            // A nonzero cursor is what hardware resumes from. Falling back to
            // BASE would publish a topology which Rust considers valid while
            // the walker follows an unrelated address, so fail closed.
            descriptor_index(retained_next_low, descriptor_base, COUNT)
                .ok_or(RxRingError::Corrupt)?
        };

        for index in 0..COUNT {
            prepare_buffer(index)?;
        }
        build_cold_ring_inner(descriptors, descriptor_base, buffer_addresses, buffer_size)?;
        relink_rotated_ring(
            descriptors,
            descriptor_base,
            buffer_addresses,
            initial_start,
        )?;
        let head = descriptor_address(descriptor_base, initial_start)?;
        publish_cold_ring_inner(mmio, &binding, head, false)?;

        Ok(Self {
            descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size: buffer_size_u16,
            initial_start,
            accepted_tail: wrap_sub_one::<COUNT>(initial_start),
            retained_next_low,
            retained_last_low,
            binding,
            lifecycle,
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

    pub const fn retained_next_low(&self) -> u32 {
        self.retained_next_low
    }

    /// Observe one descriptor without exposing mutation authority.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Validate the complete stopped list before it becomes hardware-owned.
    pub fn topology_snapshot(&self) -> RxRingTopologySnapshot {
        ring_topology_snapshot(
            self.descriptors,
            self.descriptor_base,
            self.buffer_addresses,
            self.initial_start,
            true,
        )
    }

    /// Opens the walker and consumes the stopped-state authority.
    ///
    /// The caller owns any platform-specific settle delay between
    /// [`prepare`](Self::prepare) and this edge.
    pub fn try_start<M: RxDma>(
        self,
        mmio: &mut M,
    ) -> Result<RxRingLive<'a, COUNT>, (Self, RxRingError)> {
        if let Some(lifecycle) = self.lifecycle {
            match RxDmaArenaState::load(lifecycle) {
                RxDmaArenaState::Prepared => {}
                RxDmaArenaState::ResetRequired => {
                    return Err((self, RxRingError::ResetRequired));
                }
                RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                    return Err((self, RxRingError::Busy));
                }
            }
        }
        if !self.topology_snapshot().valid {
            return Err((self, RxRingError::Corrupt));
        }
        if let Err(error) = enable_receive_inner(mmio, &self.binding) {
            if mmio.walker_enabled()
                && let Some(lifecycle) = self.lifecycle
            {
                // A failed start which nevertheless observes WALKER_ENABLE
                // cannot return an honestly stopped capability. Keep the
                // static backing quarantined; the returned value exists only
                // to preserve diagnostics/ownership until radio reset.
                RxDmaArenaState::ResetRequired.store(lifecycle);
            }
            return Err((self, error));
        }
        if let Some(lifecycle) = self.lifecycle {
            RxDmaArenaState::Live.store(lifecycle);
        }
        Ok(RxRingLive {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            observed_mask: 0,
            recycle_start: self.initial_start,
            accepted_tail: self.accepted_tail,
            pending_tail: None,
            completion_release_probe_pending: false,
            released_recycle_start: 0,
            released_recycle_count: 0,
            exhausted_republication_head_low: None,
            binding: self.binding,
            lifecycle: self.lifecycle,
            requires_stop: true,
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

    /// Observe one live descriptor without transferring its ownership.
    pub fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        descriptor_snapshot(self.descriptors, self.descriptor_base, index)
    }

    /// Walk the links from the current recycle frontier.
    ///
    /// Unlike the stopped proof this does not validate ownership words: the
    /// hardware may be changing those words while the snapshot is taken.
    /// `valid` still proves the address, link, buffer and terminal topology
    /// observed during this finite walk.
    pub fn topology_snapshot(&self) -> RxRingTopologySnapshot {
        ring_topology_snapshot(
            self.descriptors,
            self.descriptor_base,
            self.buffer_addresses,
            self.recycle_start,
            false,
        )
    }

    /// Irreversibly poison the static arena after an ownership invariant can
    /// no longer be proven. The live token may still be retained for reset
    /// diagnostics, but no later stop observation can make this arena reusable.
    pub fn require_reset(&mut self) {
        if let Some(lifecycle) = self.lifecycle {
            RxDmaArenaState::ResetRequired.store(lifecycle);
        }
    }

    /// Stop the DMA walker and consume the live frontier authority.
    ///
    /// A failed hardware confirmation returns the complete live owner. This
    /// prevents lifecycle code from rebuilding descriptors while DMA may
    /// still own them.
    pub fn try_stop<M: RxDma>(
        mut self,
        mmio: &mut M,
    ) -> Result<RxRingHalted<'a, COUNT>, (Self, RxRingError)> {
        if let Err(error) = disable_receive_inner(mmio) {
            return Err((self, error));
        }
        let halted = RxRingHalted {
            descriptors: self.descriptors,
            descriptor_base: self.descriptor_base,
            buffer_addresses: self.buffer_addresses,
            buffer_size: self.buffer_size,
            lifecycle: self.lifecycle,
        };
        if let Some(lifecycle) = self.lifecycle
            && RxDmaArenaState::load(lifecycle) != RxDmaArenaState::ResetRequired
        {
            // The halted token still owns the arena. Only that exact token may
            // rebuild the next epoch; a new root prepare must remain blocked.
            RxDmaArenaState::Prepared.store(lifecycle);
        }
        self.requires_stop = false;
        Ok(halted)
    }

    /// Snapshot the contiguous, newly completed interval after the observed
    /// prefix at the current recycle frontier.
    ///
    /// The returned count is a finite receive epoch. A caller may subsequently
    /// take and recycle exactly this many descriptors without accidentally
    /// extending the same service pass to descriptors completed after the
    /// snapshot. This is important when recycled descriptors can already be
    /// filled again while the task is still draining an RX-success wake.
    pub fn completed_descriptor_frontier(&self) -> RxCompletedDescriptorFrontier {
        let mut observed = 0;
        while observed < COUNT {
            let index = wrap_add::<COUNT>(self.recycle_start, observed);
            let bit = 1_u64 << index;
            if self.observed_mask & bit == 0 {
                break;
            }
            observed += 1;
        }
        if observed == COUNT {
            return RxCompletedDescriptorFrontier::default();
        }

        let start_index = wrap_add::<COUNT>(self.recycle_start, observed);
        let mut completed = 0;
        while observed + completed < COUNT {
            let index = wrap_add::<COUNT>(start_index, completed);
            let bit = 1_u64 << index;
            if self.observed_mask & bit != 0 || !rx_done(self.descriptors[index].word0()) {
                break;
            }
            completed += 1;
        }
        RxCompletedDescriptorFrontier {
            start_index,
            descriptor_count: completed,
        }
    }

    /// Number of newly completed descriptors beginning exactly at the recycle
    /// frontier. This legacy snapshot deliberately stops at an already
    /// observed descriptor; finite staged consumers should use
    /// [`Self::completed_descriptor_frontier`] to continue after that prefix.
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
        self.completed_unit_frontier_with_limit(COUNT, |_| true)
    }

    /// Snapshot a complete RX unit only through the descriptor frontier which
    /// hardware has published in `RX_LAST_DESCRIPTOR`.
    ///
    /// SOURCE[`libpp:wdevProcessRxSucDataAll`]: the complete vendor walker
    /// snapshots `hal_mac_rx_get_last_dscr` before touching the software list,
    /// processes through that descriptor, then refreshes the snapshot before
    /// extending the same pass. `RX_DONE` alone is therefore not sufficient
    /// ownership evidence: its store may be observed before the MAC has
    /// finished advancing its descriptor frontier. Returning such a node to
    /// the live append list can overwrite `next` while hardware still needs
    /// it, terminating RX after the first frame.
    pub fn completed_unit_frontier_through_with<F>(
        &self,
        last_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        let Some(last_index) = descriptor_index(last_descriptor_low, self.descriptor_base, COUNT)
        else {
            return RxCompletedUnitFrontier::default();
        };
        let last_distance = (last_index + COUNT - self.recycle_start) % COUNT;
        let accepted_distance = (self.accepted_tail + COUNT - self.recycle_start) % COUNT;
        if last_distance > accepted_distance {
            return RxCompletedUnitFrontier::default();
        }
        let descriptor_limit = last_distance + 1;
        self.completed_unit_frontier_with_limit(descriptor_limit, nonterminal_consumed)
    }

    /// Snapshot only the first complete unit through the hardware LAST
    /// frontier. Unlike [`Self::completed_unit_frontier_through_with`], a run
    /// of single-descriptor frames is deliberately truncated to one unit so
    /// callers can prove and recycle each link before taking the next owner.
    pub fn first_completed_unit_frontier_through_with<F>(
        &self,
        last_descriptor_low: u32,
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        let frontier =
            self.completed_unit_frontier_through_with(last_descriptor_low, nonterminal_consumed);
        if frontier.unit_count > 1 {
            RxCompletedUnitFrontier {
                unit_count: 1,
                descriptor_count: 1,
            }
        } else {
            frontier
        }
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
        nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        self.completed_unit_frontier_with_limit(COUNT, nonterminal_consumed)
    }

    fn completed_unit_frontier_with_limit<F>(
        &self,
        descriptor_limit: usize,
        mut nonterminal_consumed: F,
    ) -> RxCompletedUnitFrontier
    where
        F: FnMut(usize) -> bool,
    {
        if COUNT == 0 || COUNT > 64 {
            return RxCompletedUnitFrontier::default();
        }
        let descriptor_limit = descriptor_limit.min(COUNT);
        if descriptor_limit == 0 {
            return RxCompletedUnitFrontier::default();
        }
        let first_index = self.recycle_start;
        if self.observed_mask & (1_u64 << first_index) != 0 {
            return RxCompletedUnitFrontier::default();
        }
        if rx_done(self.descriptors[first_index].word0()) {
            let mut completed = 0;
            while completed < descriptor_limit {
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
        for step in 1..descriptor_limit {
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
    pub(crate) fn take_completed_unit_owned(
        &mut self,
        descriptor_limit: usize,
    ) -> Result<Option<RxCompletedUnit>, RxRingError> {
        if COUNT == 0 || COUNT > 64 || descriptor_limit == 0 || descriptor_limit > COUNT {
            return Ok(None);
        }
        let first_index = self.recycle_start;
        let first_bit = 1_u64 << first_index;
        if self.observed_mask & first_bit != 0 {
            return Ok(None);
        }
        let first_word0 = self.validate_descriptor_image(first_index)?;
        let first_length = descriptor_length(first_word0);
        if rx_done(first_word0) {
            let encoded_length = first_length;
            let descriptor_address = descriptor_address(self.descriptor_base, first_index)?;
            let segment_length = u16::try_from(first_length).map_err(|_| RxRingError::Size)?;
            self.observed_mask |= first_bit;
            return Ok(Some(RxCompletedUnit {
                head_index: first_index,
                descriptor_count: 1,
                descriptor_address,
                staged_word0: (first_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length: first_length as usize,
                segment_lengths: RxCompletedUnitLengths::Single(segment_length),
            }));
        }

        let mut segment_lengths = [0_u16; 64];
        segment_lengths[0] = u16::try_from(first_length).map_err(|_| RxRingError::Size)?;
        let mut total_length = first_length as usize;
        for step in 1..descriptor_limit {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let bit = 1_u64 << index;
            if self.observed_mask & bit != 0 {
                return Ok(None);
            }
            let word0 = match self.validate_descriptor_image(index) {
                Ok(word0) => word0,
                Err(error) => {
                    self.require_reset();
                    return Err(error);
                }
            };
            let length = descriptor_length(word0);
            segment_lengths[step] = u16::try_from(length).map_err(|_| RxRingError::Size)?;
            total_length = total_length
                .checked_add(length as usize)
                .ok_or(RxRingError::Overflow)?;
            if !rx_done(word0) {
                continue;
            }
            if total_length > SIZE_MASK as usize {
                return Err(RxRingError::Size);
            }
            let descriptor_count = step + 1;
            let group_mask = recycle_group_mask::<COUNT>(self.recycle_start, descriptor_count);
            let encoded_length = u32::try_from(total_length).map_err(|_| RxRingError::Overflow)?;
            let descriptor_address = descriptor_address(self.descriptor_base, self.recycle_start)?;
            self.observed_mask |= group_mask;
            return Ok(Some(RxCompletedUnit {
                head_index: self.recycle_start,
                descriptor_count,
                descriptor_address,
                staged_word0: (first_word0 & !(SIZE_MASK | LENGTH_MASK))
                    | encoded_length
                    | (encoded_length << LENGTH_SHIFT)
                    | BIT_30
                    | BIT_31,
                total_length,
                segment_lengths: RxCompletedUnitLengths::Chained(segment_lengths),
            }));
        }
        Ok(None)
    }

    /// Raw/model completion transfer without a storage-bound buffer owner.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn take_completed_unit(
        &mut self,
        descriptor_limit: usize,
    ) -> Result<Option<RxCompletedUnit>, RxRingError> {
        self.take_completed_unit_owned(descriptor_limit)
    }

    crate::place_rx_hot_path! {
    /// Takes one newly completed descriptor exactly once for this ring epoch.
    ///
    /// Kept in internal SRAM for PSRAM-code profiles: this is invoked once for
    /// every descriptor slot on every receive poll. HIL at HE20 showed that
    /// executing the complete poll/copy path from PSRAM capped useful UDP RX
    /// near 65 Mbit/s.
    #[inline(never)]
    pub(crate) fn take_completed_owned(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
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
        self.validate_descriptor_image(index).ok()?;
        let descriptor_address = descriptor_address(self.descriptor_base, index).ok()?;
        self.observed_mask |= bit;
        Some(RxCompletedDescriptor {
            index,
            descriptor_address,
            word0,
            next_descriptor_address: descriptor.next_address(),
        })
    }}

    /// Raw/model descriptor transfer without a storage-bound buffer owner.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn take_completed(&mut self, index: usize) -> Option<RxCompletedDescriptor> {
        self.take_completed_owned(index)
    }

    pub(crate) fn validate_completed_descriptor(&self, index: usize) -> Result<bool, RxRingError> {
        let descriptor = self.descriptors.get(index).ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        if !rx_done(word0) {
            return Ok(false);
        }
        self.validate_descriptor_image(index)?;
        Ok(true)
    }

    fn validate_descriptor_image(&self, index: usize) -> Result<u32, RxRingError> {
        let descriptor = self.descriptors.get(index).ok_or(RxRingError::Count)?;
        let word0 = descriptor.word0();
        let expected_size = u32::from(self.buffer_size);
        if descriptor_size(word0) != expected_size
            || descriptor_length(word0) > expected_size
            || descriptor.buffer_address() != self.buffer_addresses[index]
        {
            return Err(RxRingError::Corrupt);
        }
        let terminal_tail = self.pending_tail.unwrap_or(self.accepted_tail);
        let expected_next = if index == terminal_tail {
            0
        } else {
            descriptor_address(self.descriptor_base, wrap_add::<COUNT>(index, 1))?
        };
        if descriptor.next_address() != expected_next {
            return Err(RxRingError::Corrupt);
        }
        Ok(word0)
    }

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
    pub(crate) fn recycle_completed_half_owned<M, F>(
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

    /// Raw/model half-ring recycle with caller-provided buffer preparation.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn recycle_completed_half<M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_half_owned(mmio, prepare_buffer)
    }

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
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
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
    /// Unlike the fixed-size raw/model batch operation, this does not wait for a
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
    pub(crate) fn recycle_completed_prefix_owned<const MAX_BATCH: usize, M, F>(
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
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
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

    /// Raw/model prefix recycle with caller-provided buffer preparation.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M, F>(
        &mut self,
        mmio: &mut M,
        prepare_buffer: F,
    ) -> Result<Option<RxLiveAppend>, RxRingError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        self.recycle_completed_prefix_owned::<MAX_BATCH, _, _>(mmio, prepare_buffer)
    }

    /// Rearm and append one observed RX unit, preserving a multi-descriptor
    /// unit's `not-done .. done` completion shape until all of its bytes have
    /// been copied to independent storage.
    pub(crate) fn recycle_completed_unit_owned<M, F>(
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

    /// Raw/model unit recycle with caller-provided buffer preparation.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
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
        self.recycle_completed_unit_owned(mmio, descriptor_count, prepare_buffer)
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
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
            return Ok(None);
        }
        self.settle_reload(mmio)?;

        let group_mask = recycle_group_mask::<COUNT>(self.recycle_start, group_size);
        if self.observed_mask & group_mask != group_mask {
            return Ok(None);
        }

        for step in 0..group_size {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let word0 = match self.validate_descriptor_image(index) {
                Ok(word0) => word0,
                Err(error) => {
                    self.require_reset();
                    return Err(error);
                }
            };
            let terminal = rx_done(word0);
            let expected_terminal = !chained_unit || step + 1 == group_size;
            if terminal != expected_terminal {
                self.require_reset();
                return Err(RxRingError::Corrupt);
            }
            // Validate every value needed by the later publication before a
            // buffer guard is modified. Hardware is already excluded from
            // the observed group, so these images remain stable until rearm.
            rx_rearm_word(word0).ok_or(RxRingError::Size)?;
            if step + 1 < group_size {
                descriptor_address(self.descriptor_base, wrap_add::<COUNT>(index, 1))?;
            }
        }
        let head_index = self.recycle_start;
        let head_address = descriptor_address(self.descriptor_base, head_index)?;
        let tail_index = wrap_add::<COUNT>(head_index, group_size - 1);
        let accepted_distance = (self.accepted_tail + COUNT - self.recycle_start) % COUNT;
        if group_size - 1 > accepted_distance {
            // A completed-looking descriptor beyond the accepted hardware
            // tail is not part of the live list. Reject the complete
            // transaction before modifying buffer guards or descriptor words.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        let append_to_empty = tail_index == self.accepted_tail;
        if !append_to_empty && self.descriptors[self.accepted_tail].next_address() != 0 {
            // The accepted tail is the only legal append point. Validate it
            // before the first irreversible buffer/descriptor mutation so a
            // corrupt list cannot become a partially rearmed live arena.
            self.require_reset();
            return Err(RxRingError::Corrupt);
        }
        // RX_DONE is CPU ownership of the payload, but not by itself proof
        // that the walker has fetched this descriptor's link word. Enforce
        // that distinction at the common mutation boundary so every role
        // (STA join/WPA2, scanner, monitor and AP) is protected even when its
        // higher-level service path does not perform an earlier probe.
        if (usize::from(self.released_recycle_start) != self.recycle_start
            || usize::from(self.released_recycle_count) != group_size)
            && !self.observe_current_completed_unit_link_release(mmio, group_size)
        {
            return Ok(None);
        }
        self.released_recycle_count = 0;
        for step in 0..group_size {
            if let Err(error) = prepare_buffer(wrap_add::<COUNT>(self.recycle_start, step)) {
                // The completed-unit token has already been consumed and an
                // earlier callback may have changed buffer-side guards. That
                // partial transaction cannot be reconstructed from the next
                // service pass, so fail closed instead of leaving a silent
                // observed-mask dead-end.
                self.require_reset();
                return Err(error);
            }
        }
        for step in 0..group_size {
            let index = wrap_add::<COUNT>(self.recycle_start, step);
            let descriptor = &self.descriptors[index];
            let next = if step + 1 < group_size {
                descriptor_address(self.descriptor_base, wrap_add::<COUNT>(index, 1))?
            } else {
                0
            };
            descriptor.publish_owned(
                rx_rearm_word(descriptor.word0()).ok_or(RxRingError::Size)?,
                self.buffer_addresses[index],
                next,
            );
        }

        // `wDev_DiscardFrame` advances the software head before calling
        // `wDev_AppendRxBlocks`. If the discarded unit was also the accepted
        // tail, that advance makes the old list empty. The vendor append path
        // then publishes the returned chain directly through RX_DESCRIPTOR_BASE;
        // it must not link the descriptor to itself and ring the reload
        // doorbell. `accepted_tail` is our tail authority, so membership of
        // that exact terminal node in this prefix is the equivalent proof.
        if append_to_empty {
            mmio.fence();
            mmio.write_descriptor_base(&self.binding, head_address);
            mmio.fence();
            self.accepted_tail = tail_index;
            self.pending_tail = None;
            // A terminal-only list can be filled and exhausted without a
            // second usable RX-success edge. The vendor worker continues to
            // inspect its durable software list after this direct BASE path.
            // Preserve that behavior explicitly instead of relying on an IRQ
            // from a walker which may already have reached NEXT=0 again.
            self.exhausted_republication_head_low =
                Some(head_address & RX_DESCRIPTOR_ADDRESS_LOW_MASK);
            self.observed_mask &= !group_mask;
            // The software list was empty before this append. Its new head is
            // therefore the returned chain's head, not the physical slot
            // following its tail. `wDev_DiscardFrame` first stores the old
            // `head->next` (null here) into `wDevCtrl.head`, after which the
            // empty branch of `wDev_AppendRxBlocks` stores its argument head
            // back into `wDevCtrl.head` and RX_DESCRIPTOR_BASE. Advancing to
            // the following slot would make software skip the very list that
            // hardware has just started consuming.
            self.recycle_start = head_index;
            return Ok(Some(RxLiveAppend {
                head_index,
                head_address,
                tail_index,
                descriptor_count: group_size,
            }));
        }
        let accepted_tail = &self.descriptors[self.accepted_tail];
        // This type is the sole publication authority. All descriptors in the
        // appended group were observed complete, rearmed and remain
        // unreachable until this old-tail link and the following doorbell.
        accepted_tail.publish_next_address(head_address);
        mmio.fence();
        mmio.request_reload(&self.binding);
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

    /// Whether a completed nonterminal descriptor still awaits proof that the
    /// walker latched its old link before software may rewrite it for recycle.
    pub const fn completion_release_probe_pending(&self) -> bool {
        self.completion_release_probe_pending
    }

    /// Prove that hardware no longer needs the terminal descriptor's current
    /// `next` word before a completed unit is taken for recycle.
    ///
    /// `RX_DONE` and LAST can become visible before the walker has fetched the
    /// descriptor link. If this unit is exactly the sampled LAST and its tail
    /// has a nonzero successor, NEXT must name that successor. A later LAST
    /// proves that hardware already passed the unit only when every
    /// intervening descriptor is terminal-complete; a zero successor is a
    /// real exhausted-list terminal and deliberately requires no NEXT proof.
    fn observe_completed_unit_link_release_from_snapshot(
        &mut self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        self.completed_unit_link_released_from_ordered_snapshot(
            last_descriptor_low,
            next_descriptor_low,
            descriptor_count,
        )
    }

    /// Sample the hardware-owned LAST/NEXT pair and prove release of the
    /// current completed unit's link word.
    ///
    /// Production callers deliberately cannot supply the LAST value: a bare
    /// integer is not ownership evidence and could otherwise cache a forged
    /// release which a later storage-bound recycle would trust.
    pub fn observe_current_completed_unit_link_release<M: RxDma>(
        &mut self,
        mmio: &mut M,
        descriptor_count: usize,
    ) -> bool {
        mmio.with_ordered_cursor(|cursor| {
            self.observe_completed_unit_link_release_from_snapshot(
                cursor.last_descriptor_low(),
                cursor.next_descriptor_low(),
                descriptor_count,
            )
        })
    }

    /// Raw/model release proof from an explicitly supplied LAST snapshot.
    #[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
    pub fn observe_completed_unit_link_release<M: RxDma>(
        &mut self,
        mmio: &mut M,
        last_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        let next_descriptor_low = mmio.next_descriptor_low();
        mmio.fence();
        self.observe_completed_unit_link_release_from_snapshot(
            last_descriptor_low,
            next_descriptor_low,
            descriptor_count,
        )
    }

    fn completed_unit_link_released_from_ordered_snapshot(
        &mut self,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        descriptor_count: usize,
    ) -> bool {
        if descriptor_count == 0 || descriptor_count > COUNT {
            self.completion_release_probe_pending = false;
            self.released_recycle_count = 0;
            return false;
        }
        let tail_index = wrap_add::<COUNT>(self.recycle_start, descriptor_count - 1);
        let Some(last_index) = descriptor_index(last_descriptor_low, self.descriptor_base, COUNT)
        else {
            self.completion_release_probe_pending = false;
            self.released_recycle_count = 0;
            return false;
        };
        let tail_next_low =
            self.descriptors[tail_index].next_address() & RX_DESCRIPTOR_ADDRESS_LOW_MASK;
        let tail_distance = descriptor_count - 1;
        let last_distance = (last_index + COUNT - self.recycle_start) % COUNT;
        let accepted_distance = (self.accepted_tail + COUNT - self.recycle_start) % COUNT;
        let later_completed_frontier = last_distance > tail_distance
            && last_distance <= accepted_distance
            && (tail_distance + 1..=last_distance).all(|distance| {
                rx_done(self.descriptors[wrap_add::<COUNT>(self.recycle_start, distance)].word0())
            });
        let released = tail_next_low == 0
            || (last_distance == tail_distance && next_descriptor_low == tail_next_low)
            || later_completed_frontier;
        self.completion_release_probe_pending = !released;
        if released {
            self.released_recycle_start = self.recycle_start as u8;
            self.released_recycle_count = descriptor_count as u8;
        } else {
            self.released_recycle_count = 0;
        }
        released
    }

    /// Whether direct BASE publication of a previously empty software list
    /// requires one durable follow-up ownership probe.
    pub const fn exhausted_republication_probe_pending(&self) -> bool {
        self.exhausted_republication_head_low.is_some()
    }

    /// Observe whether the walker has accepted a directly republished head.
    ///
    /// Time is not ownership evidence. Keep the cooperative probe armed while
    /// NEXT remains zero: the BASE store may not yet have reached the walker,
    /// and an exhausted walker cannot provide a later interrupt edge. Any
    /// matching NEXT proves that hardware has fetched the published head, but
    /// it does not immediately retire the probe. One further cooperative
    /// observation closes the race where that head completes while the task
    /// is still consuming the IRQ which exhausted the previous list. After
    /// that yield, either the completion is visible to the ordinary
    /// LAST-bounded frontier or a later completion produces a fresh IRQ. A
    /// different nonzero cursor may be the retained image of the exhausted
    /// list and is not an ownership edge.
    pub fn observe_exhausted_republication<M: RxDma>(&mut self, mmio: &mut M) {
        let Some(probe) = self.exhausted_republication_head_low else {
            return;
        };
        if probe & RX_EXHAUSTED_REPUBLICATION_ACCEPTED != 0 {
            self.exhausted_republication_head_low = None;
        } else if mmio.next_descriptor_low() == probe {
            self.exhausted_republication_head_low =
                Some(probe | RX_EXHAUSTED_REPUBLICATION_ACCEPTED);
        }
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
        if mmio.try_with_reload_settled(|_settled| ()).is_none() {
            return Ok(RxReloadObservation::Pending);
        }
        self.settle_reload(mmio)?;
        Ok(RxReloadObservation::Settled)
    }

    fn settle_reload<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RxRingError> {
        let Some(pending_tail) = self.pending_tail else {
            return Ok(());
        };
        let repair_head = mmio.with_ordered_cursor(|cursor| {
            if cursor.next_descriptor_low() != 0 {
                return Ok(None);
            }
            let last_index =
                descriptor_index(cursor.last_descriptor_low(), self.descriptor_base, COUNT)
                    .ok_or(RxRingError::Corrupt)?;
            if last_index == pending_tail {
                return Ok(None);
            }
            {
                // Once the reload doorbell clears, an exhausted walker can
                // only have stopped at the old accepted tail before seeing
                // its new link, or at the new pending tail after traversing
                // the append. Any other in-arena LAST value contradicts the
                // single zero-terminated list and cannot authorize BASE
                // repair.
                if last_index != self.accepted_tail {
                    return Err(RxRingError::Corrupt);
                }
                let repair_head = self.descriptors[last_index].next_address();
                if repair_head == 0
                    || descriptor_index_full(repair_head, self.descriptor_base, COUNT).is_none()
                {
                    return Err(RxRingError::Corrupt);
                }
                Ok(Some(repair_head))
            }
        });
        let repair_head = match repair_head {
            Ok(repair_head) => repair_head,
            Err(error) => {
                self.require_reset();
                return Err(error);
            }
        };
        if let Some(repair_head) = repair_head {
            mmio.write_descriptor_base(&self.binding, repair_head);
            mmio.fence();
        }
        self.accepted_tail = pending_tail;
        self.pending_tail = None;
        Ok(())
    }
}

impl<const COUNT: usize> Drop for RxRingLive<'_, COUNT> {
    fn drop(&mut self) {
        if self.requires_stop
            && let Some(lifecycle) = self.lifecycle
        {
            RxDmaArenaState::ResetRequired.store(lifecycle);
        }
    }
}

fn validate_live_ring_geometry<const COUNT: usize>() -> Result<(), RxRingError> {
    if COUNT < 2 || COUNT > 64 || !COUNT.is_multiple_of(2) {
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

fn descriptor_snapshot<const COUNT: usize>(
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    index: usize,
) -> Option<RxDescriptorSnapshot> {
    let descriptor = descriptors.get(index)?;
    Some(RxDescriptorSnapshot {
        index,
        address: descriptor_address(descriptor_base, index).ok()?,
        word0: descriptor.word0(),
        buffer_address: descriptor.buffer_address(),
        next_address: descriptor.next_address(),
    })
}

fn descriptor_index_full(address: u32, descriptor_base: u32, count: usize) -> Option<usize> {
    let offset = address.checked_sub(descriptor_base)?;
    if offset % DESCRIPTOR_BYTES != 0 {
        return None;
    }
    let index = usize::try_from(offset / DESCRIPTOR_BYTES).ok()?;
    (index < count).then_some(index)
}

fn ring_topology_snapshot<const COUNT: usize>(
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    buffer_addresses: &[u32; COUNT],
    start: usize,
    require_armed: bool,
) -> RxRingTopologySnapshot {
    let tail_index = if COUNT == 0 || start >= COUNT {
        0
    } else {
        wrap_sub_one::<COUNT>(start)
    };
    let head_address = descriptor_address(descriptor_base, start).unwrap_or(0);
    let tail_address = descriptor_address(descriptor_base, tail_index).unwrap_or(0);
    let mut snapshot = RxRingTopologySnapshot {
        descriptor_base,
        start_index: start,
        head_address,
        head_next_address: 0,
        tail_index,
        tail_address,
        visited_descriptors: 0,
        terminal_descriptors: 0,
        valid: false,
    };
    if validate_live_ring_geometry::<COUNT>().is_err()
        || start >= COUNT
        || !descriptor_address_valid(descriptor_base)
    {
        return snapshot;
    }

    let mut visited = 0_u64;
    let mut index = start;
    for step in 0..COUNT {
        let bit = 1_u64 << index;
        if visited & bit != 0 {
            return snapshot;
        }
        visited |= bit;
        snapshot.visited_descriptors += 1;

        let descriptor = &descriptors[index];
        let word0 = descriptor.word0();
        let buffer_address = descriptor.buffer_address();
        let next_address = descriptor.next_address();
        if step == 0 {
            snapshot.head_next_address = next_address;
        }
        if buffer_address != buffer_addresses[index] {
            return snapshot;
        }
        if require_armed {
            let capacity = descriptor_size(word0);
            if capacity == 0
                || !dma_range_valid(buffer_address, capacity)
                || descriptor_length(word0) != capacity
                || word0 & BIT_31 == 0
                || word0 & (BIT_29 | BIT_30) != 0
            {
                return snapshot;
            }
        }

        if next_address == 0 {
            snapshot.terminal_descriptors += 1;
            if step + 1 != COUNT || index != tail_index {
                return snapshot;
            }
            snapshot.valid =
                snapshot.visited_descriptors == COUNT && snapshot.terminal_descriptors == 1;
            return snapshot;
        }
        if step + 1 == COUNT {
            return snapshot;
        }
        let Some(next_index) = descriptor_index_full(next_address, descriptor_base, COUNT) else {
            return snapshot;
        };
        let expected_index = wrap_add::<COUNT>(index, 1);
        if next_index != expected_index {
            return snapshot;
        }
        index = next_index;
    }
    snapshot
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
        descriptors[index].publish_owned(word0, buffer_addresses[index], next);
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
fn build_cold_ring_inner(
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
        descriptor.publish_owned(word0, buffer_addresses[index], next);
    }
    Ok(())
}

/// Build a synthetic cold descriptor list for native models or an explicit
/// raw-DMA validation probe.
///
/// Ordinary target code must construct a ring through [`RxRingStopped`],
/// which couples mutation to the arena lifecycle instead of accepting a bare
/// descriptor slice.
#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
pub fn build_cold_ring(
    descriptors: &[Descriptor],
    descriptor_dma_base: u32,
    buffer_addresses: &[u32],
    buffer_size: u32,
) -> Result<(), RxRingError> {
    build_cold_ring_inner(
        descriptors,
        descriptor_dma_base,
        buffer_addresses,
        buffer_size,
    )
}

/// Publish a synthetic cold ring in a native model with no DMA actor.
#[cfg(not(target_pointer_width = "32"))]
pub fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    let binding = RxDmaBinding::raw_validation(descriptor_dma_base);
    publish_cold_ring_inner(mmio, &binding, descriptor_dma_base, enable_rx)
}

/// Publish raw target addresses outside the owned storage path.
///
/// # Safety
///
/// `descriptor_dma_base` must identify a fully initialized static descriptor
/// chain whose buffers remain exclusively available to DMA until the walker
/// is confirmed stopped.
#[cfg(all(target_pointer_width = "32", feature = "validation-raw-dma"))]
#[allow(
    unsafe_code,
    reason = "raw target publication makes its DMA lifetime contract explicit"
)]
pub unsafe fn publish_cold_ring<M: RxDma>(
    mmio: &mut M,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    let binding = RxDmaBinding::raw_validation(descriptor_dma_base);
    publish_cold_ring_inner(mmio, &binding, descriptor_dma_base, enable_rx)
}

fn publish_cold_ring_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
    descriptor_dma_base: u32,
    enable_rx: bool,
) -> Result<(), RxRingError> {
    if !descriptor_address_valid(descriptor_dma_base) {
        return Err(RxRingError::Address);
    }
    mmio.fence();
    mmio.set_descriptor_high_window(binding, 0x02f0);
    mmio.write_descriptor_base(binding, descriptor_dma_base);
    if enable_rx {
        mmio.publish_walker_enable(binding);
    }
    mmio.fence();
    Ok(())
}

/// Opens the RX walker in a native model after publishing a cold ring.
///
/// The vendor cold path keeps these operations separate:
/// `wDev_AppendRxBlocks` publishes the first descriptor base, while the later
/// `chip_enable` path calls `hal_mac_rx_enable`. Keeping this as a distinct
/// operation preserves that ordering and gives the base register time to
/// settle while the caller completes channel/MAC setup.
#[cfg(not(target_pointer_width = "32"))]
pub fn enable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    enable_receive_inner(mmio, &RxDmaBinding::raw_validation(0))
}

fn enable_receive_inner<M: RxDma>(
    mmio: &mut M,
    binding: &RxDmaBinding<'_>,
) -> Result<(), RxRingError> {
    mmio.try_with_walker_enabled(binding, |_enabled| ())
        .ok_or(RxRingError::Busy)
}

/// Stop the RX walker and confirm that the peripheral released its enable
/// edge before the owner rebuilds descriptor words or links.
///
/// The pinned `hal_mac_rx_disable` body is exactly this bit clear. The fence
/// and readback turn that raw leaf into an explicit Rust ownership boundary:
/// callers may mutate the ring only after this function returns `Ok(())`.
fn disable_receive_inner<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    mmio.try_with_walker_stopped(|_stopped| ())
        .ok_or(RxRingError::Busy)
}

/// Stop a synthetic/native walker or an explicit raw-DMA validation epoch.
///
/// Production target lifecycle code must consume [`RxRingLive::try_stop`], so
/// the register edge and the Rust ownership state cannot diverge.
#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
pub fn disable_receive<M: RxDma>(mmio: &mut M) -> Result<(), RxRingError> {
    disable_receive_inner(mmio)
}

/// Returns one CPU-owned completed descriptor to the cold/live ring.
#[cfg(any(not(target_pointer_width = "32"), feature = "validation-raw-dma"))]
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
