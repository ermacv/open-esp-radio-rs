//! Pinned receive pool and linear ownership tokens.

use core::{
    cell::UnsafeCell,
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

use crate::{
    DmaAddressError, DmaFrameAddress, FRAME_BUFFER_SIZE, RxFrameError, RxFrameView,
    ordering::device_fence,
};

const STATE_FREE: u8 = 0;
const STATE_ARMED: u8 = 1;
const STATE_DELIVERED: u8 = 2;

#[repr(C, align(4))]
struct RxFrameBuffer(UnsafeCell<[u8; FRAME_BUFFER_SIZE]>);

const _: () = {
    assert!(core::mem::size_of::<RxFrameBuffer>() == FRAME_BUFFER_SIZE);
    assert!(core::mem::align_of::<RxFrameBuffer>() == 4);
};

impl RxFrameBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; FRAME_BUFFER_SIZE]))
    }

    #[allow(
        unsafe_code,
        reason = "a successful Free-to-Armed claim uniquely owns the buffer before MMIO publication"
    )]
    fn prepare_for_dma(&self) {
        // SAFETY: the caller changed this slot from Free to Armed and has not
        // yet returned its address token, so neither CPU nor DMA can access it.
        unsafe { (*self.0.get()).fill(0) };
    }

    #[cfg(not(target_arch = "riscv32"))]
    #[allow(
        unsafe_code,
        reason = "the native armed token is the sole model writer"
    )]
    fn write_model(&self, image: &[u8]) {
        // SAFETY: this method is reachable only through `&mut RxArmed` or
        // `&mut RxStubArmed`; native builds have no external DMA actor.
        let bytes = unsafe { &mut *self.0.get() };
        bytes.fill(0);
        let copy_length = image.len().min(FRAME_BUFFER_SIZE);
        bytes[..copy_length].copy_from_slice(&image[..copy_length]);
    }

    #[allow(
        unsafe_code,
        reason = "a Delivered token proves exclusive CPU ownership after the acquire fence"
    )]
    fn delivered(&self) -> &[u8; FRAME_BUFFER_SIZE] {
        // SAFETY: only a non-forgeable Delivered token calls this method, and
        // that token prevents release/rearm while the returned view is alive.
        unsafe { &*self.0.get() }
    }
}

// SAFETY: `states`/`stub_state` gate every UnsafeCell transition. Buffer reads
// require a unique Delivered token; writes happen only before publishing a
// unique Armed token or through the single native model token.
#[allow(
    unsafe_code,
    reason = "atomic ownership states are the RX buffer Sync boundary"
)]
unsafe impl Sync for RxFrameBuffer {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RxSlotState {
    #[default]
    Free,
    Armed,
    Delivered,
}

impl RxSlotState {
    fn decode(value: u8) -> Self {
        match value {
            STATE_FREE => Self::Free,
            STATE_ARMED => Self::Armed,
            STATE_DELIVERED => Self::Delivered,
            _ => unreachable!("RX ownership state is mutated only by this module"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPoolError {
    EmptyPool,
    Address(DmaAddressError),
    AddressWidth,
    HardwareBusy,
    Poisoned,
    Exhausted,
    State {
        expected: RxSlotState,
        observed: RxSlotState,
    },
}

impl From<DmaAddressError> for RxPoolError {
    fn from(error: DmaAddressError) -> Self {
        Self::Address(error)
    }
}

/// Failed pool-address binding that retains the exact unpinned allocation.
///
/// Address and span validation finish before the storage is pinned. On
/// failure no ownership state is changed and no address can have escaped, so
/// [`Self::into_parts`] may return the allocation for a corrected retry.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::RxPoolBindFailure;
///
/// fn duplicate(failure: RxPoolBindFailure<1>) {
///     let moved = failure;
///     let _ = failure.error();
///     drop(moved);
/// }
/// ```
pub struct RxPoolBindFailure<const COUNT: usize> {
    storage: &'static mut RxPoolStorage<COUNT>,
    error: RxPoolError,
}

impl<const COUNT: usize> RxPoolBindFailure<COUNT> {
    fn new(storage: &'static mut RxPoolStorage<COUNT>, error: RxPoolError) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> RxPoolError {
        self.error
    }

    /// Recover the unchanged allocation together with the binding error.
    pub fn into_parts(self) -> (&'static mut RxPoolStorage<COUNT>, RxPoolError) {
        (self.storage, self.error)
    }
}

impl<const COUNT: usize> core::fmt::Debug for RxPoolBindFailure<COUNT> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RxPoolBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Static RX allocation containing `COUNT` delivery buffers and one physically
/// separate stub/drop buffer.
///
/// DMA-visible buffers are the leading, contiguous fields. CPU-only ownership
/// state follows them and is never included in an address token.
#[repr(C, align(4))]
pub struct RxPoolStorage<const COUNT: usize> {
    buffers: [RxFrameBuffer; COUNT],
    stub: RxFrameBuffer,
    states: [AtomicU8; COUNT],
    stub_state: AtomicU8,
    active: AtomicBool,
    poisoned: AtomicBool,
    _pin: PhantomPinned,
}

impl<const COUNT: usize> RxPoolStorage<COUNT> {
    pub const fn new() -> Self {
        Self {
            buffers: [const { RxFrameBuffer::new() }; COUNT],
            stub: RxFrameBuffer::new(),
            states: [const { AtomicU8::new(STATE_FREE) }; COUNT],
            stub_state: AtomicU8::new(STATE_FREE),
            active: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(
        storage: &'static mut Self,
    ) -> Result<PinnedRxPool<COUNT>, RxPoolBindFailure<COUNT>> {
        if COUNT == 0 {
            return Err(RxPoolBindFailure::new(storage, RxPoolError::EmptyPool));
        }
        let address = match u32::try_from(core::ptr::addr_of!(storage.buffers).addr()) {
            Ok(address) => address,
            Err(_) => {
                return Err(RxPoolBindFailure::new(storage, RxPoolError::AddressWidth));
            }
        };
        let address = match DmaFrameAddress::try_new(address) {
            Ok(address) => address,
            Err(error) => return Err(RxPoolBindFailure::new(storage, error.into())),
        };
        Self::pin_static_inner(storage, address)
    }

    /// Bind a deterministic base address to a native pool model.
    ///
    /// `base` covers delivery slot zero. The constructor verifies all delivery
    /// buffers plus the final separate stub, not merely the first address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        base: DmaFrameAddress,
    ) -> Result<PinnedRxPool<COUNT>, RxPoolBindFailure<COUNT>> {
        Self::pin_static_inner(storage, base)
    }

    fn pin_static_inner(
        storage: &'static mut Self,
        base: DmaFrameAddress,
    ) -> Result<PinnedRxPool<COUNT>, RxPoolBindFailure<COUNT>> {
        if COUNT == 0 {
            return Err(RxPoolBindFailure::new(storage, RxPoolError::EmptyPool));
        }
        let frame_count = match COUNT.checked_add(1) {
            Some(frame_count) => frame_count,
            None => {
                return Err(RxPoolBindFailure::new(
                    storage,
                    RxPoolError::Address(DmaAddressError::RegionTooLarge),
                ));
            }
        };
        if let Err(error) = base.validates_frame_count(frame_count) {
            return Err(RxPoolBindFailure::new(storage, error.into()));
        }
        let storage = Pin::static_mut(storage).into_ref();
        Ok(PinnedRxPool { storage, base })
    }
}

impl<const COUNT: usize> Default for RxPoolStorage<COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

/// Movable owner of one permanently located receive pool.
pub struct PinnedRxPool<const COUNT: usize> {
    storage: Pin<&'static RxPoolStorage<COUNT>>,
    base: DmaFrameAddress,
}

impl<const COUNT: usize> PinnedRxPool<COUNT> {
    pub const fn capacity(&self) -> usize {
        COUNT
    }

    pub fn slot_state(&self, index: usize) -> Option<RxSlotState> {
        self.storage
            .states
            .get(index)
            .map(|state| RxSlotState::decode(state.load(Ordering::Acquire)))
    }

    pub fn stub_state(&self) -> RxSlotState {
        RxSlotState::decode(self.storage.stub_state.load(Ordering::Acquire))
    }

    /// Arm the first free delivery buffer, or the separate stub when all
    /// delivery buffers are retained by hardware or upper-layer consumers.
    ///
    /// ESP32-S31 has one direct `RXDMA_ADDR`, so exactly one ordinary or stub
    /// buffer may be Armed across the complete pool. The active gate remains
    /// closed if an Armed token is lost. Any lifecycle-state mismatch
    /// permanently poisons this owner, including across an already-issued
    /// concurrent arm's later completion.
    pub fn arm_next(&self) -> Result<RxArm<'_, COUNT>, RxPoolError> {
        if self.storage.poisoned.load(Ordering::Acquire) {
            return Err(RxPoolError::Poisoned);
        }
        if self
            .storage
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RxPoolError::HardwareBusy);
        }
        if self.storage.poisoned.load(Ordering::Acquire) {
            self.storage.active.store(false, Ordering::Release);
            return Err(RxPoolError::Poisoned);
        }

        for (index, state) in self.storage.states.iter().enumerate() {
            if state
                .compare_exchange(STATE_FREE, STATE_ARMED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.storage.buffers[index].prepare_for_dma();
                device_fence();
                return Ok(RxArm::Buffer(RxArmed { pool: self, index }));
            }
        }

        if self
            .storage
            .stub_state
            .compare_exchange(STATE_FREE, STATE_ARMED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Nothing was published and no Armed token escaped. Return the
            // pool-wide gate so a later release can make progress.
            self.storage.active.store(false, Ordering::Release);
            return Err(RxPoolError::Exhausted);
        }
        self.storage.stub.prepare_for_dma();
        device_fence();
        Ok(RxArm::Stub(RxStubArmed { pool: self }))
    }

    fn address_for(&self, frame_index: usize) -> DmaFrameAddress {
        self.base
            .checked_frame_offset(frame_index)
            .expect("the complete pool span was validated when pinned")
    }

    fn transition(
        &self,
        index: usize,
        from: RxSlotState,
        to: RxSlotState,
    ) -> Result<(), RxPoolError> {
        let state = &self.storage.states[index];
        let observed = state.compare_exchange(
            encode_state(from),
            encode_state(to),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        match observed {
            Ok(_) => Ok(()),
            Err(value) => Err(RxPoolError::State {
                expected: from,
                observed: RxSlotState::decode(value),
            }),
        }
    }

    fn transition_stub(&self, from: RxSlotState, to: RxSlotState) -> Result<(), RxPoolError> {
        let observed = self.storage.stub_state.compare_exchange(
            encode_state(from),
            encode_state(to),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        match observed {
            Ok(_) => Ok(()),
            Err(value) => Err(RxPoolError::State {
                expected: from,
                observed: RxSlotState::decode(value),
            }),
        }
    }

    fn release_active_after_delivery(&self) {
        // The slot/stub state was published as Delivered first. A later arm's
        // Acquire gate claim therefore observes that ownership transition.
        self.storage.active.store(false, Ordering::Release);
    }

    fn poison_after_lifecycle_failure(&self) {
        // Unlike the transient active bit, this is never cleared by an
        // already-issued arm completing. A caller-visible reset/rebind path is
        // deliberately absent: the inconsistent epoch remains terminal.
        self.storage.poisoned.store(true, Ordering::Release);
    }
}

const fn encode_state(state: RxSlotState) -> u8 {
    match state {
        RxSlotState::Free => STATE_FREE,
        RxSlotState::Armed => STATE_ARMED,
        RxSlotState::Delivered => STATE_DELIVERED,
    }
}

/// Result of selecting a delivery slot or the separate drop stub.
pub enum RxArm<'pool, const COUNT: usize> {
    Buffer(RxArmed<'pool, COUNT>),
    Stub(RxStubArmed<'pool, COUNT>),
}

/// Terminal owner for a failed RX lifecycle transition.
///
/// The exact input owner is retained but intentionally cannot be extracted:
/// after completion evidence has been consumed, or after an ownership-state
/// mismatch, retrying an ordinary Armed/Delivered token would be unsound. The
/// associated pool remains closed to further DMA publication.
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::{RxArm, RxLifecycleFailure};
///
/// fn retry<'pool>(failure: RxLifecycleFailure<RxArm<'pool, 1>>) {
///     let _ordinary_arm = failure.into_owner();
/// }
/// ```
pub struct RxLifecycleFailure<Owner> {
    _owner: Owner,
    error: RxPoolError,
}

impl<Owner> RxLifecycleFailure<Owner> {
    fn new(owner: Owner, error: RxPoolError) -> Self {
        Self {
            _owner: owner,
            error,
        }
    }

    pub const fn error(&self) -> RxPoolError {
        self.error
    }
}

impl<Owner> core::fmt::Debug for RxLifecycleFailure<Owner> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RxLifecycleFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Read-only classification of an opaque completed RX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxCompletionKind {
    Frame { index: usize },
    Stub,
}

enum RxCompletionOwner<'pool, const COUNT: usize> {
    Frame(RxDelivered<'pool, COUNT>),
    Stub(RxStubDelivered<'pool, COUNT>),
}

/// Opaque CPU-owned completion produced from an aggregate [`RxArm`].
///
/// Keeping the variant private prevents the successfully reclaimed owner from
/// being split away from the fail-closed [`Self::recycle`] transition.
pub struct RxCompletion<'pool, const COUNT: usize> {
    owner: RxCompletionOwner<'pool, COUNT>,
}

impl<'pool, const COUNT: usize> RxCompletion<'pool, COUNT> {
    pub const fn kind(&self) -> RxCompletionKind {
        match &self.owner {
            RxCompletionOwner::Frame(frame) => RxCompletionKind::Frame {
                index: frame.index(),
            },
            RxCompletionOwner::Stub(_) => RxCompletionKind::Stub,
        }
    }

    /// Borrow a delivered frame, or return `None` for a completed drop stub.
    pub fn frame(&self) -> Option<Result<RxFrameView<'_>, RxFrameError>> {
        match &self.owner {
            RxCompletionOwner::Frame(frame) => Some(frame.frame()),
            RxCompletionOwner::Stub(_) => None,
        }
    }

    /// Recycle this completion or retain it in an opaque terminal failure.
    ///
    /// A state mismatch permanently poisons the pool against later address
    /// publication. The failure has no method that can recover a normal
    /// completion for retry.
    pub fn recycle(self) -> Result<(), RxLifecycleFailure<Self>> {
        match self.owner {
            RxCompletionOwner::Frame(frame) => match frame.release_inner_retaining_owner() {
                Ok(()) => Ok(()),
                Err((frame, error)) => Err(RxLifecycleFailure::new(
                    Self {
                        owner: RxCompletionOwner::Frame(frame),
                    },
                    error,
                )),
            },
            RxCompletionOwner::Stub(stub) => match stub.discard_inner_retaining_owner() {
                Ok(()) => Ok(()),
                Err((stub, error)) => Err(RxLifecycleFailure::new(
                    Self {
                        owner: RxCompletionOwner::Stub(stub),
                    },
                    error,
                )),
            },
        }
    }
}

impl<'pool, const COUNT: usize> RxArm<'pool, COUNT> {
    /// Complete either kind of armed resource in the native ownership model.
    ///
    /// The model only advances software ownership; it does not observe a MAC
    /// event or perform MMIO. On mismatch the exact arm is retained inside an
    /// opaque terminal failure and later address publication stays poisoned.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn complete_model(self) -> Result<RxCompletion<'pool, COUNT>, RxLifecycleFailure<Self>> {
        self.complete_inner()
    }

    /// Transfer either kind of hardware-completed arm to CPU ownership.
    ///
    /// # Safety
    ///
    /// The caller must have observed a terminal MAC RX event for the exact
    /// address borrowed from this arm and proved that hardware cannot write it.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "hardware completion is the external DMA ownership boundary"
    )]
    pub unsafe fn assume_delivered(
        self,
    ) -> Result<RxCompletion<'pool, COUNT>, RxLifecycleFailure<Self>> {
        self.complete_inner()
    }

    fn complete_inner(self) -> Result<RxCompletion<'pool, COUNT>, RxLifecycleFailure<Self>> {
        match self {
            Self::Buffer(armed) => match armed.complete_inner_retaining_owner() {
                Ok(frame) => Ok(RxCompletion {
                    owner: RxCompletionOwner::Frame(frame),
                }),
                Err((armed, error)) => Err(RxLifecycleFailure::new(Self::Buffer(armed), error)),
            },
            Self::Stub(armed) => match armed.complete_inner_retaining_owner() {
                Ok(stub) => Ok(RxCompletion {
                    owner: RxCompletionOwner::Stub(stub),
                }),
                Err((armed, error)) => Err(RxLifecycleFailure::new(Self::Stub(armed), error)),
            },
        }
    }
}

/// Borrowed address authority for one currently armed RX image.
#[derive(Clone, Copy)]
pub struct RxDmaAddress<'armed> {
    address: DmaFrameAddress,
    _armed: PhantomData<&'armed ()>,
}

impl RxDmaAddress<'_> {
    pub const fn as_u32(self) -> u32 {
        self.address.as_u32()
    }
}

/// Hardware-owned ordinary RX slot.
pub struct RxArmed<'pool, const COUNT: usize> {
    pool: &'pool PinnedRxPool<COUNT>,
    index: usize,
}

impl<'pool, const COUNT: usize> RxArmed<'pool, COUNT> {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub fn dma_address(&self) -> RxDmaAddress<'_> {
        RxDmaAddress {
            address: self.pool.address_for(self.index),
            _armed: PhantomData,
        }
    }

    /// Model bytes produced by DMA on a native host.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn write_model(&mut self, image: &[u8]) {
        self.pool.storage.buffers[self.index].write_model(image);
    }

    /// Complete a native-model transition with no external DMA actor.
    ///
    /// This compatibility API returns only the error after poisoning the
    /// complete pool. Use aggregate [`RxArm::complete_model`] when a MAC actor
    /// must retain the exact failed arm as an opaque terminal owner.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn complete_model(self) -> Result<RxDelivered<'pool, COUNT>, RxPoolError> {
        self.complete_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    /// Transfer a hardware-completed buffer to CPU ownership.
    ///
    /// # Safety
    ///
    /// The caller must have observed a terminal MAC RX event for this exact
    /// armed address and established that hardware will no longer write it.
    /// On a lifecycle mismatch this compatibility API poisons the pool but
    /// does not return the arm; aggregate [`RxArm::assume_delivered`] retains
    /// it opaquely.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "hardware completion is the external DMA ownership boundary"
    )]
    pub unsafe fn assume_delivered(self) -> Result<RxDelivered<'pool, COUNT>, RxPoolError> {
        self.complete_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    fn complete_inner_retaining_owner(
        self,
    ) -> Result<RxDelivered<'pool, COUNT>, (Self, RxPoolError)> {
        device_fence();
        if let Err(error) =
            self.pool
                .transition(self.index, RxSlotState::Armed, RxSlotState::Delivered)
        {
            self.pool.poison_after_lifecycle_failure();
            return Err((self, error));
        }
        self.pool.release_active_after_delivery();
        Ok(RxDelivered {
            pool: self.pool,
            index: self.index,
        })
    }
}

/// CPU-owned received frame. Dropping it does not implicitly recycle the
/// allocation; an explicit release is required.
pub struct RxDelivered<'pool, const COUNT: usize> {
    pool: &'pool PinnedRxPool<COUNT>,
    index: usize,
}

impl<const COUNT: usize> RxDelivered<'_, COUNT> {
    pub const fn index(&self) -> usize {
        self.index
    }

    pub fn frame(&self) -> Result<RxFrameView<'_>, RxFrameError> {
        RxFrameView::parse(self.pool.storage.buffers[self.index].delivered())
    }

    /// Compatibility recycle that poisons the pool but drops this completion
    /// on mismatch. Aggregate [`RxCompletion::recycle`] retains it opaquely.
    pub fn release(self) -> Result<(), RxPoolError> {
        self.release_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    fn release_inner_retaining_owner(self) -> Result<(), (Self, RxPoolError)> {
        match self
            .pool
            .transition(self.index, RxSlotState::Delivered, RxSlotState::Free)
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.pool.poison_after_lifecycle_failure();
                Err((self, error))
            }
        }
    }
}

/// Hardware-owned drop stub used only while every delivery slot is retained.
pub struct RxStubArmed<'pool, const COUNT: usize> {
    pool: &'pool PinnedRxPool<COUNT>,
}

impl<'pool, const COUNT: usize> RxStubArmed<'pool, COUNT> {
    pub fn dma_address(&self) -> RxDmaAddress<'_> {
        RxDmaAddress {
            address: self.pool.address_for(COUNT),
            _armed: PhantomData,
        }
    }

    #[cfg(not(target_arch = "riscv32"))]
    pub fn write_model(&mut self, image: &[u8]) {
        self.pool.storage.stub.write_model(image);
    }

    #[cfg(not(target_arch = "riscv32"))]
    /// Compatibility transition that poisons the pool and drops the ordinary
    /// arm on mismatch. Aggregate [`RxArm::complete_model`] retains it opaquely.
    pub fn complete_model(self) -> Result<RxStubDelivered<'pool, COUNT>, RxPoolError> {
        self.complete_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    /// Mark the drop stub complete after observing its terminal MAC event.
    ///
    /// # Safety
    ///
    /// Hardware must no longer write the stub address.
    /// A mismatch poisons the pool; aggregate [`RxArm::assume_delivered`]
    /// additionally retains the failed arm as a terminal owner.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "hardware completion is the external DMA ownership boundary"
    )]
    pub unsafe fn assume_delivered(self) -> Result<RxStubDelivered<'pool, COUNT>, RxPoolError> {
        self.complete_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    fn complete_inner_retaining_owner(
        self,
    ) -> Result<RxStubDelivered<'pool, COUNT>, (Self, RxPoolError)> {
        device_fence();
        if let Err(error) = self
            .pool
            .transition_stub(RxSlotState::Armed, RxSlotState::Delivered)
        {
            self.pool.poison_after_lifecycle_failure();
            return Err((self, error));
        }
        self.pool.release_active_after_delivery();
        Ok(RxStubDelivered { pool: self.pool })
    }
}

/// Completed stub token. Stub bytes are intentionally never exposed as a
/// delivered frame.
pub struct RxStubDelivered<'pool, const COUNT: usize> {
    pool: &'pool PinnedRxPool<COUNT>,
}

impl<const COUNT: usize> RxStubDelivered<'_, COUNT> {
    /// Compatibility recycle that poisons the pool but drops this completion
    /// on mismatch. Aggregate [`RxCompletion::recycle`] retains it opaquely.
    pub fn discard(self) -> Result<(), RxPoolError> {
        self.discard_inner_retaining_owner()
            .map_err(|(_owner, error)| error)
    }

    fn discard_inner_retaining_owner(self) -> Result<(), (Self, RxPoolError)> {
        match self
            .pool
            .transition_stub(RxSlotState::Delivered, RxSlotState::Free)
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.pool.poison_after_lifecycle_failure();
                Err((self, error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DMA_HIGH, DMA_LOW, MAX_PHR_LENGTH, MIN_PHR_LENGTH};

    fn pool<const COUNT: usize>(base: u32) -> Result<PinnedRxPool<COUNT>, RxPoolError> {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::new()));
        RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(base).unwrap())
            .map_err(|failure| failure.error())
    }

    #[test]
    fn buffers_have_exact_geometry_and_contiguous_stub() {
        assert_eq!(core::mem::size_of::<RxFrameBuffer>(), FRAME_BUFFER_SIZE);
        assert_eq!(core::mem::align_of::<RxFrameBuffer>(), 4);
        let storage = RxPoolStorage::<2>::new();
        let first = (&storage.buffers[0] as *const RxFrameBuffer).addr();
        let second = (&storage.buffers[1] as *const RxFrameBuffer).addr();
        let stub = (&storage.stub as *const RxFrameBuffer).addr();
        assert_eq!(second - first, FRAME_BUFFER_SIZE);
        assert_eq!(stub - second, FRAME_BUFFER_SIZE);
    }

    #[test]
    fn pool_span_includes_separate_stub() {
        let last_two_frames = DMA_HIGH - 2 * FRAME_BUFFER_SIZE as u32;
        assert!(pool::<1>(last_two_frames).is_ok());
        assert_eq!(
            pool::<2>(last_two_frames).err(),
            Some(RxPoolError::Address(DmaAddressError::OutOfRange))
        );
        assert_eq!(pool::<0>(DMA_LOW).err(), Some(RxPoolError::EmptyPool));
    }

    #[test]
    fn failed_bind_returns_exact_unchanged_storage_for_corrected_retry() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::<2>::new()));
        let identity = core::ptr::from_mut(storage);
        let last_two_frames = DMA_HIGH - 2 * FRAME_BUFFER_SIZE as u32;
        let failure = match RxPoolStorage::pin_static_model(
            storage,
            DmaFrameAddress::try_new(last_two_frames).unwrap(),
        ) {
            Ok(_) => panic!("three-frame pool span must not bind into two frames"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            RxPoolError::Address(DmaAddressError::OutOfRange)
        );

        let (storage, error) = failure.into_parts();
        assert_eq!(core::ptr::from_mut(storage), identity);
        assert_eq!(error, RxPoolError::Address(DmaAddressError::OutOfRange));
        assert_eq!(storage.states[0].load(Ordering::Acquire), STATE_FREE);
        assert_eq!(storage.states[1].load(Ordering::Acquire), STATE_FREE);
        assert!(!storage.active.load(Ordering::Acquire));
        assert!(!storage.poisoned.load(Ordering::Acquire));

        let rebound =
            RxPoolStorage::pin_static_model(storage, DmaFrameAddress::try_new(DMA_LOW).unwrap())
                .unwrap();
        assert_eq!(rebound.capacity(), 2);
    }

    #[test]
    fn empty_pool_bind_failure_returns_exact_storage() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(RxPoolStorage::<0>::new()));
        let identity = core::ptr::from_mut(storage);
        let failure = match RxPoolStorage::pin_static_model(
            storage,
            DmaFrameAddress::try_new(DMA_LOW).unwrap(),
        ) {
            Ok(_) => panic!("empty pool must fail closed"),
            Err(failure) => failure,
        };

        assert_eq!(failure.error(), RxPoolError::EmptyPool);
        let (storage, error) = failure.into_parts();
        assert_eq!(core::ptr::from_mut(storage), identity);
        assert_eq!(error, RxPoolError::EmptyPool);
        assert!(!storage.active.load(Ordering::Acquire));
        assert!(!storage.poisoned.load(Ordering::Acquire));
    }

    #[test]
    fn free_armed_delivered_free_transition_preserves_layout() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let RxArm::Buffer(mut armed) = pool.arm_next().unwrap() else {
            panic!("first arm must select the delivery slot");
        };
        assert_eq!(armed.index(), 0);
        assert_eq!(armed.dma_address().as_u32(), DMA_LOW);
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Armed));

        let phr = MIN_PHR_LENGTH;
        let mut image = [0; FRAME_BUFFER_SIZE];
        image[0] = phr;
        image[1] = 0xa5;
        image[phr as usize - 1] = (-37_i8) as u8;
        image[phr as usize] = 199;
        armed.write_model(&image);
        let delivered = armed.complete_model().unwrap();
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
        let view = delivered.frame().unwrap();
        assert_eq!(view.mac_bytes(), &[0xa5]);
        assert_eq!(view.rssi(), -37);
        assert_eq!(view.lqi(), 199);
        delivered.release().unwrap();
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Free));
    }

    #[test]
    fn all_delivery_slots_are_used_before_stub() {
        let pool = pool::<2>(DMA_LOW).unwrap();
        let RxArm::Buffer(first) = pool.arm_next().unwrap() else {
            panic!();
        };
        let first = first.complete_model().unwrap();
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));

        let RxArm::Buffer(second) = pool.arm_next().unwrap() else {
            panic!();
        };
        assert_eq!(first.index(), 0);
        assert_eq!(second.index(), 1);
        let second = second.complete_model().unwrap();
        assert_eq!(pool.slot_state(1), Some(RxSlotState::Delivered));

        let RxArm::Stub(stub) = pool.arm_next().unwrap() else {
            panic!("the third arm must use the separate stub");
        };
        assert_eq!(
            stub.dma_address().as_u32(),
            DMA_LOW + 2 * FRAME_BUFFER_SIZE as u32
        );
        assert_eq!(pool.stub_state(), RxSlotState::Armed);
        assert!(matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)));

        let stub = stub.complete_model().unwrap();
        assert_eq!(pool.stub_state(), RxSlotState::Delivered);
        // All ordinary buffers and the stub are retained. Exhaustion must
        // return the active gate, so a repeated attempt is Exhausted too.
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Exhausted)));
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Exhausted)));

        first.release().unwrap();
        second.release().unwrap();
        stub.discard().unwrap();
        assert_eq!(pool.stub_state(), RxSlotState::Free);
    }

    #[test]
    fn reentrant_and_concurrent_arm_are_rejected_until_completion() {
        let pool = pool::<2>(DMA_LOW).unwrap();
        let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
            panic!();
        };

        assert!(matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)));
        std::thread::scope(|scope| {
            let rejected = scope
                .spawn(|| matches!(pool.arm_next(), Err(RxPoolError::HardwareBusy)))
                .join()
                .unwrap();
            assert!(rejected);
        });

        let delivered = armed.complete_model().unwrap();
        let RxArm::Buffer(next) = pool.arm_next().unwrap() else {
            panic!("successful completion must release the active gate");
        };
        assert_eq!(next.index(), 1);
        next.complete_model().unwrap().release().unwrap();
        delivered.release().unwrap();
    }

    #[test]
    fn failed_completion_keeps_active_gate_closed() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
            panic!();
        };

        // Fault injection models corrupted lifecycle evidence at the external
        // completion boundary. Production callers cannot mutate this atomic.
        pool.storage.states[0].store(STATE_FREE, Ordering::Release);
        assert!(matches!(
            armed.complete_model(),
            Err(RxPoolError::State {
                expected: RxSlotState::Armed,
                observed: RxSlotState::Free,
            })
        ));
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
    }

    #[test]
    fn aggregate_buffer_completion_failure_retains_terminal_owner() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let arm = pool.arm_next().unwrap();

        // Fault injection stands in for inconsistent external lifecycle
        // evidence. The aggregate API must not return an Armed token to retry.
        pool.storage.states[0].store(STATE_FREE, Ordering::Release);
        let failure = match arm.complete_model() {
            Ok(_) => panic!("mismatched ownership state must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            RxPoolError::State {
                expected: RxSlotState::Armed,
                observed: RxSlotState::Free,
            }
        );
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
    }

    #[test]
    fn aggregate_stub_completion_failure_retains_terminal_owner() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let RxArm::Buffer(frame) = pool.arm_next().unwrap() else {
            panic!();
        };
        let retained_frame = frame.complete_model().unwrap();
        let arm = pool.arm_next().unwrap();
        assert!(matches!(&arm, RxArm::Stub(_)));

        pool.storage.stub_state.store(STATE_FREE, Ordering::Release);
        let failure = match arm.complete_model() {
            Ok(_) => panic!("mismatched stub ownership state must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            RxPoolError::State {
                expected: RxSlotState::Armed,
                observed: RxSlotState::Free,
            }
        );
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
        retained_frame.release().unwrap();
    }

    #[test]
    fn aggregate_completion_and_recycle_cover_frame_and_stub() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let completion = pool.arm_next().unwrap().complete_model().unwrap();
        assert_eq!(completion.kind(), RxCompletionKind::Frame { index: 0 });
        assert!(completion.frame().is_some());

        let retained_frame = pool.arm_next().unwrap().complete_model().unwrap();
        assert_eq!(retained_frame.kind(), RxCompletionKind::Stub);
        assert!(retained_frame.frame().is_none());
        retained_frame.recycle().unwrap();
        completion.recycle().unwrap();
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Free));
        assert_eq!(pool.stub_state(), RxSlotState::Free);
    }

    #[test]
    fn recycle_poison_survives_already_issued_arm_completion() {
        let pool = pool::<2>(DMA_LOW).unwrap();
        let completion = pool.arm_next().unwrap().complete_model().unwrap();
        let concurrent_arm = pool.arm_next().unwrap();
        pool.storage.states[0].store(STATE_ARMED, Ordering::Release);

        let failure = match completion.recycle() {
            Ok(()) => panic!("mismatched delivered state must fail"),
            Err(failure) => failure,
        };
        assert_eq!(
            failure.error(),
            RxPoolError::State {
                expected: RxSlotState::Delivered,
                observed: RxSlotState::Armed,
            }
        );
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));

        // This arm escaped before the mismatch was detected. Its completion
        // clears the transient active bit but must never clear poison.
        let concurrent_completion = concurrent_arm.complete_model().unwrap();
        concurrent_completion.recycle().unwrap();
        assert!(!pool.storage.active.load(Ordering::Acquire));
        assert!(pool.storage.poisoned.load(Ordering::Acquire));
        assert!(matches!(pool.arm_next(), Err(RxPoolError::Poisoned)));
    }

    #[test]
    fn dropped_delivery_is_quarantined_and_stub_absorbs_next_frame() {
        let pool = pool::<1>(DMA_LOW).unwrap();
        let RxArm::Buffer(armed) = pool.arm_next().unwrap() else {
            panic!();
        };
        {
            let _delivered = armed.complete_model().unwrap();
            assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
        }
        assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
        assert!(matches!(pool.arm_next(), Ok(RxArm::Stub(_))));
    }

    #[test]
    fn delivered_view_rejects_invalid_phr_without_recycling() {
        for phr in [0, MIN_PHR_LENGTH - 1, MAX_PHR_LENGTH + 1, u8::MAX] {
            let pool = pool::<1>(DMA_LOW).unwrap();
            let RxArm::Buffer(mut armed) = pool.arm_next().unwrap() else {
                panic!();
            };
            armed.write_model(&[phr]);
            let delivered = armed.complete_model().unwrap();
            assert_eq!(
                delivered.frame().unwrap_err(),
                RxFrameError::PhrLengthOutOfRange { length: phr }
            );
            assert_eq!(pool.slot_state(0), Some(RxSlotState::Delivered));
            delivered.release().unwrap();
        }
    }
}
