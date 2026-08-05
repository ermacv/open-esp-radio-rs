//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. Descriptor count,
//! payload capacity and placement policy are selected by the board or runtime
//! composition and remain const-generic here.

use core::cell::UnsafeCell;

use crate::{
    descriptor::Descriptor,
    rx_dma::RxDma,
    rx_ring::{
        RX_BUFFER_SENTINEL, RxCompletedDescriptor, RxCompletedUnit, RxCompletedUnitFrontier,
        RxLiveAppend, RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment,
        prepare_recycled_buffer,
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
    pub fn recycle<M: RxDma>(self, mmio: &mut M) -> Result<Option<RxLiveAppend>, RxRingError> {
        let descriptor_count = self.unit.descriptor_count();
        self.ring
            .recycle_completed_unit(mmio, descriptor_count, |index| {
                // SAFETY: this consuming token owns every segment until this
                // rearm closure returns it to the live walker.
                unsafe { self.storage.buffers[index].prepare_for_recycle() }
            })
    }
}

/// Descriptor and buffer arena whose address remains stable for a DMA epoch.
///
/// The buffer address table remains caller-owned because the live ring borrows
/// it for its entire epoch. Keeping that table separate avoids a
/// self-referential owner and lets a platform place only DMA-visible storage
/// in its dedicated linker section.
pub struct RxDmaStorage<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> {
    descriptors: [Descriptor; COUNT],
    buffers: [RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>; COUNT],
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; COUNT],
            buffers: [const { RxDmaBuffer::new() }; COUNT],
        }
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
        let buffer_size = u32::try_from(BUFFER_SIZE).map_err(|_| RxRingError::Size)?;
        RxRingStopped::prepare(
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
        )
    }

    /// Rebuild a halted ring only when it belongs to this storage arena.
    #[allow(unsafe_code, reason = "halted ring exclusively owns buffer rearm")]
    pub fn prepare_halted<'storage, M: RxDma>(
        &'storage self,
        ring: RxRingHalted<'storage, COUNT>,
        mmio: &mut M,
    ) -> Result<RxRingStopped<'storage, COUNT>, (RxRingHalted<'storage, COUNT>, RxRingError)> {
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
        ring.prepare(mmio, buffer_size, |index| {
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
        let Some(descriptor) = ring.take_completed(index) else {
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
        ring.recycle_completed_half(mmio, |index| {
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
        ring.recycle_completed_prefix::<MAX_BATCH, _, _>(mmio, |index| {
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

    pub fn take_completed_unit<'owner, 'ring>(
        &'owner self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        descriptor_limit: usize,
    ) -> Result<
        Option<RxDmaCompletedUnit<'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        let Some(unit) = ring.take_completed_unit(descriptor_limit) else {
            return Ok(None);
        };
        Ok(Some(RxDmaCompletedUnit {
            unit,
            storage: self,
            ring,
        }))
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

    #[test]
    fn arena_initializes_in_its_final_location_and_recycles_one_buffer() {
        let mut storage = RxDmaStorage::<2, 16, 20>::new();

        assert_eq!(storage.descriptors().len(), 2);
        assert_eq!(storage.buffers().len(), 2);
        assert_eq!(storage.buffers().as_ptr().addr() & 3, 0);

        storage.prepare_unpublished_buffer(0).unwrap();
        assert!(!storage.buffers()[0].leading_guard_overwritten());
    }
}
