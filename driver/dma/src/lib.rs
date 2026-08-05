#![no_std]
#![deny(unsafe_code)]

//! Minimal ownership contract for memory whose address remains stable while a
//! radio transaction retains its backing owner.
//!
//! This crate deliberately does not know about a chip, executor, network
//! stack, descriptor layout or allocator. A concrete pinned pool implements
//! [`StableDmaBacking`] at its own audited memory boundary; chip LMAC code can
//! then retain that owner instead of relying on an external lifetime comment.

/// Exclusive view of one DMA-capable region at its stable address.
///
/// The view itself is ordinary borrowed memory. Address stability beyond this
/// short borrow is supplied by the [`StableDmaBacking`] implementation and is
/// valid while the backing owner remains alive and exclusively retained.
pub struct StableDmaRegion<'a> {
    storage: &'a mut [u8],
}

impl<'a> StableDmaRegion<'a> {
    /// Construct a view after proving that the allocation does not move while
    /// its backing owner is retained.
    ///
    /// # Safety
    ///
    /// `storage` must keep the same address until the `StableDmaBacking`
    /// owner which returned this region is dropped or explicitly released.
    #[allow(
        unsafe_code,
        reason = "constructor is the single stable-address proof boundary"
    )]
    pub const unsafe fn new(storage: &'a mut [u8]) -> Self {
        Self { storage }
    }

    pub const fn len(&self) -> usize {
        self.storage.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.storage
    }
}

/// An owned lease whose DMA region remains at one address until the lease is
/// dropped.
///
/// # Safety
///
/// Implementations must return the same non-moving allocation for the entire
/// lifetime of `self`. No safe operation may release, relocate or alias that
/// allocation while the owner is retained by a hardware transaction.
#[allow(
    unsafe_code,
    reason = "implementing the stable-address invariant requires an audited pinned pool"
)]
pub unsafe trait StableDmaBacking {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_>;
}
