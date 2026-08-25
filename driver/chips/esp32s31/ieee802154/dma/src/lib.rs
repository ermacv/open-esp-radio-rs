#![no_std]
#![deny(unsafe_code)]

//! Safe memory and ownership boundary for the ESP32-S31 IEEE 802.15.4 MAC DMA.
//!
//! This leaf contains no MMIO, command, interrupt, or RF behavior. It only
//! prepares permanently located 128-byte frame buffers, validates their
//! internal-SRAM addresses, and represents CPU/DMA ownership transitions with
//! non-clone tokens.

#[cfg(test)]
extern crate std;

mod address;
mod frame;
mod ordering;
mod rx;
mod terminal;
mod tx;

pub use address::{DMA_HIGH, DMA_LOW, DmaAddressError, DmaFrameAddress};
pub use frame::{
    FRAME_BUFFER_SIZE, MAX_MAC_FRAME_SIZE, MAX_PHR_LENGTH, MIN_MAC_FRAME_SIZE, MIN_PHR_LENGTH,
    RxFrameError, RxFrameView, TxFrameError, TxFrameView,
};
pub use rx::{
    PinnedRxPool, RxArm, RxArmed, RxCompletion, RxCompletionKind, RxDelivered, RxDmaAddress,
    RxLifecycleFailure, RxPoolBindFailure, RxPoolError, RxPoolStorage, RxSlotState, RxStubArmed,
    RxStubDelivered,
};
pub use terminal::DmaTerminalEvidence;
pub use tx::{
    PinnedTxBuffer, PreparedTx, TxAckNotRequested, TxAckRequested, TxArmed, TxBindFailure,
    TxCompleted, TxDmaAddress, TxPrepared, TxState, TxStorage, TxStorageError,
};
