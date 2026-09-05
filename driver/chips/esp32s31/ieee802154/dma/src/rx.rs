//! Pinned receive pool and linear ownership tokens.

use core::{
    cell::UnsafeCell,
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

use crate::{
    DmaAddressError, DmaFrameAddress, DmaTerminalEvidence, FRAME_BUFFER_SIZE, RxFrameError,
    RxFrameView, ordering::device_fence,
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
    /// Transfer either terminal RX resource to CPU ownership.
    ///
    /// Evidence is minted only by the sealed runtime after the exact active
    /// operation accepts an acknowledged terminal batch. The completion keeps
    /// ordinary delivery buffers distinct from the drop stub and must be
    /// explicitly recycled before that slot can be armed again.
    pub fn complete(
        self,
        _terminal: &DmaTerminalEvidence,
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
    /// This model-only API returns only the error after poisoning the complete
    /// pool. Production reclaim uses aggregate [`RxArm::complete`] with affine
    /// terminal evidence and retains a failed arm as an opaque terminal owner.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn complete_model(self) -> Result<RxDelivered<'pool, COUNT>, RxPoolError> {
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
    /// Model transition that poisons the pool and drops the ordinary arm on
    /// mismatch. Production uses aggregate [`RxArm::complete`].
    pub fn complete_model(self) -> Result<RxStubDelivered<'pool, COUNT>, RxPoolError> {
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
mod tests;
