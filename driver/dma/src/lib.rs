#![no_std]
#![deny(unsafe_code)]

//! Audited storage primitives for radio pipelines.
//!
//! This crate deliberately does not know about a chip, executor, network
//! stack, descriptor layout or allocator. [`StableDmaBacking`] proves that a
//! retained TX allocation cannot move, while [`RxHandoffPool`] transfers one
//! final receive buffer through finite state-specific leases. Higher protocol
//! and integration crates use only these safe capabilities.

mod pinned_tx;
mod rx_handoff;

pub use pinned_tx::{
    DmaIndexReturn, IndexedStableDmaLease, PinnedDmaTxNetworkLease, PinnedDmaTxPool,
    PinnedDmaTxRadioLease, ReturningStableDmaBacking,
};
pub use rx_handoff::{RxHandoffPool, RxNetworkLease, RxRadioLease};

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

    /// Consume the short region view and retain the original backing borrow.
    ///
    /// This is useful when an owner must return a slice borrowed from its
    /// retained lease instead of from the temporary region wrapper.
    pub fn into_mut_slice(self) -> &'a mut [u8] {
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
