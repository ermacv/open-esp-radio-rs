//! Fixed receive staging ownership recovered from the vendor ESF path.
//!
//! The vendor implementation does not retain Wi-Fi DMA buffers while upper
//! layers parse a frame. Complete `_oracles/libpp.a[wdev.o]::wDev_IndicateFrame`
//! allocates a kind-7 ESF object, copies the completed RX unit, calls
//! `wDev_DiscardFrame`, and only then calls `lmacRxDone`. Complete
//! `_oracles/libpp.a[lmac.o]::lmacRxDone` enqueues that independent object and
//! posts PP event 17.
//!
//! Complete `_oracles/libpp.a[esf_buf.o]::esf_buf_setup` and
//! `esf_buf_alloc_dynamic` establish the ordinary large-RX profile: 32
//! kind-7 objects backed by internal memory. The vendor ABI uses a 0x90-byte
//! header plus a 1,700-byte payload. This module preserves the exact
//! `Free -> Radio -> Network -> Free` ownership and 32-by-1,700 geometry,
//! while omitting the C-only ESF header and intrusive pointers.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    descriptor::length as descriptor_length,
    rx::{RxCompletedDescriptor, RxDma, RxRingError, RxRingLive, RxSegment},
};

pub const VENDOR_LARGE_RX_SLOT_COUNT: usize = 32;
pub const VENDOR_LARGE_RX_PAYLOAD_CAPACITY: usize = 1_700;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageError {
    InvalidPool,
    Exhausted,
    Empty,
    TooLong,
    SourceTooShort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxStageTransactionError {
    Stage(RxStageError),
    Ring(RxRingError),
    Ownership,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StagedMetadata {
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
    length: usize,
}

#[repr(C, align(4))]
struct RxStageSlot<const CAPACITY: usize>(UnsafeCell<[u8; CAPACITY]>);

impl<const CAPACITY: usize> RxStageSlot<CAPACITY> {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; CAPACITY]))
    }
}

// SAFETY: access to the UnsafeCell is admitted only after the matching bit
// makes the slot uniquely Radio-owned. Network ownership exposes immutable
// bytes, and the slot cannot be claimed again until that token is dropped.
unsafe impl<const CAPACITY: usize> Sync for RxStageSlot<CAPACITY> {}

/// Fixed staging storage with explicit vendor-equivalent ownership states.
///
/// Place `RxStagePool<32, 1700>` in internal SRAM. It is ordinary CPU-owned
/// memory and must never be published to the Wi-Fi DMA walker.
pub struct RxStagePool<const SLOTS: usize, const CAPACITY: usize> {
    slots: [RxStageSlot<CAPACITY>; SLOTS],
    claimed: AtomicUsize,
    network: AtomicUsize,
}

impl<const SLOTS: usize, const CAPACITY: usize> RxStagePool<SLOTS, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            slots: [const { RxStageSlot::new() }; SLOTS],
            claimed: AtomicUsize::new(0),
            network: AtomicUsize::new(0),
        }
    }

    pub fn try_stage<'pool>(
        &'pool self,
        completed: RxCompletedDescriptor,
        source: &[u8],
    ) -> Result<RadioRxFrame<'pool, SLOTS, CAPACITY>, RxStageError> {
        if SLOTS == 0 || SLOTS > usize::BITS as usize || CAPACITY == 0 {
            return Err(RxStageError::InvalidPool);
        }
        let length = usize::try_from(descriptor_length(completed.word0()))
            .map_err(|_| RxStageError::TooLong)?;
        if length == 0 {
            return Err(RxStageError::Empty);
        }
        if length > CAPACITY {
            return Err(RxStageError::TooLong);
        }
        let source = source.get(..length).ok_or(RxStageError::SourceTooShort)?;
        let slot = (0..SLOTS)
            .find(|&slot| self.try_claim_radio(slot))
            .ok_or(RxStageError::Exhausted)?;

        // SAFETY: try_claim_radio made this slot uniquely Radio-owned, and
        // both source and destination have been bounded by `length`.
        unsafe {
            (&mut *self.slots[slot].0.get())[..length].copy_from_slice(source);
        }
        Ok(RadioRxFrame {
            pool: self,
            slot,
            metadata: StagedMetadata {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                next_descriptor_address: completed.next_descriptor_address(),
                length,
            },
            live: true,
        })
    }

    /// Copy one completed DMA frame, return its descriptor to hardware, and
    /// only then publish an independent upper-layer owner.
    ///
    /// This is the complete ownership order in
    /// `_oracles/libpp.a[wdev.o]::wDev_IndicateFrame`: allocate/copy,
    /// `wDev_DiscardFrame`/`wDev_AppendRxBlocks`, then `lmacRxDone`. Neither
    /// protocol parsing nor the network queue is allowed to retain the DMA
    /// descriptor. The caller supplies only the storage-specific rearm
    /// operation; this method owns the ordering and the reload completion.
    ///
    /// SOURCE\[HIL_OPEN_HE20_RX_OWNERSHIP_2026_07_30]: in the
    /// `psram-code-psram-data` profile, two reset-separated bidirectional
    /// HE20/MCS9 runs completed with `BUFFER_FULL=0` and `FIFO_OVERFLOW=0`.
    /// The proof run performed 5,147 RX-priority scheduling handoffs during
    /// TX preparation and delivered 10.036-Mbit/s RX plus 67.942-Mbit/s TX.
    /// The preceding parse-before-recycle path produced `BUFFER_FULL`.
    #[inline(never)]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_rx_hot")
    )]
    pub fn stage_recycle_and_publish<'pool, const COUNT: usize, M, F>(
        &'pool self,
        completed: RxCompletedDescriptor,
        source: &[u8],
        mmio: &mut M,
        ring: &mut RxRingLive<'_, COUNT>,
        prepare_buffer: F,
    ) -> Result<NetworkRxFrame<'pool, SLOTS, CAPACITY>, RxStageTransactionError>
    where
        M: RxDma,
        F: FnMut(usize) -> Result<(), RxRingError>,
    {
        let radio_frame = self
            .try_stage(completed, source)
            .map_err(RxStageTransactionError::Stage)?;
        let append = ring
            .recycle_completed_prefix::<1, _, _>(mmio, prepare_buffer)
            .map_err(RxStageTransactionError::Ring)?
            .ok_or(RxStageTransactionError::Ring(RxRingError::Busy))?;
        if append.descriptor_count != 1 {
            return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
        }
        ring.finish_pending_reload(mmio)
            .map_err(RxStageTransactionError::Ring)?;
        radio_frame
            .publish()
            .map_err(|_radio_frame| RxStageTransactionError::Ownership)
    }

    pub fn claimed_slots(&self) -> u32 {
        self.claimed.load(Ordering::Acquire).count_ones()
    }

    pub fn network_slots(&self) -> u32 {
        self.network.load(Ordering::Acquire).count_ones()
    }

    fn try_claim_radio(&self, slot: usize) -> bool {
        let bit = 1_usize << slot;
        if self.claimed.fetch_or(bit, Ordering::AcqRel) & bit != 0 {
            return false;
        }
        if self.network.load(Ordering::Acquire) & bit == 0 {
            return true;
        }
        self.claimed.fetch_and(!bit, Ordering::AcqRel);
        false
    }

    fn try_publish_network(&self, slot: usize) -> bool {
        let bit = 1_usize << slot;
        if self.claimed.load(Ordering::Acquire) & bit == 0
            || self.network.fetch_or(bit, Ordering::AcqRel) & bit != 0
        {
            return false;
        }
        if self.claimed.load(Ordering::Acquire) & bit != 0 {
            return true;
        }
        self.network.fetch_and(!bit, Ordering::AcqRel);
        false
    }

    fn release_radio(&self, slot: usize) -> bool {
        let bit = 1_usize << slot;
        self.network.load(Ordering::Acquire) & bit == 0
            && self.claimed.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }

    fn release_network(&self, slot: usize) -> bool {
        let bit = 1_usize << slot;
        if self.network.load(Ordering::Acquire) & bit == 0
            || self.claimed.fetch_and(!bit, Ordering::AcqRel) & bit == 0
        {
            return false;
        }
        self.network.fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> Default for RxStagePool<SLOTS, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique frame owner between the DMA copy and queue publication.
pub struct RadioRxFrame<'pool, const SLOTS: usize, const CAPACITY: usize> {
    pool: &'pool RxStagePool<SLOTS, CAPACITY>,
    slot: usize,
    metadata: StagedMetadata,
    live: bool,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> RadioRxFrame<'pool, SLOTS, CAPACITY> {
    /// Transfer the staged frame to the upper receive queue.
    pub fn publish(
        mut self,
    ) -> Result<NetworkRxFrame<'pool, SLOTS, CAPACITY>, RadioRxFrame<'pool, SLOTS, CAPACITY>> {
        if !self.pool.try_publish_network(self.slot) {
            return Err(self);
        }
        self.live = false;
        Ok(NetworkRxFrame {
            pool: self.pool,
            slot: self.slot,
            metadata: self.metadata,
        })
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> Drop for RadioRxFrame<'_, SLOTS, CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            // The transition is required behavior, not a debug-only check.
            // Keep it outside `debug_assert!`: release builds erase the
            // complete assertion expression.
            let released = self.pool.release_radio(self.slot);
            debug_assert!(released);
        }
    }
}

/// Unique upper-layer owner of one staged receive frame.
pub struct NetworkRxFrame<'pool, const SLOTS: usize, const CAPACITY: usize> {
    pool: &'pool RxStagePool<SLOTS, CAPACITY>,
    slot: usize,
    metadata: StagedMetadata,
}

impl<const SLOTS: usize, const CAPACITY: usize> NetworkRxFrame<'_, SLOTS, CAPACITY> {
    pub fn segment(&self) -> RxSegment<'_> {
        // SAFETY: this non-Clone token uniquely retains Network ownership, so
        // the pool cannot mutate or reclaim the matching slot.
        let bytes = unsafe { &*self.pool.slots[self.slot].0.get() };
        RxSegment {
            descriptor_address: self.metadata.descriptor_address,
            descriptor_word0: self.metadata.descriptor_word0,
            buffer: &bytes[..self.metadata.length],
            next_descriptor_address: self.metadata.next_descriptor_address,
        }
    }

    pub const fn length(&self) -> usize {
        self.metadata.length
    }
}

impl<const SLOTS: usize, const CAPACITY: usize> Drop for NetworkRxFrame<'_, SLOTS, CAPACITY> {
    fn drop(&mut self) {
        let released = self.pool.release_network(self.slot);
        debug_assert!(released);
    }
}

/// Bounded FIFO of independently owned RX frames.
///
/// The queue stores ownership tokens rather than DMA pointers. Consequently,
/// queueing and parsing can never extend hardware ownership of the original
/// descriptor. Its maximum useful depth is the staging-pool slot count.
pub struct RxFrameQueue<'pool, const SLOTS: usize, const CAPACITY: usize> {
    frames: [Option<NetworkRxFrame<'pool, SLOTS, CAPACITY>>; SLOTS],
    head: usize,
    tail: usize,
    len: usize,
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> RxFrameQueue<'pool, SLOTS, CAPACITY> {
    pub fn new() -> Self {
        assert!(SLOTS != 0, "RX frame queue requires at least one slot");
        Self {
            frames: [const { None }; SLOTS],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn is_full(&self) -> bool {
        self.len == SLOTS
    }

    pub fn try_push(
        &mut self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        if self.is_full() || self.frames[self.tail].is_some() {
            return Err(frame);
        }
        self.frames[self.tail] = Some(frame);
        self.tail = (self.tail + 1) % SLOTS;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        if self.is_empty() {
            return None;
        }
        let frame = self.frames[self.head].take()?;
        self.head = (self.head + 1) % SLOTS;
        self.len -= 1;
        Some(frame)
    }
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize> Default
    for RxFrameQueue<'pool, SLOTS, CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(length: u32) -> RxCompletedDescriptor {
        RxCompletedDescriptor::from_raw_parts_for_test(
            0,
            0x2f00_1000,
            1_700 | (length << 14) | 0xc000_0000,
            0,
        )
    }

    #[test]
    fn ownership_moves_free_radio_network_free() {
        let pool = RxStagePool::<2, 32>::new();
        let radio = pool.try_stage(completed(4), &[1, 2, 3, 4]).unwrap();
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(pool.network_slots(), 0);
        let network = radio.publish().ok().unwrap();
        assert_eq!(pool.network_slots(), 1);
        assert_eq!(network.segment().buffer, &[1, 2, 3, 4]);
        drop(network);
        assert_eq!(pool.claimed_slots(), 0);
        assert_eq!(pool.network_slots(), 0);
    }

    #[test]
    fn pool_exhaustion_is_bounded_and_does_not_alias() {
        let pool = RxStagePool::<1, 32>::new();
        let first = pool.try_stage(completed(4), &[1, 2, 3, 4]).unwrap();
        assert!(matches!(
            pool.try_stage(completed(4), &[5, 6, 7, 8]),
            Err(RxStageError::Exhausted)
        ));
        drop(first);
        assert!(pool.try_stage(completed(4), &[5, 6, 7, 8]).is_ok());
    }

    #[test]
    fn oversized_frame_never_claims_a_slot() {
        let pool = RxStagePool::<1, 3>::new();
        assert!(matches!(
            pool.try_stage(completed(4), &[1, 2, 3, 4]),
            Err(RxStageError::TooLong)
        ));
        assert_eq!(pool.claimed_slots(), 0);
    }

    #[test]
    fn frame_queue_preserves_fifo_ownership_and_releases_on_drop() {
        let pool = RxStagePool::<2, 32>::new();
        let first = pool
            .try_stage(completed(1), &[0x11])
            .unwrap()
            .publish()
            .ok()
            .unwrap();
        let second = pool
            .try_stage(completed(1), &[0x22])
            .unwrap()
            .publish()
            .ok()
            .unwrap();
        let mut queue = RxFrameQueue::new();
        queue.try_push(first).ok().unwrap();
        queue.try_push(second).ok().unwrap();
        assert!(queue.is_full());
        assert_eq!(queue.pop().unwrap().segment().buffer, &[0x11]);
        assert_eq!(queue.pop().unwrap().segment().buffer, &[0x22]);
        assert!(queue.is_empty());
        assert_eq!(pool.claimed_slots(), 0);
    }
}
