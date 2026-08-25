//! Pinned transmit-buffer ownership.

use core::{
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
};

use crate::{
    DmaAddressError, DmaFrameAddress, DmaTerminalEvidence, FRAME_BUFFER_SIZE, TxFrameError,
    TxFrameView, frame::prepare_tx, ordering::device_fence,
};

#[repr(C, align(4))]
struct TxFrameBuffer([u8; FRAME_BUFFER_SIZE]);

const _: () = {
    assert!(core::mem::size_of::<TxFrameBuffer>() == FRAME_BUFFER_SIZE);
    assert!(core::mem::align_of::<TxFrameBuffer>() == 4);
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TxState {
    #[default]
    Free,
    Prepared,
    Armed,
    Completed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxStorageError {
    Address(DmaAddressError),
    Frame(TxFrameError),
    State {
        expected: TxState,
        observed: TxState,
    },
    AddressWidth,
}

impl From<DmaAddressError> for TxStorageError {
    fn from(error: DmaAddressError) -> Self {
        Self::Address(error)
    }
}

impl From<TxFrameError> for TxStorageError {
    fn from(error: TxFrameError) -> Self {
        Self::Frame(error)
    }
}

/// Failed target-address binding that retains the exact unpinned allocation.
///
/// Binding validates the complete address before creating a pinned owner or
/// publishing anything to hardware. It is therefore safe to recover the
/// allocation and try a different binding policy.
///
/// The failure is linear and cannot be cloned:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::TxBindFailure;
///
/// fn duplicate(failure: TxBindFailure) {
///     let moved = failure;
///     let _ = failure.error();
///     drop(moved);
/// }
/// ```
pub struct TxBindFailure {
    storage: &'static mut TxStorage,
    error: TxStorageError,
}

impl TxBindFailure {
    #[cfg(target_arch = "riscv32")]
    fn new(storage: &'static mut TxStorage, error: TxStorageError) -> Self {
        Self { storage, error }
    }

    pub const fn error(&self) -> TxStorageError {
        self.error
    }

    /// Recover the unchanged allocation together with the binding error.
    pub fn into_parts(self) -> (&'static mut TxStorage, TxStorageError) {
        (self.storage, self.error)
    }
}

impl core::fmt::Debug for TxBindFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("TxBindFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Permanently located allocation for one 128-byte transmit frame.
///
/// Construct it in static storage and bind it exactly once with `pin_static`
/// on the RISC-V S31 target or [`Self::pin_static_model`] on a native host.
#[repr(C)]
pub struct TxStorage {
    frame: TxFrameBuffer,
    state: TxState,
    _pin: PhantomPinned,
}

impl TxStorage {
    pub const fn new() -> Self {
        Self {
            frame: TxFrameBuffer([0; FRAME_BUFFER_SIZE]),
            state: TxState::Free,
            _pin: PhantomPinned,
        }
    }

    #[cfg(target_arch = "riscv32")]
    pub fn pin_static(storage: &'static mut Self) -> Result<PinnedTxBuffer, TxBindFailure> {
        let address = match u32::try_from(core::ptr::addr_of!(storage.frame).addr()) {
            Ok(address) => address,
            Err(_) => {
                return Err(TxBindFailure::new(storage, TxStorageError::AddressWidth));
            }
        };
        let address = match DmaFrameAddress::try_new(address) {
            Ok(address) => address,
            Err(error) => return Err(TxBindFailure::new(storage, error.into())),
        };
        Ok(Self::pin_static_inner(storage, address))
    }

    /// Bind deterministic internal-SRAM address evidence to a native model.
    ///
    /// The model accepts a validated token, never a raw integer.
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_ieee802154_dma::{DMA_LOW, TxStorage};
    ///
    /// let storage = Box::leak(Box::new(TxStorage::new()));
    /// let _ = TxStorage::pin_static_model(storage, DMA_LOW);
    /// ```
    #[cfg(not(target_arch = "riscv32"))]
    pub fn pin_static_model(
        storage: &'static mut Self,
        address: DmaFrameAddress,
    ) -> PinnedTxBuffer {
        Self::pin_static_inner(storage, address)
    }

    fn pin_static_inner(storage: &'static mut Self, address: DmaFrameAddress) -> PinnedTxBuffer {
        PinnedTxBuffer {
            storage: Pin::static_mut(storage),
            address,
        }
    }
}

impl Default for TxStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique movable owner of one pinned transmit allocation.
pub struct PinnedTxBuffer {
    storage: Pin<&'static mut TxStorage>,
    address: DmaFrameAddress,
}

/// Type-level proof that the immutable prepared FCF requests an ACK.
///
/// The type is uninhabited and the DMA owner is the only code that can mint a
/// token carrying it.
pub enum TxAckRequested {}

/// Type-level proof that the immutable prepared FCF does not request an ACK.
///
/// The type is uninhabited and the DMA owner is the only code that can mint a
/// token carrying it.
pub enum TxAckNotRequested {}

/// ACK classification of one successfully prepared immutable DMA image.
///
/// Callers must handle the FCF-derived variant; there is no caller-selected ACK
/// argument and no conversion between the two proof modes.
pub enum PreparedTx<'owner> {
    AckRequested(TxPrepared<'owner, TxAckRequested>),
    AckNotRequested(TxPrepared<'owner, TxAckNotRequested>),
}

impl PinnedTxBuffer {
    pub fn state(&self) -> TxState {
        self.storage.as_ref().get_ref().state
    }

    /// Prepare a complete PHR + MAC + reserved-FCS image.
    ///
    /// The returned linear token retains the mutable owner borrow, so another
    /// prepare or any buffer mutation cannot occur while it exists.
    ///
    /// ```compile_fail
    /// use open_esp_radio_esp32s31_ieee802154_dma::{DmaFrameAddress, PinnedTxBuffer, TxStorage};
    ///
    /// let storage = Box::leak(Box::new(TxStorage::new()));
    /// let address = DmaFrameAddress::try_new(0x2f00_0000).unwrap();
    /// let mut owner = TxStorage::pin_static_model(storage, address);
    /// let first = owner.prepare(&[0x01]).unwrap();
    /// let _second = owner.prepare(&[0x02]).unwrap();
    /// drop(first);
    /// ```
    pub fn prepare(&mut self, mac_frame: &[u8]) -> Result<PreparedTx<'_>, TxStorageError> {
        let observed = self.state();
        if observed != TxState::Free {
            return Err(TxStorageError::State {
                expected: TxState::Free,
                observed,
            });
        }

        // Validate before touching the backing image or its lifecycle.
        if mac_frame.is_empty() || mac_frame.len() > crate::MAX_MAC_FRAME_SIZE {
            return Err(TxFrameError::MacLengthOutOfRange {
                length: mac_frame.len(),
            }
            .into());
        }

        let storage = self.storage_mut();
        let phr_length = prepare_tx(&mut storage.frame.0, mac_frame)?;
        let acknowledgement_requested =
            TxFrameView::new(&storage.frame.0, phr_length).acknowledgement_requested();
        storage.state = TxState::Prepared;
        if acknowledgement_requested {
            Ok(PreparedTx::AckRequested(TxPrepared {
                owner: self,
                phr_length,
                _mode: PhantomData,
            }))
        } else {
            Ok(PreparedTx::AckNotRequested(TxPrepared {
                owner: self,
                phr_length,
                _mode: PhantomData,
            }))
        }
    }

    #[allow(
        unsafe_code,
        reason = "pin projection mutates fields without moving the !Unpin allocation"
    )]
    fn storage_mut(&mut self) -> &mut TxStorage {
        // SAFETY: this method never moves `frame` or `_pin`; it only mutates
        // fields in the allocation permanently retained by `self.storage`.
        unsafe { self.storage.as_mut().get_unchecked_mut() }
    }
}

/// CPU-owned, immutable prepared transmit image.
pub struct TxPrepared<'owner, Mode> {
    owner: &'owner mut PinnedTxBuffer,
    phr_length: u8,
    _mode: PhantomData<Mode>,
}

impl<'owner, Mode> TxPrepared<'owner, Mode> {
    pub fn frame(&self) -> TxFrameView<'_> {
        TxFrameView::new(
            &self.owner.storage.as_ref().get_ref().frame.0,
            self.phr_length,
        )
    }

    /// Publish all buffer writes before a later MMIO address/command write.
    pub fn arm(self) -> TxArmed<'owner, Mode> {
        device_fence();
        self.owner.storage_mut().state = TxState::Armed;
        TxArmed {
            owner: self.owner,
            phr_length: self.phr_length,
            _mode: PhantomData,
        }
    }

    pub fn cancel(self) {
        self.owner.storage_mut().state = TxState::Free;
    }
}

/// Borrowed address authority for one currently hardware-owned TX image.
///
/// Copies remain bounded by the borrow of the exact armed owner and cannot be
/// replayed after that owner crosses its terminal boundary:
///
/// ```compile_fail
/// use open_esp_radio_esp32s31_ieee802154_dma::{
///     DmaTerminalEvidence, TxAckNotRequested, TxArmed,
/// };
///
/// fn replay_after_completion<'owner>(
///     armed: TxArmed<'owner, TxAckNotRequested>,
///     terminal: &DmaTerminalEvidence,
/// ) {
///     let address = armed.dma_address();
///     let _completed = armed.complete(terminal);
///     let _ = address.as_u32();
/// }
/// ```
#[derive(Clone, Copy)]
pub struct TxDmaAddress<'armed> {
    address: DmaFrameAddress,
    _armed: PhantomData<&'armed PinnedTxBuffer>,
}

impl TxDmaAddress<'_> {
    pub const fn as_u32(self) -> u32 {
        self.address.as_u32()
    }
}

/// Hardware-owned TX image. The token is deliberately neither `Clone` nor
/// `Copy` and retains exclusive access to its pinned allocation.
pub struct TxArmed<'owner, Mode> {
    owner: &'owner mut PinnedTxBuffer,
    phr_length: u8,
    _mode: PhantomData<Mode>,
}

impl<'owner, Mode> TxArmed<'owner, Mode> {
    pub fn dma_address(&self) -> TxDmaAddress<'_> {
        TxDmaAddress {
            address: self.owner.address,
            _armed: PhantomData,
        }
    }

    /// Transfer a terminal TX image back to CPU ownership.
    ///
    /// Evidence is minted only by the sealed runtime after the exact active
    /// operation accepts an acknowledged terminal batch. No raw event or
    /// caller-provided completion flag can reach this boundary.
    pub fn complete(self, _terminal: &DmaTerminalEvidence) -> TxCompleted<'owner> {
        self.complete_inner()
    }

    fn complete_inner(self) -> TxCompleted<'owner> {
        device_fence();
        self.owner.storage_mut().state = TxState::Completed;
        TxCompleted {
            owner: self.owner,
            phr_length: self.phr_length,
        }
    }
}

/// CPU-owned terminal TX token.
pub struct TxCompleted<'owner> {
    owner: &'owner mut PinnedTxBuffer,
    phr_length: u8,
}

impl TxCompleted<'_> {
    pub fn frame(&self) -> TxFrameView<'_> {
        TxFrameView::new(
            &self.owner.storage.as_ref().get_ref().frame.0,
            self.phr_length,
        )
    }

    pub fn release(self) {
        self.owner.storage_mut().state = TxState::Free;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DMA_LOW, MAX_MAC_FRAME_SIZE};

    fn owner(address: u32) -> PinnedTxBuffer {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(TxStorage::new()));
        TxStorage::pin_static_model(storage, DmaFrameAddress::try_new(address).unwrap())
    }

    #[test]
    fn storage_frame_has_exact_geometry() {
        assert_eq!(core::mem::size_of::<TxFrameBuffer>(), FRAME_BUFFER_SIZE);
        assert_eq!(core::mem::align_of::<TxFrameBuffer>(), 4);
        let storage = TxStorage::new();
        assert_eq!((&storage.frame as *const TxFrameBuffer).addr() & 3, 0);
    }

    #[test]
    fn prepared_armed_completed_release_is_linear() {
        let mut owner = owner(DMA_LOW);
        let PreparedTx::AckRequested(prepared) = owner.prepare(&[0x61, 0x88, 0x01]).unwrap() else {
            panic!("FCF requests an ACK");
        };
        assert_eq!(prepared.frame().phr_length(), 5);
        assert_eq!(prepared.frame().reserved_fcs(), &[0, 0]);
        let armed = prepared.arm();
        assert_eq!(armed.dma_address().as_u32(), DMA_LOW);
        let terminal = DmaTerminalEvidence::for_native_model();
        let completed = armed.complete(&terminal);
        assert_eq!(completed.frame().mac_bytes(), &[0x61, 0x88, 0x01]);
        completed.release();
        assert_eq!(owner.state(), TxState::Free);
    }

    #[test]
    fn cancel_returns_prepared_buffer_to_free() {
        let mut owner = owner(DMA_LOW);
        let PreparedTx::AckNotRequested(prepared) = owner.prepare(&[1]).unwrap() else {
            panic!("FCF does not request an ACK");
        };
        prepared.cancel();
        assert_eq!(owner.state(), TxState::Free);
        assert!(owner.prepare(&[2]).is_ok());
    }

    #[test]
    fn every_fcf_octet_mints_only_its_immutable_image_mode() {
        let mut owner = owner(DMA_LOW);
        for fcf in u8::MIN..=u8::MAX {
            let original = owner.storage.as_ref().get_ref().frame.0;
            let frame_type = fcf & 0x07;
            match owner.prepare(&[fcf]) {
                Ok(PreparedTx::AckRequested(prepared)) => {
                    assert!(frame_type <= 3, "FCF 0x{fcf:02x}");
                    assert_ne!(fcf & 0x20, 0, "FCF 0x{fcf:02x}");
                    assert_eq!(prepared.frame().buffer()[1], fcf);
                    assert!(prepared.frame().acknowledgement_requested());
                    prepared.cancel();
                }
                Ok(PreparedTx::AckNotRequested(prepared)) => {
                    assert!(frame_type <= 3, "FCF 0x{fcf:02x}");
                    assert_eq!(fcf & 0x20, 0, "FCF 0x{fcf:02x}");
                    assert_eq!(prepared.frame().buffer()[1], fcf);
                    assert!(!prepared.frame().acknowledgement_requested());
                    prepared.cancel();
                }
                Err(TxStorageError::Frame(TxFrameError::UnsupportedFrameType {
                    frame_type: rejected,
                })) => {
                    assert!(frame_type > 3, "FCF 0x{fcf:02x}");
                    assert_eq!(rejected, frame_type);
                    assert_eq!(owner.state(), TxState::Free);
                    assert_eq!(owner.storage.as_ref().get_ref().frame.0, original);
                }
                Err(error) => panic!("unexpected FCF 0x{fcf:02x} error: {error:?}"),
            }
            assert_eq!(owner.state(), TxState::Free);
        }
    }

    #[test]
    fn mode_is_bound_to_the_copy_not_the_caller_slice() {
        let mut owner = owner(DMA_LOW);
        let mut source = [0x21, 0x88, 0x01];
        let PreparedTx::AckRequested(prepared) = owner.prepare(&source).unwrap() else {
            panic!("fixture requests an ACK");
        };

        source[0] = 0x01;
        assert_eq!(source[0], 0x01);
        assert_eq!(prepared.frame().mac_bytes(), &[0x21, 0x88, 0x01]);
        assert!(prepared.frame().acknowledgement_requested());

        let armed = prepared.arm();
        let completed = armed.complete(&DmaTerminalEvidence::for_native_model());
        completed.release();
        assert_eq!(owner.state(), TxState::Free);
    }

    #[test]
    fn invalid_input_leaves_storage_free() {
        let mut owner = owner(DMA_LOW);
        assert!(matches!(
            owner.prepare(&[]),
            Err(TxStorageError::Frame(TxFrameError::MacLengthOutOfRange {
                length: 0
            }))
        ));
        assert!(matches!(
            owner.prepare(&[0; MAX_MAC_FRAME_SIZE + 1]),
            Err(TxStorageError::Frame(
                TxFrameError::MacLengthOutOfRange { .. }
            ))
        ));
        assert_eq!(owner.state(), TxState::Free);
    }
}
