//! Permanently located TX storage and its finite ownership transitions.

use core::{
    cell::UnsafeCell,
    marker::PhantomPinned,
    ops::{Deref, DerefMut},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

use crate::{StableDmaBacking, StableDmaRegion};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;
const DMA_OVERRUN_GUARD_SIZE: usize = 32;
const DMA_OVERRUN_GUARD_BYTE: u8 = 0xa5;

#[repr(C)]
struct PinnedTxBytes<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    headroom: [u8; HEADROOM],
    ethernet: [u8; FRAME_CAPACITY],
    trailer: [u8; TRAILER],
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            headroom: [0; HEADROOM],
            ethernet: [0; FRAME_CAPACITY],
            trailer: [0; TRAILER],
        }
    }
}

#[repr(C, align(16))]
struct PinnedTxSlot<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    bytes: UnsafeCell<PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>>,
    // Hardware only owns `bytes`. Keep a checked boundary before CPU-only
    // ownership metadata so a late or overlong DMA write fails closed instead
    // of silently turning a free slot into a forged state transition.
    dma_overrun_guard: UnsafeCell<[u8; DMA_OVERRUN_GUARD_SIZE]>,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new(PinnedTxBytes::new()),
            // DMA pools normally live in a zero-initialized linker section.
            // `pin_static` installs the non-zero diagnostic marker before the
            // allocation can be published to hardware.
            dma_overrun_guard: UnsafeCell::new([DMA_OVERRUN_GUARD_BYTE; DMA_OVERRUN_GUARD_SIZE]),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim(&self, index: u8, from: u8, to: u8, message: &str) {
        self.assert_dma_boundary(index);
        let observed = self
            .state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire);
        assert_eq!(
            observed,
            Ok(from),
            "{message}; slot={index} expected={from} requested={to}"
        );
    }

    #[allow(
        unsafe_code,
        reason = "the guard observes whether a hardware actor crossed the DMA region boundary"
    )]
    fn assert_dma_boundary(&self, index: u8) {
        // SAFETY: CPU writers never mutate this guard after initialization.
        // A differing byte is diagnostic evidence that the external DMA actor
        // violated the region exposed by StableDmaBacking; no recovery follows.
        let guard = unsafe { &*self.dma_overrun_guard.get() };
        if let Some((offset, byte)) = guard
            .iter()
            .copied()
            .enumerate()
            .find(|(_, byte)| *byte != DMA_OVERRUN_GUARD_BYTE)
        {
            let prefix = u64::from_le_bytes(guard[..8].try_into().unwrap());
            panic!(
                "pinned TX DMA crossed its backing boundary; slot={index} offset={offset} value={byte} guard_prefix={prefix:#018x}"
            );
        }
    }

    #[allow(
        unsafe_code,
        reason = "the pool audit observes whether a hardware actor crossed a DMA region boundary"
    )]
    fn dma_boundary_damage(&self) -> Option<(usize, u8)> {
        // SAFETY: see `assert_dma_boundary`; the guard is never a CPU write
        // target after the pool has been pinned.
        let guard = unsafe { &*self.dma_overrun_guard.get() };
        guard
            .iter()
            .copied()
            .enumerate()
            .find(|(_, byte)| *byte != DMA_OVERRUN_GUARD_BYTE)
    }

    #[allow(
        unsafe_code,
        reason = "the diagnostic reads the same hardware-observable guard as dma_boundary_damage"
    )]
    fn dma_boundary_prefix(&self) -> u64 {
        // SAFETY: see `assert_dma_boundary`; this is an observation-only read.
        let guard = unsafe { &*self.dma_overrun_guard.get() };
        u64::from_le_bytes(guard[..8].try_into().unwrap())
    }

    fn publish_ready(&self, index: u8, length: usize) {
        self.assert_dma_boundary(index);
        self.length.store(length, Ordering::Relaxed);
        let observed = self.state.compare_exchange(
            SLOT_NETWORK,
            SLOT_READY,
            Ordering::Release,
            Ordering::Acquire,
        );
        assert_eq!(
            observed,
            Ok(SLOT_NETWORK),
            "only the network lease may publish a pinned TX slot; slot={index}"
        );
    }

    fn release_radio(&self, index: u8) {
        self.assert_dma_boundary(index);
        self.length.store(0, Ordering::Relaxed);
        self.claim(
            index,
            SLOT_RADIO,
            SLOT_FREE,
            "only the radio lease may return a pinned TX slot",
        );
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    #[allow(
        unsafe_code,
        reason = "TX slot exposes its contiguous storage to a typed lease"
    )]
    fn storage(&self) -> &[u8] {
        // SAFETY: the caller holds the unique state-specific lease. The three
        // byte-aligned repr(C) fields form one padding-free allocation.
        unsafe {
            let bytes = &*self.bytes.get();
            debug_assert_eq!(
                core::mem::size_of_val(bytes),
                HEADROOM + FRAME_CAPACITY + TRAILER
            );
            core::slice::from_raw_parts(
                ptr::addr_of!(bytes.headroom).cast::<u8>(),
                HEADROOM + FRAME_CAPACITY + TRAILER,
            )
        }
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }

    const fn storage_capacity(&self) -> usize {
        HEADROOM + FRAME_CAPACITY + TRAILER
    }
}

// SAFETY: all access to the UnsafeCell is gated by the atomic state machine
// and by non-Clone state-specific leases.
#[allow(unsafe_code, reason = "TX slot state machine is its Sync boundary")]
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Sync
    for PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
}

/// Permanently located TX allocations exposed to radio DMA.
pub struct PinnedDmaTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    slots: [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    _pin: PhantomPinned,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { PinnedTxSlot::new() }; QUEUE_DEPTH],
            _pin: PhantomPinned,
        }
    }

    pub fn pin_static(storage: &'static mut Self) -> Pin<&'static mut Self> {
        for slot in &mut storage.slots {
            *slot.dma_overrun_guard.get_mut() = [DMA_OVERRUN_GUARD_BYTE; DMA_OVERRUN_GUARD_SIZE];
        }
        Pin::static_mut(storage)
    }

    pub fn claim_network(
        &self,
        index: u8,
    ) -> PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER> {
        if let Some((damaged_slot, (offset, byte))) = self
            .slots
            .iter()
            .enumerate()
            .find_map(|(slot, value)| value.dma_boundary_damage().map(|damage| (slot, damage)))
        {
            let prefix = self.slots[damaged_slot].dma_boundary_prefix();
            panic!(
                "pinned TX pool boundary changed before a network claim; requested={index} damaged_slot={damaged_slot} offset={offset} value={byte} guard_prefix={prefix:#018x}"
            );
        }
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("pinned TX index belongs to this pool");
        slot.claim(
            index,
            SLOT_FREE,
            SLOT_NETWORK,
            "free-channel entry did not name a free pinned TX slot",
        );
        PinnedDmaTxNetworkLease {
            slot,
            index,
            live: true,
        }
    }

    pub fn claim_radio(
        &self,
        index: u8,
    ) -> PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("pinned TX index belongs to this pool");
        slot.claim(
            index,
            SLOT_READY,
            SLOT_RADIO,
            "ready-channel entry did not name a ready pinned TX slot",
        );
        PinnedDmaTxRadioLease {
            slot,
            index,
            live: true,
        }
    }

    /// Number of slots retained by a network, ready-queue or radio stage.
    ///
    /// This is observation only and grants no right to recover a quarantined
    /// radio slot. A platform reset path needs a separate ownership proof
    /// before such a transition can be added.
    pub fn claimed_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) != SLOT_FREE)
            .count()
    }
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Default for PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Unique network writer for one free TX slot.
pub struct PinnedDmaTxNetworkLease<
    'pool,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
> {
    slot: &'pool PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    pub fn publish<R>(mut self, length: usize, write: impl FnOnce(&mut [u8]) -> R) -> (u8, R) {
        assert!(length <= FRAME_CAPACITY, "TX frame exceeds slot capacity");
        // SAFETY: this non-Clone Network lease is the unique SLOT_NETWORK
        // owner. No radio lease can exist before `publish_ready` below.
        #[allow(unsafe_code, reason = "network lease uniquely initializes its TX slot")]
        let storage = unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr(),
                self.slot.storage_capacity(),
            )
        };
        let result = write(&mut storage[HEADROOM..HEADROOM + length]);
        self.slot.publish_ready(self.index, length);
        self.live = false;
        (self.index, result)
    }

    pub fn release(mut self) -> u8 {
        self.slot.claim(
            self.index,
            SLOT_NETWORK,
            SLOT_FREE,
            "only the network lease may return a pinned TX slot",
        );
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Drop
    for PinnedDmaTxNetworkLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.claim(
                self.index,
                SLOT_NETWORK,
                SLOT_FREE,
                "only the live network lease may return a pinned TX slot",
            );
        }
    }
}

/// Unique radio/DMA owner for one ready TX allocation.
pub struct PinnedDmaTxRadioLease<
    'pool,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
> {
    slot: &'pool PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    pub const fn ethernet_offset(&self) -> usize {
        HEADROOM
    }

    pub fn ethernet_length(&self) -> usize {
        self.slot.length()
    }

    pub fn len(&self) -> usize {
        self.ethernet_length()
    }

    pub fn is_empty(&self) -> bool {
        self.ethernet_length() == 0
    }

    pub fn ethernet(&self) -> &[u8] {
        let length = self.ethernet_length();
        &self.slot.storage()[HEADROOM..HEADROOM + length]
    }

    pub fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    pub fn ethernet_mut(&mut self) -> &mut [u8] {
        let length = self.ethernet_length();
        &mut self.storage_mut()[HEADROOM..HEADROOM + length]
    }

    pub fn storage_mut(&mut self) -> &mut [u8] {
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_RADIO lease.
        #[allow(unsafe_code, reason = "radio lease uniquely owns its pinned TX slot")]
        unsafe {
            core::slice::from_raw_parts_mut(
                self.slot.storage_mut_ptr(),
                self.slot.storage_capacity(),
            )
        }
    }

    pub const fn trailer_capacity(&self) -> usize {
        TRAILER
    }

    pub fn release(mut self) -> u8 {
        self.slot.release_radio(self.index);
        self.live = false;
        self.index
    }

    fn requeue(mut self) -> u8 {
        self.slot.claim(
            self.index,
            SLOT_RADIO,
            SLOT_READY,
            "only the live radio lease may requeue a pinned TX slot",
        );
        self.live = false;
        self.index
    }
}

// SAFETY: this non-Clone lease retains a separately pinned pool slot in the
// Radio state. Moving the lease cannot move or release its allocation.
#[allow(
    unsafe_code,
    reason = "radio lease proves the stable DMA backing contract"
)]
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    StableDmaBacking for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        // SAFETY: the Radio lease exclusively retains this pinned allocation.
        #[allow(unsafe_code, reason = "radio lease retains the pinned allocation")]
        unsafe {
            StableDmaRegion::new(self.storage_mut())
        }
    }
}

/// Stable DMA lease which yields its pool index when explicitly released.
pub trait IndexedStableDmaLease: StableDmaBacking {
    fn release_index(self) -> u8;
}

/// Stable lease which can return an unmodified radio claim to its ready queue.
pub trait RequeueStableDmaLease: IndexedStableDmaLease {
    fn requeue_index(self) -> u8;
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> IndexedStableDmaLease
    for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn release_index(self) -> u8 {
        self.release()
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> RequeueStableDmaLease
    for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn requeue_index(self) -> u8 {
        self.requeue()
    }
}

/// Safe integration callback which returns a released pool index to its queue.
pub trait DmaIndexReturn {
    fn return_index(&self, index: u8);
}

/// Stable backing plus the integration-specific queue return capability.
///
/// Dropping this value first releases hardware ownership and only then makes
/// the index available to another producer.
pub struct ReturningStableDmaBacking<B: IndexedStableDmaLease, R: DmaIndexReturn> {
    backing: Option<B>,
    returner: R,
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> ReturningStableDmaBacking<B, R> {
    pub const fn new(backing: B, returner: R) -> Self {
        Self {
            backing: Some(backing),
            returner,
        }
    }

    /// Return an unmodified radio claim to its producer without publishing
    /// its index through the normal free-slot capability.
    ///
    /// This is used by a queue owner which immediately republishes the same
    /// index to a different owned queue. Taking the backing first makes the
    /// wrapper's `Drop` implementation a no-op, so the index cannot be
    /// returned twice.
    pub fn take_requeued_index(mut self) -> u8
    where
        B: RequeueStableDmaLease,
    {
        self.backing
            .take()
            .expect("returning DMA backing remains live until requeue")
            .requeue_index()
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> Deref for ReturningStableDmaBacking<B, R> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        self.backing
            .as_ref()
            .expect("returning DMA backing remains live until drop")
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> DerefMut for ReturningStableDmaBacking<B, R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.backing
            .as_mut()
            .expect("returning DMA backing remains live until drop")
    }
}

// SAFETY: the wrapper retains the exact stable backing and exposes no
// operation which can release it before this owner is dropped.
#[allow(
    unsafe_code,
    reason = "wrapper retains the same audited stable DMA owner"
)]
unsafe impl<B: IndexedStableDmaLease, R: DmaIndexReturn> StableDmaBacking
    for ReturningStableDmaBacking<B, R>
{
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        self.deref_mut().stable_dma_region()
    }
}

impl<B: IndexedStableDmaLease, R: DmaIndexReturn> Drop for ReturningStableDmaBacking<B, R> {
    fn drop(&mut self) {
        if let Some(backing) = self.backing.take() {
            let index = backing.release_index();
            self.returner.return_index(index);
        }
    }
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Drop
    for PinnedDmaTxRadioLease<'_, FRAME_CAPACITY, HEADROOM, TRAILER>
{
    fn drop(&mut self) {
        if self.live {
            self.slot.release_radio(self.index);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::*;

    type TestPool = PinnedDmaTxPool<32, 8, 4, 1>;

    struct ReturnProbe<'pool> {
        pool: &'pool TestPool,
        returned: &'pool Cell<Option<u8>>,
    }

    impl DmaIndexReturn for ReturnProbe<'_> {
        fn return_index(&self, index: u8) {
            assert_eq!(
                self.pool.slots[usize::from(index)]
                    .state
                    .load(Ordering::Acquire),
                SLOT_FREE,
                "the backing must release its slot before queue publication"
            );
            self.returned.set(Some(index));
        }
    }

    fn prepared_radio(pool: &TestPool) -> PinnedDmaTxRadioLease<'_, 32, 8, 4> {
        let network = pool.claim_network(0);
        let (index, ()) = network.publish(4, |frame| frame.copy_from_slice(&[1, 2, 3, 4]));
        pool.claim_radio(index)
    }

    #[test]
    fn dropped_stage_leases_restore_the_slot() {
        let pool = TestPool::new();
        drop(pool.claim_network(0));

        let radio = prepared_radio(&pool);
        assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
        drop(radio);

        assert_eq!(pool.claim_network(0).release(), 0);
    }

    #[test]
    fn returning_backing_releases_before_publishing_its_index() {
        let pool = TestPool::new();
        let returned = Cell::new(None);
        let backing = ReturningStableDmaBacking::new(
            prepared_radio(&pool),
            ReturnProbe {
                pool: &pool,
                returned: &returned,
            },
        );

        assert_eq!(returned.get(), None);
        drop(backing);
        assert_eq!(returned.get(), Some(0));
        assert_eq!(pool.claim_network(0).release(), 0);
    }

    #[test]
    fn forgotten_backing_remains_quarantined() {
        let pool = TestPool::new();
        let returned = Cell::new(None);
        let backing = ReturningStableDmaBacking::new(
            prepared_radio(&pool),
            ReturnProbe {
                pool: &pool,
                returned: &returned,
            },
        );

        core::mem::forget(backing);

        assert_eq!(returned.get(), None);
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(pool.slots[0].state.load(Ordering::Acquire), SLOT_RADIO);
    }
}
