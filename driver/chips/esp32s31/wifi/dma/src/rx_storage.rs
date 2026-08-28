//! Permanently located storage for an ESP32-S31 RX DMA ring.
//!
//! This module owns only the chip DMA memory representation. Descriptor count,
//! payload capacity and placement policy are selected by the board or runtime
//! composition and remain const-generic here.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use open_esp_radio_dma::ExternalRxBuffer;

use crate::{
    descriptor::Descriptor,
    rx_dma::RxDma,
    rx_ring::{
        RX_BUFFER_SENTINEL, RxCompletedDescriptor, RxCompletedUnit, RxCompletedUnitFrontier,
        RxDmaArenaState, RxDmaBufferAddresses, RxFrozenCursor, RxLiveAppend, RxRingError,
        RxRingHalted, RxRingLive, RxRingStopped, RxSegment, prepare_recycled_buffer,
    },
};

/// One aligned DMA-visible buffer with room for the hardware recycle guard.
#[repr(C, align(4))]
pub struct RxDmaBuffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    UnsafeCell<[u8; STORAGE_SIZE]>,
    AtomicU8,
);

const RX_BUFFER_RING: u8 = 0;
const RX_BUFFER_DETACHED: u8 = 1;
const RX_BUFFER_RELEASED: u8 = 2;

impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE> {
    const fn new() -> Self {
        assert!(STORAGE_SIZE >= BUFFER_SIZE + 4);
        Self(
            UnsafeCell::new([0; STORAGE_SIZE]),
            AtomicU8::new(RX_BUFFER_RING),
        )
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

    fn detach(&self, length: usize, index: usize) -> Result<ExternalRxBuffer, RxRingError> {
        if length == 0 || length > BUFFER_SIZE {
            return Err(RxRingError::Size);
        }
        self.1
            .compare_exchange(
                RX_BUFFER_RING,
                RX_BUFFER_DETACHED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| RxRingError::Busy)?;
        let pointer = NonNull::new(self.0.get().cast::<u8>()).expect("RX DMA buffer is non-null");
        let owner = NonNull::from(self).cast::<()>();
        // SAFETY: the completed-unit owner proved that DMA released this
        // buffer. The buffer is part of stable arena storage and the state
        // transition prevents recycle until the callback marks it released.
        #[allow(
            unsafe_code,
            reason = "completed descriptor detaches stable DMA buffer ownership"
        )]
        Ok(unsafe {
            ExternalRxBuffer::new(
                pointer,
                length,
                BUFFER_SIZE,
                owner,
                index,
                release_detached_buffer::<BUFFER_SIZE, STORAGE_SIZE>,
            )
        })
    }

    fn is_released(&self) -> bool {
        self.1.load(Ordering::Acquire) == RX_BUFFER_RELEASED
    }

    fn state(&self) -> u8 {
        self.1.load(Ordering::Acquire)
    }

    #[allow(
        unsafe_code,
        reason = "released affine buffer is returned to its descriptor owner"
    )]
    unsafe fn prepare_released_for_recycle(&self) -> Result<(), RxRingError> {
        self.1
            .compare_exchange(
                RX_BUFFER_RELEASED,
                RX_BUFFER_RING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| RxRingError::Busy)?;
        // SAFETY: the successful state transition proves that neither upper
        // software nor DMA owns the buffer while the ring prepares it.
        unsafe { self.prepare_for_recycle() }
    }

    #[allow(
        unsafe_code,
        reason = "stopped DMA owner may normalize a returned detached buffer"
    )]
    unsafe fn prepare_for_stopped_ring(&self) -> Result<(), RxRingError> {
        match self.1.load(Ordering::Acquire) {
            RX_BUFFER_RING => {}
            RX_BUFFER_RELEASED => {
                self.1
                    .compare_exchange(
                        RX_BUFFER_RELEASED,
                        RX_BUFFER_RING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| RxRingError::Busy)?;
            }
            RX_BUFFER_DETACHED => return Err(RxRingError::Busy),
            _ => return Err(RxRingError::Corrupt),
        }
        // SAFETY: stopped-ring ownership excludes DMA, and DETACHED was
        // rejected before touching the allocation.
        unsafe { self.prepare_for_recycle() }
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

#[allow(
    unsafe_code,
    reason = "detached-buffer state machine serializes cross-core access"
)]
unsafe impl<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Sync
    for RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>
{
}

#[allow(
    unsafe_code,
    reason = "type-erased external lease returns to its concrete DMA buffer"
)]
unsafe fn release_detached_buffer<const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>(
    owner: NonNull<()>,
    _index: usize,
) {
    // SAFETY: `RxDmaBuffer::detach` installed this callback with a pointer to
    // the same concrete buffer allocation, which remains static for the DMA
    // epoch.
    let buffer = unsafe {
        owner
            .cast::<RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>>()
            .as_ref()
    };
    assert_eq!(
        buffer.1.compare_exchange(
            RX_BUFFER_DETACHED,
            RX_BUFFER_RELEASED,
            Ordering::Release,
            Ordering::Acquire,
        ),
        Ok(RX_BUFFER_DETACHED),
        "detached RX DMA buffer must be released exactly once"
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDmaStorageError {
    AddressWidth,
    Count,
}

const fn identity_atomic_buffer_ids<const COUNT: usize>() -> [AtomicU8; COUNT] {
    assert!(COUNT <= u8::MAX as usize + 1);
    let mut ids = [const { AtomicU8::new(0) }; COUNT];
    let mut index = 0;
    while index < COUNT {
        ids[index] = AtomicU8::new(index as u8);
        index += 1;
    }
    ids
}

/// First immutable DMA-address binding that no longer matches its arena.
///
/// This is fault evidence only. It exposes neither descriptor mutation nor a
/// way to repair a live ring after adjacent memory corruption.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBufferAddressMismatch {
    pub index: usize,
    pub expected: u32,
    pub observed: u32,
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
    // A hardware completion transaction belongs to the core-local DMA owner.
    // Only the detached packet buffer produced by that transaction may cross
    // an executor/core boundary.
    _not_send: PhantomData<*mut ()>,
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
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
> {
    unit: RxCompletedUnit,
    storage: &'storage RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>,
    ring: &'owner mut RxRingLive<'ring, COUNT>,
    requires_recycle: bool,
    // Keep physical descriptor credit on the core-local DMA owner. The
    // consuming detach transition deliberately returns a separate Send-able
    // packet-buffer type.
    _not_send: PhantomData<*mut ()>,
}

/// One complete single-buffer unit detached from its descriptor.
pub struct RxDmaDetachedUnit {
    buffer: ExternalRxBuffer,
    descriptor_address: u32,
    descriptor_word0: u32,
    length: usize,
}

impl RxDmaDetachedUnit {
    pub const fn descriptor_address(&self) -> u32 {
        self.descriptor_address
    }

    pub const fn descriptor_word0(&self) -> u32 {
        self.descriptor_word0
    }

    pub const fn length(&self) -> usize {
        self.length
    }

    pub fn into_buffer(self) -> ExternalRxBuffer {
        self.buffer
    }
}

impl<
    'storage,
    'owner,
    'ring,
    const COUNT: usize,
    const BUFFER_SIZE: usize,
    const STORAGE_SIZE: usize,
> RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>
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
        let buffer = self.storage.buffer_for_descriptor(index)?;
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
                unsafe {
                    self.storage
                        .buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_recycle()
                }
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

    /// Detach one complete hardware buffer for zero-copy upper ownership.
    ///
    /// The descriptor remains observed and CPU-owned in the ring. Dropping
    /// the returned buffer marks it released; only a later ring-owner service
    /// may rearm the descriptor.
    pub fn detach_single(mut self) -> Result<RxDmaDetachedUnit, RxRingError>
    where
        'storage: 'static,
    {
        if self.unit.descriptor_count() != 1 {
            return Err(RxRingError::Count);
        }
        let index = self.unit.head_index();
        let buffer_id = self
            .storage
            .descriptor_buffer_id(index)
            .ok_or(RxRingError::Count)?;
        let buffer = self
            .storage
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?
            .detach(self.unit.total_length(), buffer_id)?;
        self.requires_recycle = false;
        Ok(RxDmaDetachedUnit {
            buffer,
            descriptor_address: self.unit.descriptor_address(),
            descriptor_word0: self.unit.staged_word0(),
            length: self.unit.total_length(),
        })
    }
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Drop
    for RxDmaCompletedUnit<'_, '_, '_, COUNT, BUFFER_SIZE, STORAGE_SIZE>
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
    descriptor_buffer_ids: [AtomicU8; COUNT],
    lifecycle: AtomicU8,
}

#[allow(unsafe_code, reason = "atomic bindings serialize cross-core ownership")]
unsafe impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize> Sync
    for RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>
{
    pub const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; COUNT],
            buffers: [const { RxDmaBuffer::new() }; COUNT],
            descriptor_buffer_ids: identity_atomic_buffer_ids(),
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
        buffer_addresses: &mut RxDmaBufferAddresses<COUNT>,
    ) -> Result<u32, RxDmaStorageError> {
        self.dma_layout_rotated(buffer_addresses, 0)
    }

    /// Bind each physical descriptor to a cyclically shifted arena buffer.
    ///
    /// Rotation zero is the production identity layout. A non-zero rotation
    /// is a target proof that every CPU ownership path follows the explicit
    /// descriptor-to-buffer binding instead of indexing the buffer arena by
    /// descriptor number. The binding is frozen before the first ring owner
    /// exists and remains immutable for that complete DMA lifecycle.
    #[allow(
        unsafe_code,
        reason = "Reusable lifecycle is the exclusive pre-ring binding boundary"
    )]
    pub fn dma_layout_rotated(
        &self,
        buffer_addresses: &mut RxDmaBufferAddresses<COUNT>,
        rotation: usize,
    ) -> Result<u32, RxDmaStorageError> {
        self.validate_descriptor_rotation(rotation)?;
        for (descriptor_index, address) in buffer_addresses.iter_mut().enumerate() {
            let buffer_id = (descriptor_index + rotation) % COUNT;
            *address = self.buffers[buffer_id].dma_address()?;
        }
        self.bind_descriptor_rotation(rotation)?;
        u32::try_from(self.descriptors.as_ptr().addr()).map_err(|_| RxDmaStorageError::AddressWidth)
    }

    fn validate_descriptor_rotation(&self, rotation: usize) -> Result<(), RxDmaStorageError> {
        if COUNT == 0 || COUNT > usize::from(u8::MAX) + 1 || rotation >= COUNT {
            return Err(RxDmaStorageError::Count);
        }
        if self.lifecycle_state() != RxDmaArenaState::Reusable {
            return Err(RxDmaStorageError::Count);
        }
        Ok(())
    }

    #[allow(
        unsafe_code,
        reason = "Reusable lifecycle is the exclusive pre-ring binding boundary"
    )]
    fn bind_descriptor_rotation(&self, rotation: usize) -> Result<(), RxDmaStorageError> {
        self.validate_descriptor_rotation(rotation)?;
        for descriptor_index in 0..COUNT {
            let buffer_id = (descriptor_index + rotation) % COUNT;
            self.descriptor_buffer_ids[descriptor_index].store(buffer_id as u8, Ordering::Release);
        }
        Ok(())
    }

    /// Current physical buffer identity bound to one descriptor.
    pub fn descriptor_buffer_id(&self, descriptor_index: usize) -> Option<usize> {
        self.descriptor_buffer_ids
            .get(descriptor_index)
            .map(|id| usize::from(id.load(Ordering::Acquire)))
    }

    fn buffer_for_descriptor(
        &self,
        descriptor_index: usize,
    ) -> Option<&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>> {
        self.buffer_by_id(self.descriptor_buffer_id(descriptor_index)?)
    }

    fn any_buffer_in_state(&self, state: u8) -> bool {
        self.buffers.iter().any(|buffer| buffer.state() == state)
    }

    pub fn detached_buffer_count(&self) -> usize {
        self.buffers
            .iter()
            .filter(|buffer| buffer.state() == RX_BUFFER_DETACHED)
            .count()
    }

    fn buffer_by_id(&self, buffer_id: usize) -> Option<&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>> {
        self.buffers.get(buffer_id)
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
        buffer_addresses: &'addresses RxDmaBufferAddresses<COUNT>,
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
        #[cfg(target_pointer_width = "32")] buffer_addresses: &'storage RxDmaBufferAddresses<COUNT>,
        #[cfg(not(target_pointer_width = "32"))] buffer_addresses: &'storage [u32; COUNT],
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
                unsafe {
                    self.buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_stopped_ring()
                }
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
        if self.any_buffer_in_state(RX_BUFFER_DETACHED) {
            return Err((ring, RxRingError::Busy));
        }
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
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_for_stopped_ring()
            }
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
        let buffer = self
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?;
        Ok(Some(RxDmaCompletedDescriptor {
            descriptor,
            buffer,
            _ring: ring,
            _not_send: PhantomData,
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
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_for_recycle()
            }
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
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_for_recycle()
            }
        })
    }

    /// Return the longest contiguous prefix whose detached upper owners have
    /// all released their original DMA buffers.
    ///
    /// A still-retained descriptor terminates the prefix even if later frames
    /// have already returned. This preserves hardware list order while making
    /// network-token release the only authorization for descriptor reuse.
    #[allow(
        unsafe_code,
        reason = "released buffer state authorizes the ring rearm transition"
    )]
    pub fn recycle_released_prefix<const MAX_BATCH: usize, M: RxDma>(
        &self,
        ring: &mut RxRingLive<'_, COUNT>,
        mmio: &mut M,
    ) -> Result<Option<RxLiveAppend>, RxRingError> {
        self.validate_live_ring(ring)?;
        if MAX_BATCH == 0 {
            return Err(RxRingError::Count);
        }
        let start = ring.recycle_start();
        let observed = ring.observed_mask();
        let mut released = 0_usize;
        while released < MAX_BATCH.min(COUNT) {
            let index = (start + released) % COUNT;
            if observed & (1_u64 << index) == 0
                || !crate::descriptor::rx_done(self.descriptors[index].word0())
                || !self
                    .buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .is_released()
            {
                break;
            }
            released += 1;
        }
        if released == 0 {
            return Ok(None);
        }
        ring.recycle_released_terminal_prefix_owned(mmio, released, |index| {
            // SAFETY: the prefix scan proved RELEASED and the ring revalidates
            // the complete descriptor image before invoking this callback.
            unsafe {
                self.buffer_for_descriptor(index)
                    .ok_or(RxRingError::Count)?
                    .prepare_released_for_recycle()
            }
        })
    }

    /// Number of detached buffers already returned by upper owners but not
    /// yet appended back to the hardware list.
    pub fn released_buffer_count(&self) -> usize {
        self.buffers
            .iter()
            .filter(|buffer| buffer.is_released())
            .count()
    }

    pub fn completed_unit_frontier(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<RxCompletedUnitFrontier, RxRingError> {
        self.validate_live_ring(ring)?;
        Ok(ring.completed_unit_frontier_with(|index| {
            self.buffer_for_descriptor(index)
                .is_some_and(RxDmaBuffer::leading_guard_overwritten)
        }))
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
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
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
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
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
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
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
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
        ))
    }

    /// Copy bytes from the first descriptor of the first complete unit without
    /// transferring descriptor ownership.
    ///
    /// This is a deliberately narrow admission-preview boundary. The ordered
    /// LAST/NEXT cursor proves that the first unit is complete, while copying
    /// values into caller-owned storage prevents a DMA buffer reference from
    /// escaping or surviving recycle. The operation neither marks the unit as
    /// observed nor changes the live ring generation.
    #[allow(
        unsafe_code,
        reason = "the frozen completed-unit frontier proves temporary CPU read ownership"
    )]
    pub fn copy_first_completed_unit_bytes_through_cursor(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        last_descriptor_low: u32,
        next_descriptor_low: u32,
        offset: usize,
        output: &mut [u8],
    ) -> Result<bool, RxRingError> {
        self.validate_live_ring(ring)?;
        let frontier = ring.first_completed_unit_frontier_through_cursor_with(
            last_descriptor_low,
            next_descriptor_low,
            |index| {
                self.buffer_for_descriptor(index)
                    .is_some_and(RxDmaBuffer::leading_guard_overwritten)
            },
        );
        if frontier.unit_count == 0 {
            return Ok(false);
        }
        let index = self
            .completion_frontier_head(ring)?
            .ok_or(RxRingError::Corrupt)?;
        let available = crate::descriptor::length(ring.descriptors()[index].word0()) as usize;
        let Some(end) = offset
            .checked_add(output.len())
            .filter(|end| *end <= available)
        else {
            return Ok(false);
        };
        // SAFETY: the frozen cursor and terminal unit frontier establish that
        // DMA has released this descriptor. Only values are copied, and the
        // immutable view ends before this function returns.
        let buffer = self
            .buffer_for_descriptor(index)
            .ok_or(RxRingError::Count)?;
        output.copy_from_slice(unsafe { &buffer.completed()[offset..end] });
        Ok(true)
    }

    /// Physical descriptor index immediately after the contiguous observed
    /// prefix retained by upper owners.
    ///
    /// This is only an index calculation; callers still need a hardware
    /// frontier proof before reading descriptor metadata or payload bytes.
    pub fn completion_frontier_head(
        &self,
        ring: &RxRingLive<'_, COUNT>,
    ) -> Result<Option<usize>, RxRingError> {
        self.validate_live_ring(ring)?;
        let observed = ring.observed_mask();
        for distance in 0..COUNT {
            let index = (ring.recycle_start() + distance) % COUNT;
            if observed & (1_u64 << index) == 0 {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    pub fn take_completed_unit<'storage, 'owner, 'ring>(
        &'storage self,
        ring: &'owner mut RxRingLive<'ring, COUNT>,
        descriptor_limit: usize,
    ) -> Result<
        Option<RxDmaCompletedUnit<'storage, 'owner, 'ring, COUNT, BUFFER_SIZE, STORAGE_SIZE>>,
        RxRingError,
    > {
        self.validate_live_ring(ring)?;
        let unit = match ring.take_completed_unit_owned(descriptor_limit, |index| {
            self.buffer_for_descriptor(index)
                .is_some_and(RxDmaBuffer::leading_guard_overwritten)
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
            _not_send: PhantomData,
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
                unsafe {
                    self.buffer_for_descriptor(index)
                        .ok_or(RxRingError::Count)?
                        .prepare_for_recycle()
                }
            },
        )
    }

    fn validate_live_ring(&self, ring: &RxRingLive<'_, COUNT>) -> Result<(), RxRingError> {
        if ring.belongs_to_lifecycle(&self.lifecycle) {
            return Ok(());
        }
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
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Result<(), RxRingError> {
        if !core::ptr::eq(descriptors, &self.descriptors) {
            return Err(RxRingError::DescriptorOwnerAddress);
        }
        self.validate_descriptor_base(descriptor_base)?;
        self.validate_buffer_addresses(buffer_addresses)
    }

    fn validate_descriptor_base(&self, descriptor_base: u32) -> Result<(), RxRingError> {
        #[cfg(target_pointer_width = "32")]
        if u32::try_from(self.descriptors.as_ptr().addr()).map_err(|_| RxRingError::Address)?
            != descriptor_base
        {
            return Err(RxRingError::DescriptorBaseAddress);
        }
        #[cfg(not(target_pointer_width = "32"))]
        let _ = descriptor_base;
        Ok(())
    }

    fn validate_buffer_addresses(
        &self,
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Result<(), RxRingError> {
        // ESP32-S31 DMA is a 32-bit target. Native builds use synthetic low
        // addresses and a mock that never touches host memory; descriptor
        // identity remains checked by `validate_ring_layout` there.
        #[cfg(target_pointer_width = "32")]
        for (index, &address) in buffer_addresses.iter().enumerate() {
            let buffer = self
                .buffer_for_descriptor(index)
                .ok_or(RxRingError::Count)?;
            if buffer.dma_address().map_err(|_| RxRingError::Address)? != address {
                return Err(if index + 1 == COUNT {
                    RxRingError::TailBufferAddress
                } else {
                    RxRingError::BufferAddress
                });
            }
        }
        #[cfg(not(target_pointer_width = "32"))]
        let _ = buffer_addresses;
        Ok(())
    }

    pub fn first_buffer_address_mismatch(
        &self,
        buffer_addresses: &RxDmaBufferAddresses<COUNT>,
    ) -> Option<RxBufferAddressMismatch> {
        for (index, &observed) in buffer_addresses.iter().enumerate() {
            let buffer = self.buffer_for_descriptor(index)?;
            let expected = buffer.dma_address().ok()?;
            if expected != observed {
                return Some(RxBufferAddressMismatch {
                    index,
                    expected,
                    observed,
                });
            }
        }
        None
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
    extern crate std;

    use self::std::boxed::Box;
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
    fn completed_ownership_follows_a_permuted_descriptor_buffer_binding() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        // Descriptor zero names physical buffer one; descriptor one names
        // physical buffer zero. Synthetic host addresses preserve that order.
        let buffers = [0x2f00_2200, 0x2f00_2000];
        let mut storage = Box::new(RxDmaStorage::<COUNT, 16, 20>::new());
        storage.buffer_mut(1).unwrap()[4..8].copy_from_slice(&[5, 6, 7, 8]);
        let storage = Box::leak(storage);
        storage
            .bind_descriptor_rotation(1)
            .expect("pre-ring rotation");
        assert_eq!(storage.descriptor_buffer_id(0), Some(1));
        assert_eq!(storage.descriptor_buffer_id(1), Some(0));

        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared rotated owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live rotated epoch");
        storage.descriptors()[0].write_word0(
            16 | (8 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = 0;

        let detached = storage
            .take_completed_unit(&mut live, 1)
            .unwrap()
            .expect("completed rotated unit")
            .detach_single()
            .expect("rotated buffer detaches");
        let pool = open_esp_radio_dma::ExternalRxHandoffPool::<16, 1>::new();
        let radio = pool
            .try_claim_radio(detached.into_buffer(), 0)
            .map_err(drop)
            .expect("rotated handoff slot");
        let length = radio.frame().len();
        let mut network = pool.claim_network(radio.republish(0, length));
        assert_eq!(
            network.with_frame(|frame| frame[4..8].to_vec()),
            [5, 6, 7, 8],
            "descriptor zero must expose its bound physical buffer one"
        );
        drop(network);
        assert!(
            storage
                .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn detached_buffer_cannot_rearm_until_the_network_lease_returns() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
        storage.buffer_mut(0).unwrap()[4..8].copy_from_slice(&[1, 2, 3, 4]);
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        storage.descriptors()[0].write_word0(
            16 | (8 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
        // Model the finite list exhausted at its accepted tail. Merely seeing
        // NEXT name descriptor one is not sufficient: HIL proved that image
        // may precede hardware fetching descriptor zero's link word.
        mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = 0;

        let detached = storage
            .take_completed_unit(&mut live, 1)
            .unwrap()
            .expect("completed unit")
            .detach_single()
            .expect("single descriptor detaches");
        let pool = open_esp_radio_dma::ExternalRxHandoffPool::<16, 1>::new();
        let radio = pool
            .try_claim_radio(detached.into_buffer(), 0)
            .map_err(drop)
            .expect("external handoff slot");
        let length = radio.frame().len();
        let mut network = pool.claim_network(radio.republish(0, length));
        assert_eq!(
            network.with_frame(|frame| frame[4..8].to_vec()),
            [1, 2, 3, 4]
        );
        assert_eq!(
            storage.recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio),
            Ok(None),
            "live network token must keep the descriptor CPU-owned"
        );
        assert_ne!(
            storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
            0
        );

        drop(network);
        let append = storage
            .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
            .expect("released prefix reclaim")
            .expect("one descriptor appended");
        assert_eq!(append.descriptor_count, 1);
        assert_eq!(storage.released_buffer_count(), 0);
        assert_eq!(
            storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
            0
        );
    }

    #[test]
    fn released_dma_buffers_rearm_only_as_a_contiguous_ring_prefix() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        for descriptor in &storage.descriptors()[..2] {
            descriptor.write_word0(
                16 | (8 << crate::descriptor::LENGTH_SHIFT)
                    | crate::descriptor::BIT_30
                    | crate::descriptor::BIT_31,
            );
        }
        mmio.last_descriptor_low =
            (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = 0;

        let first = storage
            .take_completed_unit(&mut live, 1)
            .unwrap()
            .expect("first completed unit")
            .detach_single()
            .expect("first single descriptor");
        let second = storage
            .take_completed_unit(&mut live, 1)
            .unwrap()
            .expect("second completed unit")
            .detach_single()
            .expect("second single descriptor");
        let pool = open_esp_radio_dma::ExternalRxHandoffPool::<16, 2>::new();
        let first = pool
            .try_claim_radio(first.into_buffer(), 0)
            .map_err(drop)
            .expect("first handoff slot");
        let first_length = first.frame().len();
        let first = pool.claim_network(first.republish(0, first_length));
        let second = pool
            .try_claim_radio(second.into_buffer(), 1)
            .map_err(drop)
            .expect("second handoff slot");
        let second_length = second.frame().len();
        let second = pool.claim_network(second.republish(0, second_length));

        drop(second);
        assert_eq!(storage.released_buffer_count(), 1);
        assert_eq!(
            storage.recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio),
            Ok(None),
            "a returned successor must not skip a retained ring head"
        );

        drop(first);
        let append = storage
            .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
            .expect("released prefix reclaim")
            .expect("both returned descriptors append together");
        assert_eq!(append.head_index, 0);
        assert_eq!(append.descriptor_count, 2);
        assert_eq!(storage.released_buffer_count(), 0);
        assert_eq!(live.recycle_start(), 2);
        assert!(
            storage.descriptors()[..2]
                .iter()
                .all(|descriptor| descriptor.word0() & crate::descriptor::BIT_30 == 0)
        );
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
    fn live_binding_rejects_a_foreign_storage_arena() {
        const COUNT: usize = 2;
        const BASE: u32 = 0x2f00_1000;
        let buffers = [0x2f00_2000, 0x2f00_2200];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let foreign = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let live = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner")
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live owner");

        assert_eq!(
            foreign.completed_unit_frontier(&live),
            Err(RxRingError::DescriptorOwnerAddress)
        );

        let _halted = live
            .try_stop(&mut mmio)
            .unwrap_or_else(|_| panic!("walker stops"));
    }

    #[test]
    fn first_frontier_returns_one_unit_from_a_terminal_burst() {
        const COUNT: usize = 4;
        const BASE: u32 = 0x2f00_1000;
        const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
        let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
        let storage = RxDmaStorage::<COUNT, 16, 20>::new();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
            .expect("prepared owner");
        let live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");

        for descriptor in storage.descriptors() {
            descriptor.write_word0(
                16 | (4 << crate::descriptor::LENGTH_SHIFT)
                    | crate::descriptor::BIT_30
                    | crate::descriptor::BIT_31,
            );
        }
        mmio.last_descriptor_low =
            (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
        mmio.next_descriptor_low = BASE & ADDRESS_LOW_MASK;

        let complete = storage
            .completed_unit_frontier_through_cursor(
                &live,
                mmio.last_descriptor_low,
                mmio.next_descriptor_low,
            )
            .expect("complete burst frontier");
        assert_eq!(complete.unit_count, COUNT);
        assert_eq!(complete.descriptor_count, COUNT);

        let first = storage
            .first_completed_unit_frontier_through_cursor(
                &live,
                mmio.last_descriptor_low,
                mmio.next_descriptor_low,
            )
            .expect("first unit frontier");
        assert_eq!(first.unit_count, 1);
        assert_eq!(first.descriptor_count, 1);
        assert!(live.try_stop(&mut mmio).is_ok());
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
