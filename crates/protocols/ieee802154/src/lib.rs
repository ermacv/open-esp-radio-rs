#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Hardware-independent IEEE 802.15.4 radio boundary.
//!
//! This crate owns bounded MAC bytes, normalized metadata, portable radio
//! values and a finite command/event state machine. It contains no PHR/FCS DMA
//! image, ESP register layout, interrupt owner, allocator, executor or async
//! trait. Platform adapters translate these contracts to their own hardware
//! ownership model; OpenThread- and Zephyr-facing code can use them without
//! inheriting that platform representation.

#[cfg(test)]
extern crate std;

/// Bounded IEEE 802.15.4 MAC byte representations.
pub mod mac {
    /// Owned and borrowed MAC frames without platform DMA framing.
    pub mod frame;
}

/// Hardware-independent radio command/event and state contracts.
pub mod radio;

pub use mac::frame::{Frame, FrameError, FrameView, MAX_MAC_FRAME_LEN, MIN_MAC_FRAME_LEN};
pub use radio::capabilities::{CapabilityBitsError, RadioCapabilities};
pub use radio::channel::{Channel, ChannelError};
pub use radio::command::{
    CommandKind, Configuration, EnergyScanRequest, RadioCommand, TxMode, TxRequest,
};
pub use radio::event::{
    FcsStatus, FramePending, RadioEvent, RadioFault, ReceivedFrame, RxMetadata, SecurityStatus,
    TxStatus,
};
pub use radio::state::{
    AcceptedCommand, CommandError, EventError, RadioState, RadioStateMachine, RestingState,
};
pub use radio::{RadioTimestamp, RequestId};
