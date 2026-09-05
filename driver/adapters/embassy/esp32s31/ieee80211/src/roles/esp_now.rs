//! Standalone plaintext ESP-NOW role over an exclusive station-address VIF.
//!
//! This role never enters scan, authentication, association, WPA2 or a
//! network stack. It uses normal station RX filtering (including hardware
//! auto-ACK), the shared bounded ESP-NOW application mailboxes and the sole
//! pre-connected ordinary TX descriptor.

mod channel;
mod rx;
mod service;

pub use channel::Esp32s31StandaloneEspNowChannelControl;
#[cfg(target_arch = "riscv32")]
pub use channel::Esp32s31StandaloneEspNowPhyChannelControl;
pub use rx::{
    Esp32s31StandaloneEspNowReceive, Esp32s31StandaloneEspNowRx, Esp32s31StandaloneEspNowRxProgress,
};
pub use service::{
    Esp32s31StandaloneEspNowBinding, Esp32s31StandaloneEspNowBindingError,
    Esp32s31StandaloneEspNowOffChannelRunError, Esp32s31StandaloneEspNowOffChannelRunFailure,
    Esp32s31StandaloneEspNowRunError, Esp32s31StandaloneEspNowRunFailure,
    Esp32s31StandaloneEspNowRunReport, Esp32s31StandaloneEspNowService,
    Esp32s31StandaloneEspNowStopError, Esp32s31StandaloneEspNowStopped,
};

pub use crate::roles::station::connected::{
    EspNowOffChannelFailureStage, EspNowOwnedRxEvent, EspNowOwnedV1Tx, EspNowRxMailboxEpochError,
    EspNowRxMailboxResources, EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher,
    EspNowRxReceiver, EspNowTxBackpressure, EspNowTxCancelReason, EspNowTxCompletion,
    EspNowTxHandle, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError, EspNowTxMailboxOwner,
    EspNowTxMailboxResources, EspNowTxMailboxShutdown, EspNowTxRuntimeFailure, EspNowTxTerminal,
    EspNowTxTicket, EspNowTxTrySendError, EspNowV2RxEvent, EspNowV2RxMailboxError,
    EspNowV2TxTrySendError,
};
