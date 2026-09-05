//! Pointer-free-to-users ownership of DMA buffers retained across tasks.
//!
//! A chip DMA owner creates an [`ExternalRxBuffer`] only after hardware has
//! released the matching descriptor. The bounded pool then transfers that
//! exact buffer from the radio/protocol owner to the network owner. No payload
//! bytes are copied and no safe API exposes the release callback or raw owner
//! context.

use core::{
    cell::UnsafeCell,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;
const SLOT_RELEASING: u8 = 4;

/// One stable external receive buffer and its affine return edge.
pub struct ExternalRxBuffer {
    pointer: NonNull<u8>,
    length: usize,
    capacity: usize,
    owner: NonNull<()>,
    owner_index: usize,
    release: unsafe fn(NonNull<()>, usize),
    live: bool,
}

impl ExternalRxBuffer {
    /// Construct a buffer after a DMA owner has detached it from hardware.
    ///
    /// # Safety
    ///
    /// `pointer..pointer + capacity` must remain allocated and exclusively
    /// CPU-owned until `release(owner, owner_index)` is called exactly once.
    /// The callback must make the allocation eligible for DMA reuse only
    /// after this value has relinquished all access to it.
    #[allow(
        unsafe_code,
        reason = "chip DMA storage establishes the stable detached-buffer proof"
    )]
    pub unsafe fn new(
        pointer: NonNull<u8>,
        length: usize,
        capacity: usize,
        owner: NonNull<()>,
        owner_index: usize,
        release: unsafe fn(NonNull<()>, usize),
    ) -> Self {
        assert!(length <= capacity, "external RX length exceeds capacity");
        Self {
            pointer,
            length,
            capacity,
            owner,
            owner_index,
            release,
            live: true,
        }
    }

    pub const fn length(&self) -> usize {
        self.length
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    fn frame(&self, offset: usize, length: usize) -> &[u8] {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= self.length)
            .expect("external RX view stays inside initialized bytes");
        let _ = end;
        // SAFETY: the affine value retains exclusive CPU ownership and the
        // checked range lies inside its stable allocation.
        #[allow(unsafe_code, reason = "affine external buffer owns this range")]
        unsafe {
            core::slice::from_raw_parts(self.pointer.as_ptr().add(offset), length)
        }
    }

    fn frame_mut(&mut self, offset: usize, length: usize) -> &mut [u8] {
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= self.length)
            .expect("external RX mutable view stays inside initialized bytes");
        let _ = end;
        // SAFETY: `&mut self` borrows the sole affine owner and the range was
        // checked against initialized bytes.
        #[allow(unsafe_code, reason = "affine external buffer owns this range")]
        unsafe {
            core::slice::from_raw_parts_mut(self.pointer.as_ptr().add(offset), length)
        }
    }

    fn release_once(&mut self) {
        if !self.live {
            return;
        }
        self.live = false;
        // SAFETY: the constructor bound this exact callback and owner identity
        // to the allocation. `live` makes this the sole invocation.
        #[allow(
            unsafe_code,
            reason = "affine token invokes its bound return edge once"
        )]
        unsafe {
            (self.release)(self.owner, self.owner_index);
        }
    }
}

impl Drop for ExternalRxBuffer {
    fn drop(&mut self) {
        self.release_once();
    }
}

// SAFETY: the chip owner promises stable storage and cross-core ownership is
// transferred only by the pool's release/acquire state transitions.
#[allow(
    unsafe_code,
    reason = "affine external buffer may cross executor cores"
)]
unsafe impl Send for ExternalRxBuffer {}

struct ExternalRxHandoffSlot {
    buffer: UnsafeCell<Option<ExternalRxBuffer>>,
    offset: UnsafeCell<usize>,
    length: UnsafeCell<usize>,
    state: AtomicU8,
}

impl ExternalRxHandoffSlot {
    const fn new() -> Self {
        Self {
            buffer: UnsafeCell::new(None),
            offset: UnsafeCell::new(0),
            length: UnsafeCell::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn try_claim_radio(&self, buffer: ExternalRxBuffer) -> Result<(), ExternalRxBuffer> {
        if self
            .state
            .compare_exchange(SLOT_FREE, SLOT_RADIO, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err(buffer);
        }
        // SAFETY: SLOT_RADIO is exclusively owned by this successful claim;
        // no consumer can observe the binding before the Release publication.
        #[allow(unsafe_code, reason = "slot state exclusively owns binding mutation")]
        unsafe {
            *self.buffer.get() = Some(buffer);
            *self.offset.get() = 0;
            *self.length.get() = (*self.buffer.get())
                .as_ref()
                .expect("claimed external slot contains its buffer")
                .length();
        }
        Ok(())
    }

    fn publish(&self, owner: u8, offset: usize, length: usize) {
        let initialized = self.initialized_length();
        let end = offset
            .checked_add(length)
            .filter(|&end| end <= initialized)
            .expect("published external RX range stays initialized");
        let _ = end;
        // SAFETY: the current affine owner is the only stage allowed to alter
        // the published view before its Release transition.
        #[allow(unsafe_code, reason = "slot owner exclusively mutates range metadata")]
        unsafe {
            *self.offset.get() = offset;
            *self.length.get() = length;
        }
        assert_eq!(
            self.state
                .compare_exchange(owner, SLOT_READY, Ordering::Release, Ordering::Acquire,),
            Ok(owner),
            "only the current external RX owner may publish"
        );
    }

    fn claim_network(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_READY,
                SLOT_NETWORK,
                Ordering::Acquire,
                Ordering::Relaxed,
            ),
            Ok(SLOT_READY),
            "ready index did not name an external RX slot"
        );
    }

    fn initialized_length(&self) -> usize {
        // SAFETY: every non-free state contains an initialized binding. The
        // state transition which published it supplies the acquire ordering.
        #[allow(unsafe_code, reason = "non-free slot retains its external binding")]
        unsafe {
            (*self.buffer.get())
                .as_ref()
                .expect("claimed external slot contains a buffer")
                .length()
        }
    }

    fn frame(&self) -> &[u8] {
        // SAFETY: a live lease uniquely owns the non-free slot state.
        #[allow(unsafe_code, reason = "live lease retains external slot ownership")]
        unsafe {
            let offset = *self.offset.get();
            let length = *self.length.get();
            (*self.buffer.get())
                .as_ref()
                .expect("claimed external slot contains a buffer")
                .frame(offset, length)
        }
    }

    fn with_frame<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        // SAFETY: the caller's affine lease requires unique mutable access,
        // proving exclusive ownership of the current non-free allocation.
        #[allow(unsafe_code, reason = "affine lease uniquely owns external bytes")]
        unsafe {
            let offset = *self.offset.get();
            let length = *self.length.get();
            let buffer = (*self.buffer.get())
                .as_mut()
                .expect("claimed external slot contains a buffer");
            f(buffer.frame_mut(offset, length))
        }
    }

    fn release(&self, owner: u8) {
        assert_eq!(
            self.state.compare_exchange(
                owner,
                SLOT_RELEASING,
                Ordering::Acquire,
                Ordering::Acquire,
            ),
            Ok(owner),
            "only the current external RX owner may release"
        );
        // SLOT_RELEASING excludes a new producer until the old binding has
        // been removed. The affine buffer's Drop then publishes its separate
        // DMA-release state.
        #[allow(unsafe_code, reason = "free slot exclusively owns binding removal")]
        let buffer = unsafe {
            *self.offset.get() = 0;
            *self.length.get() = 0;
            (*self.buffer.get()).take()
        };
        // The DMA allocation must become releasable before this handoff slot
        // is advertised as a new radio credit. Publishing FREE first creates
        // a cross-core window in which a new detached buffer can claim this
        // slot while the old descriptor buffer is still DETACHED, violating
        // the pool's upper bound on retained physical DMA credits.
        drop(buffer);
        self.state.store(SLOT_FREE, Ordering::Release);
    }
}

// SAFETY: atomic slot ownership serializes every access to UnsafeCell fields;
// Release/Acquire publishes both the binding and its bytes across cores.
#[allow(
    unsafe_code,
    reason = "slot state machine is the external pool Sync boundary"
)]
unsafe impl Sync for ExternalRxHandoffSlot {}

/// Bounded index pool for DMA buffers retained above the descriptor ring.
pub struct ExternalRxHandoffPool<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> {
    slots: [ExternalRxHandoffSlot; SLOT_COUNT],
}

impl<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize>
    ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { ExternalRxHandoffSlot::new() }; SLOT_COUNT],
        }
    }

    pub fn try_claim_radio(
        &self,
        buffer: ExternalRxBuffer,
        start: usize,
    ) -> Result<ExternalRxRadioLease<'_, FRAME_CAPACITY>, ExternalRxBuffer> {
        assert!(
            buffer.length() <= FRAME_CAPACITY,
            "external RX frame exceeds the handoff profile"
        );
        if SLOT_COUNT == 0 || SLOT_COUNT > usize::from(u8::MAX) + 1 {
            return Err(buffer);
        }
        let mut buffer = Some(buffer);
        for distance in 0..SLOT_COUNT {
            let index = (start + distance) % SLOT_COUNT;
            let candidate = buffer
                .take()
                .expect("unclaimed external buffer remains affine");
            match self.slots[index].try_claim_radio(candidate) {
                Ok(()) => {
                    return Ok(ExternalRxRadioLease {
                        slot: &self.slots[index],
                        index: index as u8,
                        live: true,
                    });
                }
                Err(candidate) => buffer = Some(candidate),
            }
        }
        Err(buffer.expect("failed claim returns the external buffer"))
    }

    pub fn claim_network(&self, index: u8) -> ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("external RX index belongs to its pool");
        slot.claim_network();
        ExternalRxNetworkLease {
            slot,
            index,
            live: true,
        }
    }

    pub fn claimed_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) != SLOT_FREE)
            .count()
    }

    pub fn network_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Acquire) == SLOT_NETWORK)
            .count()
    }
}

impl<const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> Default
    for ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct ExternalRxRadioLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool ExternalRxHandoffSlot,
    index: u8,
    live: bool,
}

impl<'pool, const FRAME_CAPACITY: usize> ExternalRxRadioLease<'pool, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame(&self) -> &[u8] {
        self.slot.frame()
    }

    pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        self.slot.with_frame(f)
    }

    /// Publish the final protocol-owned view for a later network claim.
    ///
    /// The radio/protocol owner remains affine until this transition. Unlike
    /// an eager `RADIO -> READY -> NETWORK` handoff, this does not fabricate a
    /// network owner while Core0 is still decoding and transforming the MPDU.
    pub fn republish(mut self, offset: usize, length: usize) -> u8 {
        self.slot.publish(SLOT_RADIO, offset, length);
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for ExternalRxRadioLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(SLOT_RADIO);
        }
    }
}

pub struct ExternalRxNetworkLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool ExternalRxHandoffSlot,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize> ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame(&self) -> &[u8] {
        self.slot.frame()
    }

    pub fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        self.slot.with_frame(f)
    }

    pub fn republish(mut self, offset: usize, length: usize) -> u8 {
        self.slot.publish(SLOT_NETWORK, offset, length);
        self.live = false;
        self.index
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(SLOT_NETWORK);
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for ExternalRxNetworkLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(SLOT_NETWORK);
        }
    }
}

#[cfg(test)]
mod tests;
