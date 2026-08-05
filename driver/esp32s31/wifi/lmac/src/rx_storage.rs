//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. Descriptor count,
//! payload capacity and placement policy are selected by the board or runtime
//! composition and remain const-generic here.

#![allow(unsafe_code, reason = "RX DMA storage boundary")]

use core::cell::UnsafeCell;

use crate::{
    descriptor::Descriptor,
    rx::{RX_BUFFER_SENTINEL, RxRingError, prepare_recycled_buffer},
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

    pub fn dma_address(&self) -> Result<u32, RxDmaStorageError> {
        u32::try_from(self.0.get().addr()).map_err(|_| RxDmaStorageError::AddressWidth)
    }

    /// The caller must own the matching completed descriptor. The returned
    /// view must not survive descriptor recycle.
    pub unsafe fn completed(&self) -> &[u8; BUFFER_SIZE] {
        // SAFETY: the type guarantees a prefix of exactly this size.
        unsafe { &*self.0.get().cast::<[u8; BUFFER_SIZE]>() }
    }

    /// The caller must own the matching completed descriptor and invoke this
    /// only from the ring's rearm closure.
    pub unsafe fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        // SAFETY: ring ownership makes this the only CPU or DMA writer.
        unsafe { prepare_recycled_buffer(&mut *self.0.get(), BUFFER_SIZE) }
    }

    /// Whether DMA has overwritten the leading recycle guard.
    ///
    /// This is observation only: it never transfers buffer ownership. It is
    /// used together with a later terminal descriptor to distinguish a full
    /// non-terminal segment from an untouched armed descriptor.
    pub fn leading_guard_overwritten(&self) -> bool {
        // SAFETY: volatile access models the asynchronous DMA writer. The
        // result is only evidence for a subsequent descriptor observation.
        unsafe { self.0.get().cast::<u32>().read_volatile() != RX_BUFFER_SENTINEL }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDmaStorageError {
    AddressWidth,
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

    pub fn dma_layout(
        &self,
        buffer_addresses: &mut [u32; COUNT],
    ) -> Result<u32, RxDmaStorageError> {
        for (address, buffer) in buffer_addresses.iter_mut().zip(&self.buffers) {
            *address = buffer.dma_address()?;
        }
        u32::try_from(self.descriptors.as_ptr().addr()).map_err(|_| RxDmaStorageError::AddressWidth)
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
        let storage = RxDmaStorage::<2, 16, 20>::new();

        assert_eq!(storage.descriptors().len(), 2);
        assert_eq!(storage.buffers().len(), 2);
        assert_eq!(storage.buffers().as_ptr().addr() & 3, 0);

        // SAFETY: no DMA walker exists in this unit test and this buffer has
        // not been published to any other owner.
        unsafe { storage.buffers()[0].prepare_for_recycle().unwrap() };
        assert!(!storage.buffers()[0].leading_guard_overwritten());
    }
}
