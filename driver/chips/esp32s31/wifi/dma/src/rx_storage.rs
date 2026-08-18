//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. Descriptor count,
//! payload capacity and placement policy are selected by the board or runtime
//! composition and remain const-generic here.

use core::{cell::UnsafeCell, sync::atomic::AtomicU8};

use crate::{
    descriptor::Descriptor,
    rx_dma::RxDma,
    rx_ring::{
        RX_BUFFER_SENTINEL, RxCompletedDescriptor, RxCompletedUnit, RxCompletedUnitFrontier,
        RxDmaArenaState, RxFrozenCursor, RxLiveAppend, RxRingError, RxRingHalted, RxRingLive,
        RxRingStopped, RxSegment, prepare_recycled_buffer,
    },
};

/// One aligned DMA-visible buffer with room for the hardware recycle guard.
#[repr(C, align(4))]
pub struct RxDmaBuffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    UnsafeCell<[u8; STORAGE_SIZE]>,
);

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE> {
    const fn new() -> Self {
        assert!(STORAGE_SIZE >= BUFFER_SIZE + 4);
        Self(UnsafeCell::new([0; STORAGE_SIZE]))
    }

    pub(crate) fn dma_address(&self) -> Result<u32, RxDmaStorageError> {
        u32::try_from(self.0.get().addr()).map_err(|_| RxDmaStorageError::AddressWidth)
    }

    fn cpu_owned_mut(&mut self) -> &mut [u8; BUFFER_SIZE] {
        self.0
            .get_mut()
            .get_mut(..BUFFER_SIZE)
            .expect("RX DMA storage always contains its declared buffer prefix")
            .try_into()
            .expect("the selected RX DMA prefix has the exact declared size")
    }

    fn cpu_owned_storage_mut(&mut self) -> &mut [u8; STORAGE_SIZE] {
        self.0.get_mut()
    }

    /// # Safety
    ///
    /// The caller must own the matching completed descriptor. The returned
    /// view must not survive descriptor recycle.
    #[allow(unsafe_code, reason = "completed descriptor proves CPU ownership")]
    pub(crate) unsafe fn completed(&self) -> &[u8; BUFFER_SIZE] {
        // SAFETY: the type guarantees a prefix of exactly this size.
        unsafe { &*self.0.get().cast::<[u8; BUFFER_SIZE]>() }
    }

    /// # Safety
    ///
    /// The caller must own the matching completed descriptor and invoke this
    /// only from the ring's rearm closure.
    #[allow(unsafe_code, reason = "ring rearm owns the DMA buffer transition")]
    pub(crate) unsafe fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        // SAFETY: ring ownership makes this the only CPU or DMA writer.
        unsafe { prepare_recycled_buffer(&mut *self.0.get(), BUFFER_SIZE) }
    }

    /// Whether DMA has overwritten the leading recycle guard.
    ///
    /// This is observation only: it never transfers buffer ownership. It is
    /// used together with a later terminal descriptor to distinguish a full
    /// non-terminal segment from an untouched armed descriptor.
    #[allow(
        unsafe_code,
        reason = "volatile read observes the asynchronous DMA writer"
    )]
    pub(crate) fn leading_guard_overwritten(&self) -> bool {
        // SAFETY: volatile access models the asynchronous DMA writer. The
        // result is only evidence for a subsequent descriptor observation.
        unsafe { self.0.get().cast::<u32>().read_volatile() != RX_BUFFER_SENTINEL }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDmaStorageError {
    AddressWidth,
}

/// Read authority for one completed descriptor and its matching DMA buffer.
///
/// The token retains the unique mutable borrow of the live ring. Consequently
/// no safe caller can rearm the descriptor while a segment view is alive.
pub struct RxDmaCompletedDescriptor<
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
> {
    descriptor: RxCompletedDescriptor,
    buffer: &'owner RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>,
    _ring: &'owner mut RxRingLive<'ring, COUNT>,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaCompletedDescriptor<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn index(&self) -> usize {
        self.descriptor.index()
    }

    #[allow(
        unsafe_code,
        reason = "completed descriptor token retains the live ring"
    )]
    pub fn segment(&self) -> RxSegment<'_> {
        // SAFETY: the unforgeable descriptor token retains the live ring's
        // mutable borrow, and `validate_live_ring` matched this buffer arena
        // to that ring before transferring completion ownership.
        let buffer = unsafe { self.buffer.completed() };
        RxSegment {
            descriptor_address: self.descriptor.descriptor_address(),
            descriptor_word0: self.descriptor.word0(),
            buffer,
            next_descriptor_address: self.descriptor.next_descriptor_address(),
        }
    }
}

/// Read and recycle authority for one complete, possibly chained RX unit.
///
/// All segment views are bounded by lengths captured before this token was
/// created. The token retains the unique live-ring borrow until the complete
/// copy is finished and [`recycle`](Self::recycle) consumes it.
pub struct RxDmaCompletedUnit<
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
> {
    unit: RxCompletedUnit,
    storage: &'owner RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>,
    ring: &'owner mut RxRingLive<'ring, COUNT>,
    requires_recycle: bool,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaCompletedUnit<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn head_index(&self) -> usize {
        self.unit.head_index()
    }

    pub const fn descriptor_count(&self) -> usize {
        self.unit.descriptor_count()
    }

    pub const fn total_length(&self) -> usize {
        self.unit.total_length()
    }

    pub const fn metadata(&self) -> &RxCompletedUnit {
        &self.unit
    }

    #[allow(unsafe_code, reason = "completed unit token retains every segment")]
    pub fn segment(&self, step: usize) -> Option<&[u8]> {
        let length = self.unit.segment_length(step)?;
        let index = self
            .unit
            .head_index()
            .checked_add(step)?
            .checked_rem(COUNT)?;
        let buffer = self.storage.buffers.get(index)?;
        // SAFETY: `RxCompletedUnit` proves that the terminal descriptor made
        // every preceding segment CPU-owned. This token retains the mutable
        // ring borrow, preventing recycle while the returned slice lives.
        unsafe { buffer.completed().get(..length) }
    }

    #[allow(
        unsafe_code,
        reason = "consuming completed unit owns the rearm transition"
    )]
    pub fn recycle<M: RxDma>(mut self, mmio: &mut M) -> Result<Option<RxLiveAppend>, RxRingError> {
        let descriptor_count = self.unit.descriptor_count();
        let result = self
            .ring
            .recycle_completed_unit_owned(mmio, descriptor_count, |index| {
                // SAFETY: this consuming token owns every segment until this
                // rearm closure returns it to the live walker.
                unsafe { self.storage.buffers[index].prepare_for_recycle() }
            });
        if matches!(result, Ok(Some(_))) {
            self.requires_recycle = false;
        }
        result
    }

    /// Finish the staging handoff without returning this descriptor unit to
    /// the active walker yet.
    ///
    /// The unit remains represented by the ring's observed mask. Its producer
    /// must reclaim it against the same frozen LAST before allowing an append
    /// to create a new descriptor-address generation.
    pub fn retain_for_deferred_recycle(mut self) {
        self.requires_recycle = false;
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Drop
    for RxDmaCompletedUnit<'_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    fn drop(&mut self) {
        if self.requires_recycle {
            // `take_completed_unit` marks the complete descriptor group as
            // observed exactly once. Losing that linear CPU owner before a
            // successful rearm would otherwise leave an arena which looks
            // live but can never transfer the same group again.
            self.ring.require_reset();
        }
    }
}

/// Descriptor and buffer arena whose address remains stable for a DMA epoch.
///
/// The buffer address table remains caller-owned because the live ring borrows
/// it for its entire epoch. Keeping that table separate avoids a
/// self-referential owner and lets a platform place only DMA-visible storage
/// in its dedicated linker section.
#[repr(C)]
pub struct RxDmaStorage<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> {
    descriptors: [Descriptor; COUNT],
    buffers: [RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT],
    lifecycle: AtomicU8,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; COUNT],
            buffers: [const { RxDmaBuffer::new() }; COUNT],
            lifecycle: AtomicU8::new(RxDmaArenaState::Reusable as u8),
        }
    }

    /// Sticky lifecycle state stored with the static arena rather than in its
    /// movable ring token.
    pub fn lifecycle_state(&self) -> RxDmaArenaState {
        RxDmaArenaState::load(&self.lifecycle)
    }

    pub const fn descriptors(&self) -> &[Descriptor; COUNT] {
        &self.descriptors
    }

    pub const fn buffers(&self) -> &[RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT] {
        &self.buffers
    }

    /// Mutable CPU view available only before an RX ring borrows this arena.
    ///
    /// The exclusive storage borrow prevents a stopped or live descriptor
    /// owner from existing at the same time. This is primarily useful for
    /// deterministic host fixtures and pre-publication initialization.
    pub fn buffer_mut(&mut self, index: usize) -> Option<&mut [u8; BUFFER_SIZE]> {
        self.buffers.get_mut(index).map(RxDmaBuffer::cpu_owned_mut)
    }

    /// Restore one buffer's recycle guards before any ring borrows this arena.
    ///
    /// Requiring `&mut self` proves that no stopped/live ring token can exist
    /// at the same time, so pre-publication setup does not need privileged
    /// access to the buffer's `UnsafeCell`.
    pub fn prepare_unpublished_buffer(&mut self, index: usize) -> Result<(), RxRingError> {
        let buffer = self.buffers.get_mut(index).ok_or(RxRingError::Count)?;
        prepare_recycled_buffer(buffer.cpu_owned_storage_mut(), BUFFER_SIZE)
    }

    pub fn dma_layout(
        &self,
        buffer_addresses: &mut [u32; COUNT],
    ) -> Result<u32, RxDmaStorageError> {
        for (address, buffer) in buffer_addresses.iter_mut().zip(&self.buffers) {
            *address = buffer.dma_address()?;
        }
        u32::try_from(self.descriptors.as_ptr().addr()).map_err(|_| RxDmaStorageError::AddressWidth)
    }

    /// Prepare the first stopped ring on a hardware target.
    ///
    /// The `'static` receiver is essential: safe code may forget a live ring,
    /// so DMA-visible descriptors and buffers must remain allocated even when
    /// the typestate owner is lost. Borrowing the arena for `'static` also
    /// prevents later safe mutable pre-publication access.
    #[cfg(target_pointer_width = "32")]
    pub fn prepare_ring<'addresses, M: RxDma>(
        &'static self,
        mmio: &mut M,
        descriptor_base: u32,
        buffer_addresses: &'addresses [u32; COUNT],
    ) -> Result<RxRingStopped<'addresses, COUNT>, RxRingError> {
        self.prepare_ring_bound(mmio, descriptor_base, buffer_addresses)
    }

    /// Prepare a stopped ring for a native host model with no DMA actor.
    #[cfg(not(target_pointer_width = "32"))]
    pub fn prepare_ring<'storage, M: RxDma>(
        &'storage self,
        mmio: &mut M,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<RxRingStopped<'storage, COUNT>, RxRingError> {
        self.prepare_ring_bound(mmio, descriptor_base, buffer_addresses)
    }

    #[allow(
        unsafe_code,
        reason = "stopped ring exclusively owns initial buffer rearm"
    )]
    fn prepare_ring_bound<'storage, M: RxDma>(
        &'storage self,
        mmio: &mut M,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; COUNT],
    ) -> Result<RxRingStopped<'storage, COUNT>, RxRingError> {
        self.validate_descriptor_base(descriptor_base)?;
        self.validate_buffer_addresses(buffer_addresses)?;
        match self.lifecycle_state() {
            RxDmaArenaState::Reusable => {}
            RxDmaArenaState::Prepared | RxDmaArenaState::Live => {
                return Err(RxRingError::Busy);
            }
            RxDmaArenaState::ResetRequired => return Err(RxRingError::ResetRequired),
        }
        let buffer_size = u32::try_from(BUFFER_SIZE).map_err(|_| RxRingError::Size)?;
        // `prepare_ring` takes a shared static arena reference, so Rust
        // borrowing alone cannot prevent two root capabilities. Claim the
        // stopped/halted owner graph before mutating any descriptor.
        if !RxDmaArenaState::claim_prepared(&self.lifecycle) {
            return Err(RxRingError::Busy);
        }
        let prepared = RxRingStopped::prepare_inner(
            mmio,
            &self.descriptors,
            descriptor_base,
            buffer_addresses,
            buffer_size,
            |index| {
                // SAFETY: stopped-ring preparation owns every descriptor and
                // the validated address table binds `index` to this arena.
                unsafe { self.buffers[index].prepare_for_recycle() }
            },
            Some(&self.lifecycle),
        );
        if prepared.is_err() {
            // Preparation may have failed while disabling an unexpectedly
            // live walker. Without the returned ring capability there is no
            // proof that DMA stopped referencing this arena, so never reopen
            // the root constructor until radio reset.
            RxDmaArenaState::ResetRequired.store(&self.lifecycle);
        }
        prepared
    }

    /// Rebuild a halted ring only when it belongs to this storage arena.
    #[allow(unsafe_code, reason = "halted ring exclusively owns buffer rearm")]
    pub fn prepare_halted<'storage, M: RxDma>(
        &'storage self,
        ring: RxRingHalted<'storage, COUNT>,
        mmio: &mut M,
    ) -> Result<RxRingStopped<'storage, COUNT>, (RxRingHalted<'storage, COUNT>, RxRingError)> {
        match self.lifecycle_state() {
            RxDmaArenaState::Prepared => {}
            #[cfg(not(target_pointer_width = "32"))]
            RxDmaArenaState::Reusable => {
                // Native models may construct a raw halted ring over this
                // arena because no asynchronous DMA actor exists. Claim the
                // same singleton lifecycle before accepting that test owner.
                if !RxDmaArenaState::claim_prepared(&self.lifecycle) {
                    return Err((ring, RxRingError::Busy));
                }
            }
            RxDmaArenaState::ResetRequired => {
                return Err((ring, RxRingError::ResetRequired));
            }
            #[cfg(target_pointer_width = "32")]
            RxDmaArenaState::Reusable | RxDmaArenaState::Live => {
                return Err((ring, RxRingError::Busy));
            }
            #[cfg(not(target_pointer_width = "32"))]
            RxDmaArenaState::Live => {
                return Err((ring, RxRingError::Busy));
            }
        }
        if let Err(error) = self.validate_ring_layout(
            ring.descriptor_base(),
            ring.descriptors(),
            ring.buffer_addresses(),
        ) {
            return Err((ring, error));
        }
        let buffer_size = match u32::try_from(BUFFER_SIZE) {
            Ok(buffer_size) => buffer_size,
            Err(_) => return Err((ring, RxRingError::Size)),
        };
        ring.prepare_owned(mmio, buffer_size, |index| {
            // SAFETY: the halted owner proves that DMA is stopped, and the
            // validated layout binds the descriptor to this exact buffer.
            unsafe { self.buffers[index].prepare_for_recycle() }
        })
    }

    /// Transfer one completed descriptor together with its buffer read
    /// authority. The returned token prevents a concurrent recycle borrow.
    pub fn take_completed<'owner, 'ring>(
        &'owner self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        index: usize,
    ) -> Result<
        Option<RxDmaCompletedDescriptor<'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        match ring.validate_completed_descriptor(index) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(error) => {
                if error != RxRingError::Count {
                    ring.require_reset();
                }
                return Err(error);
            }
        }
        let Some(descriptor) = ring.take_completed_owned(index) else {
            return Ok(None);
        };
        let buffer = self.buffers.get(index).ok_or(RxRingError::Count)?;
        Ok(Some(RxDmaCompletedDescriptor {
            descriptor,
            buffer,
            _ring: ring,
        }))
    }

    #[allow(
        unsafe_code,
        reason = "live ring proves its completed half is CPU-owned"
    )]
    pub fn recycle_completed_half<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend>, RxRingError> {
        self.validate_live_ring(ring)?;
        ring.recycle_completed_half_owned(mmio, |index| {
            // SAFETY: the ring invokes the closure only after observing the
            // complete half and immediately before descriptor publication.
            unsafe { self.buffers[index].prepare_for_recycle() }
        })
    }

    #[allow(
        unsafe_code,
        reason = "live ring proves its completed prefix is CPU-owned"
    )]
    pub fn recycle_completed_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend>, RxRingError> {
        self.validate_live_ring(ring)?;
        ring.recycle_completed_prefix_owned::<MAX_BATCH, _, _>(mmio, |index| {
            // SAFETY: the ring invokes the closure only for its observed
            // prefix immediately before returning those buffers to DMA.
            unsafe { self.buffers[index].prepare_for_recycle() }
        })
    }

    pub fn completed_unit_frontier(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring
            .completed_unit_frontier_with(|index| self.buffers[index].leading_guard_overwritten()))
    }

    /// Return only complete units which the hardware last-descriptor frontier
    /// has released to software. A descriptor-local `RX_DONE` observation is
    /// intentionally insufficient for recycling its link word.
    pub fn completed_unit_frontier_through(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.completed_unit_frontier_through_with(last_descriptor_low, |index| {
                self.buffers[index].leading_guard_overwritten()
            }),
        )
    }

    /// LAST-bounded frontier with the matching ordered NEXT observation.
    /// NEXT=0 and LAST at the accepted tail release untouched intermediate
    /// descriptors which the vendor worker also walks before the terminal.
    pub fn completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| self.buffers[index].leading_guard_overwritten(),
        ))
    }

    /// Return the first complete unit at the software frontier, bounded by
    /// the hardware LAST snapshot. This is the unit-sized counterpart used
    /// when every recycle requires its own link-release proof.
    pub fn first_completed_unit_frontier_through(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(
            ring.first_completed_unit_frontier_through_with(last_descriptor_low, |index| {
                self.buffers[index].leading_guard_overwritten()
            }),
        )
    }

    pub fn first_completed_unit_frontier_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.first_completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| self.buffers[index].leading_guard_overwritten(),
        ))
    }

    pub fn take_completed_unit<'owner, 'ring>(
        &'owner self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        descriptor_limit: usize,
    ) -> Result<
        Option<RxDmaCompletedUnit<'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        let unit = match ring.take_completed_unit_owned(descriptor_limit, |index| {
            self.buffers[index].leading_guard_overwritten()
        }) {
            Ok(Some(unit)) => unit,
            Ok(None) => return Ok(None),
            Err(error) => {
                ring.require_reset();
                return Err(error);
            }
        };
        Ok(Some(RxDmaCompletedUnit {
            unit,
            storage: self,
            ring,
            requires_recycle: true,
        }))
    }

    /// Return one copied complete unit through the vendor's frozen-LAST
    /// ownership proof.
    #[allow(
        unsafe_code,
        reason = "the frozen vendor LAST proves exclusive unit-buffer rearm"
    )]
    pub fn recycle_completed_unit_through_frozen_last<M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
        cursor: RxFrozenCursor,
        descriptor_count: usize,
    ) -> Result<Option<RxLiveAppend>, RxRingError> {
        self.validate_live_ring(ring)?;
        ring.recycle_completed_unit_through_frozen_last_owned(
            mmio,
            cursor,
            descriptor_count,
            |index| {
                // SAFETY: the ring limits mutation to the observed complete
                // unit ending at or before the generation-bound LAST.
                unsafe { self.buffers[index].prepare_for_recycle() }
            },
        )
    }

    fn validate_live_ring(&self, ring: &RxRingLive<'_, COUNT>) -> Result<(), RxRingError> {
        self.validate_ring_layout(
            ring.descriptor_base(),
            ring.descriptors(),
            ring.buffer_addresses(),
        )
    }

    fn validate_ring_layout(
        &self,
        descriptor_base: u32,
        descriptors: &[Descriptor; COUNT],
        buffer_addresses: &[u32; COUNT],
    ) -> Result<(), RxRingError> {
        if !core::ptr::eq(descriptors, &self.descriptors) {
            return Err(RxRingError::Address);
        }
        self.validate_descriptor_base(descriptor_base)?;
        self.validate_buffer_addresses(buffer_addresses)
    }

    fn validate_descriptor_base(&self, descriptor_base: u32) -> Result<(), RxRingError> {
        #[cfg(target_pointer_width = "32")]
        if u32::try_from(self.descriptors.as_ptr().addr()).map_err(|_| RxRingError::Address)?
            != descriptor_base
        {
            return Err(RxRingError::Address);
        }
        #[cfg(not(target_pointer_width = "32"))]
        let _ = descriptor_base;
        Ok(())
    }

    fn validate_buffer_addresses(
        &self,
        buffer_addresses: &[u32; COUNT],
    ) -> Result<(), RxRingError> {
        // ESP32-S31 DMA is a 32-bit target. Native builds use synthetic low
        // addresses and a mock that never touches host memory; descriptor
        // identity remains checked by `validate_ring_layout` there.
        #[cfg(target_pointer_width = "32")]
        for (buffer, &address) in self.buffers.iter().zip(buffer_addresses) {
            if buffer.dma_address().map_err(|_| RxRingError::Address)? != address {
                return Err(RxRingError::Address);
            }
        }
        #[cfg(not(target_pointer_width = "32"))]
        let _ = buffer_addresses;
        Ok(())
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Default
    for RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx_dma::RxDmaBinding;

    #[derive(Default)]
    struct MockRxDma {
        walker: bool,
        descriptor_base: u32,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        fail_disable: bool,
        ambiguous_enable: bool,
    }

    impl RxDma for MockRxDma {
        fn last_descriptor_low(&mut self) -> u32 {
            self.last_descriptor_low
        }

        fn next_descriptor_low(&mut self) -> u32 {
            self.next_descriptor_low
        }

        fn next_descriptor_word(&mut self) -> u32 {
            self.next_descriptor_low
        }

        fn with_ordered_cursor<R>(
            &mut self,
            observed: impl for<'confirmation> FnOnce(
                crate::rx_dma::RxDmaCursorObservation<'confirmation>,
            ) -> R,
        ) -> R {
            let last = self.last_descriptor_low();
            self.fence();
            let next = self.next_descriptor_low();
            self.fence();
            observed(crate::rx_dma::RxDmaCursorObservation::validation(
                last, next,
            ))
        }

        fn walker_enabled(&mut self) -> bool {
            self.walker
        }

        fn reload_pending(&mut self) -> bool {
            false
        }

        fn try_with_reload_settled<R>(
            &mut self,
            settled: impl for<'confirmation> FnOnce(
                crate::rx_dma::RxDmaReloadSettled<'confirmation>,
            ) -> R,
        ) -> Option<R> {
            (!self.reload_pending())
                .then(|| settled(crate::rx_dma::RxDmaReloadSettled::validation()))
        }

        fn set_descriptor_high_window(&mut self, _: &RxDmaBinding<'_>, _: u16) {}

        fn write_descriptor_base(&mut self, _: &RxDmaBinding<'_>, address: u32) {
            self.descriptor_base = address;
        }

        fn publish_walker_enable(&mut self, _: &RxDmaBinding<'_>) {
            self.walker = true;
        }

        fn request_reload(&mut self, _: &RxDmaBinding<'_>) {}

        fn try_with_walker_enabled<R>(
            &mut self,
            _: &RxDmaBinding<'_>,
            enabled: impl for<'confirmation> FnOnce(
                crate::rx_dma::RxDmaWalkerEnabled<'confirmation>,
            ) -> R,
        ) -> Option<R> {
            self.walker = true;
            (!self.ambiguous_enable)
                .then(|| enabled(crate::rx_dma::RxDmaWalkerEnabled::validation()))
        }

        fn try_with_walker_stopped<R>(
            &mut self,
            stopped: impl for<'confirmation> FnOnce(
                crate::rx_dma::RxDmaWalkerStopped<'confirmation>,
            ) -> R,
        ) -> Option<R> {
            if self.fail_disable {
                return None;
            }
            self.walker = false;
            Some(stopped(crate::rx_dma::RxDmaWalkerStopped::validation()))
        }

        fn fence(&mut self) {}
    }

    #[test]
    fn arena_initializes_in_its_final_location_and_recycles_one_buffer() {
        let mut storage = RxDmaStorage::<2, 16, 20>::new();

        assert_eq!(storage.descriptors().len(), 2);
        assert_eq!(storage.buffers().len(), 2);
        assert_eq!(storage.buffers().as_ptr().addr() & 3, 0);

        storage.prepare_unpublished_buffer(0).unwrap();
        assert!(!storage.buffers()[0].leading_guard_overwritten());
    }

    #[test]
    fn one_arena_cannot_issue_two_root_ring_capabilities() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut first_mmio = MockRxDma::default();
        let mut second_mmio = MockRxDma::default();

        let first = storage
            .prepare_ring(&mut first_mmio, BASE, &buffers)
            .expect("first root capability");
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
        assert!(matches!(
            storage.prepare_ring(&mut second_mmio, BASE, &buffers),
            Err(RxRingError::Busy)
        ));

        let live = first
            .try_start(&mut first_mmio)
            .map_err(|(_, error)| error)
            .expect("first live epoch");
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
        let _halted = live
            .try_stop(&mut first_mmio)
            .unwrap_or_else(|_| panic!("walker stops"));
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
        assert!(matches!(
            storage.prepare_ring(&mut second_mmio, BASE, &buffers),
            Err(RxRingError::Busy)
        ));
    }

    #[test]
    fn failed_root_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma {
            walker: true,
            fail_disable: true,
            ..MockRxDma::default()
        };

        assert!(matches!(
            storage.prepare_ring(&mut mmio, BASE, &buffers),
            Err(RxRingError::Busy)
        ));
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    }

    #[test]
    fn failed_halted_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        let halted = match live.try_stop(&mut mmio) {
            Ok(halted) => halted,
            Err(_) => panic!("walker stops"),
        };

        // Model an external hardware fault between epochs: the walker became
        // active again and refuses the stop request made by preparation.
        mmio.walker = true;
        mmio.fail_disable = true;
        let (_halted, error) = match storage.prepare_halted(halted, &mut mmio) {
            Ok(_) => panic!("unproved stop must reject preparation"),
            Err(failure) => failure,
        };

        assert_eq!(error, RxRingError::Busy);
        assert!(mmio.walker);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    }

    #[test]
    fn partial_live_buffer_rearm_quarantines_the_arena() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(live.take_completed(0).is_some());

        assert_eq!(
            live.recycle_completed_prefix::<1, _, _>(&mut mmio, |_| Err(RxRingError::Size)),
            Ok(None),
        );
        storage.descriptors()[1].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert_eq!(
            live.recycle_completed_prefix::<1, _, _>(&mut mmio, |_| Err(RxRingError::Size)),
            Err(RxRingError::Size),
        );
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    }

    #[test]
    fn completed_descriptor_metadata_is_validated_before_payload_transfer() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];

        for corruption in 0..3 {
            let storage = RxDmaStorage::<COUNT, 16, 20>::new();
            let mut mmio = MockRxDma::default();
            let prepared = storage
                .prepare_ring(&mut mmio, BASE, &buffers)
                .expect("prepared owner");
            let mut live = prepared
                .try_start(&mut mmio)
                .map_err(|(_, error)| error)
                .expect("live epoch");
            let descriptor = &storage.descriptors()[0];
            let mut word0 = 16
                | (8 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31;
            let mut buffer_address = buffers[0];
            let mut next_address = BASE + crate::descriptor::DESCRIPTOR_BYTES;
            match corruption {
                0 => word0 = (word0 & !crate::descriptor::SIZE_MASK) | 17,
                1 => buffer_address = buffers[0] + 4,
                2 => next_address = 0,
                _ => unreachable!(),
            }
            descriptor.publish(word0, buffer_address, next_address);

            assert!(matches!(
                storage.take_completed(&mut live, 0),
                Err(RxRingError::Corrupt)
            ));
            assert_eq!(live.observed_mask(), 0);
            assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
            assert!(live.try_stop(&mut mmio).is_ok());
        }
    }

    #[test]
    fn impossible_reload_frontier_quarantines_the_static_arena() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_none()
        );
        storage.descriptors()[1].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_some()
        );
        assert!(live.reload_pending());

        mmio.next_descriptor_low = 0;
        mmio.last_descriptor_low = (BASE + 1) & ADDRESS_LOW_MASK;
        assert_eq!(
            live.poll_pending_reload(&mut mmio),
            Err(RxRingError::Corrupt)
        );
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn in_arena_intermediate_last_repairs_from_its_successor() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_none()
        );
        storage.descriptors()[1].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_some()
        );
        assert!(live.reload_pending());

        // Old accepted tail is descriptor 3 and the pending tail is 0, but
        // hardware can still report the earlier descriptor 1 while the vendor
        // worker is returning several completed units. Its successor is the
        // exact base-repair value used by wDev_AppendRxBlocks.
        mmio.next_descriptor_low = 0;
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert_eq!(
            live.poll_pending_reload(&mut mmio),
            Ok(crate::rx_ring::RxReloadObservation::Settled)
        );
        assert_eq!(
            mmio.descriptor_base,
            BASE + 2 * crate::descriptor::DESCRIPTOR_BYTES
        );
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn exhausted_reload_records_the_direct_base_repair_facts() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_none()
        );
        storage.descriptors()[1].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert!(
            storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .is_some()
        );

        // The walker exhausted the previously accepted tail before observing
        // its new link. The vendor suffix republishes that link through BASE.
        mmio.next_descriptor_low = 0;
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        assert_eq!(
            live.poll_pending_reload(&mut mmio),
            Ok(crate::rx_ring::RxReloadObservation::Settled)
        );
        assert_eq!(mmio.descriptor_base, BASE);
        assert_eq!(
            live.reload_repair_evidence(),
            crate::rx_ring::RxReloadRepairEvidence {
                observations: 1,
                nonzero_word_with_zero_address: 0,
                base_repairs: 1,
                last_next_word: 0,
                last_last_low: Some(
                    (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK
                ),
                last_repair_head: Some(BASE),
            }
        );
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn exhausted_list_reclaims_untouched_prefix_before_terminal_payload() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");

        // Return descriptor zero while the old finite list has already
        // exhausted at descriptor three. The vendor append suffix repairs
        // BASE to the returned descriptor, leaving software head at one.
        storage.descriptors()[0].write_word0(
            16 | (4 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
        mmio.last_descriptor_low =
            (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = 0;
        let unit = storage
            .take_completed_unit(&mut live, 1)
            .expect("completed descriptor")
            .expect("unit owner");
        unit.recycle(&mut mmio)
            .expect("live append")
            .expect("returned descriptor");
        live.complete_pending_reload(&mut mmio)
            .expect("vendor reload repair");
        assert_eq!(live.recycle_start(), 1);
        assert_eq!(live.accepted_tail(), 0);

        // Hardware consumed only the repaired terminal before exhausting
        // again. Descriptors one through three retain their guards and armed
        // lengths, but NEXT=0/LAST=tail proves their links are released.
        storage.descriptors()[0].write_word0(
            16 | (4 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = 0;
        let frontier = storage
            .first_completed_unit_frontier_through_cursor(
                &live,
                mmio.last_descriptor_low,
                mmio.next_descriptor_low,
            )
            .expect("exhausted frontier");
        assert_eq!(frontier.unit_count, 1);
        assert_eq!(frontier.descriptor_count, COUNT);

        let unit = storage
            .take_completed_unit(&mut live, frontier.descriptor_count)
            .expect("released unit")
            .expect("terminal payload owner");
        assert_eq!(unit.descriptor_count(), COUNT);
        assert_eq!(unit.total_length(), 4);
        assert_eq!(unit.segment(0), Some(&[][..]));
        assert_eq!(unit.segment(1), Some(&[][..]));
        assert_eq!(unit.segment(2), Some(&[][..]));
        assert_eq!(unit.segment(3).map(<[u8]>::len), Some(4));
        let append = unit
            .recycle(&mut mmio)
            .expect("exhausted list recycle")
            .expect("full list republished");
        assert_eq!(append.head_index, 1);
        assert_eq!(append.descriptor_count, COUNT);
        assert_eq!(live.recycle_start(), 1);
        assert_eq!(live.accepted_tail(), 0);
        assert!(live.topology_snapshot().valid);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn dropping_a_taken_completed_unit_requires_radio_reset() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        let frontier = storage
            .completed_unit_frontier_through(&live, mmio.last_descriptor_low)
            .unwrap();
        let unit = storage
            .take_completed_unit(&mut live, frontier.descriptor_count)
            .unwrap()
            .expect("completed unit");

        drop(unit);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn frozen_last_releases_the_descriptor_equal_to_last() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        assert_eq!(live.recycle_start(), 0);
        assert_eq!(live.accepted_tail(), COUNT - 1);

        storage.descriptors()[0].write_word0(
            16 | (4 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
        mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        let frozen_cursor = live.freeze_cursor(&mut mmio);
        let unit = storage
            .take_completed_unit(&mut live, 1)
            .expect("completed descriptor")
            .expect("descriptor owner");
        unit.retain_for_deferred_recycle();

        let append = storage
            .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, frozen_cursor, 1)
            .expect("frozen LAST reclaim")
            .expect("LAST itself releases the observed descriptor");
        assert_eq!(append.head_index, 0);
        assert_eq!(append.descriptor_count, 1);
        assert_eq!(
            storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
            0
        );
        assert_eq!(live.observed_mask(), 0);
        live.complete_pending_reload(&mut mmio)
            .expect("vendor reload suffix");
        assert_eq!(live.accepted_tail(), 0);
        assert!(live.topology_snapshot().valid);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn frozen_last_unit_reclaim_returns_only_one_vendor_chain() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");

        for index in 0..2 {
            storage.descriptors()[index].write_word0(
                16 | (4 << crate::descriptor::LENGTH_SHIFT)
                    | crate::descriptor::BIT_30
                    | crate::descriptor::BIT_31,
            );
        }
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low =
            (BASE + 2 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        let cursor = live.freeze_cursor(&mut mmio);

        storage
            .take_completed_unit(&mut live, 1)
            .expect("first unit inspection")
            .expect("first unit owner")
            .retain_for_deferred_recycle();
        let append = storage
            .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 1)
            .expect("first vendor unit reclaim")
            .expect("first unit precedes LAST");
        assert_eq!(append.head_index, 0);
        assert_eq!(append.descriptor_count, 1);
        assert_eq!(live.observed_mask(), 0);
        assert_ne!(
            storage.descriptors()[1].word0() & crate::descriptor::BIT_30,
            0,
            "the following complete unit must remain a distinct vendor chain"
        );
        live.complete_pending_reload(&mut mmio)
            .expect("first vendor reload suffix");

        storage
            .take_completed_unit(&mut live, 1)
            .expect("second unit inspection")
            .expect("second unit owner")
            .retain_for_deferred_recycle();
        assert!(
            storage
                .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 1,)
                .expect("stale generation check")
                .is_none(),
            "one frozen cursor must not authorize a second append generation"
        );
        let refreshed = live.freeze_cursor(&mut mmio);
        storage
            .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, refreshed, 1)
            .expect("refreshed cursor check")
            .expect("refreshed LAST releases the second vendor unit");
        live.complete_pending_reload(&mut mmio)
            .expect("second vendor reload suffix");
        assert!(live.topology_snapshot().valid);
        assert!(live.try_stop(&mut mmio).is_ok());
    }

    #[test]
    fn ambiguous_start_quarantines_the_arena_when_walker_is_observed_live() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma {
            ambiguous_enable: true,
            ..MockRxDma::default()
        };
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");

        let (_prepared, error) = match prepared.try_start(&mut mmio) {
            Ok(_) => panic!("ambiguous enable is not a live capability"),
            Err(failure) => failure,
        };
        assert_eq!(error, RxRingError::Busy);
        assert!(mmio.walker);
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    }
}
