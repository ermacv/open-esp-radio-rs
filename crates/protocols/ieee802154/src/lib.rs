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
//!
//! Start with [`RadioStateMachine`], which distinguishes accepted commands
//! from terminal [`RadioEvent`] values while retaining bounded [`Frame`]
//! storage. The ESP32-S31 lower crates separately implement timing,
//! energy-detection/CCA, DMA, IRQ and affine MAC-operation boundaries. There is
//! currently no public RF-ready IEEE 802.15.4 service composition: these types
//! must not be read as an application start API or as hardware readiness.

#[cfg(test)]
extern crate std;

/// Bounded IEEE 802.15.4 MAC byte representations.
pub mod mac {
    /// Owned and borrowed MAC frames without platform DMA framing.
    pub mod frame;
    /// MAC header inspection of `[PHR, PSDU...]` images.
    pub mod header;
    /// Frame-pending table and ACK pending-bit decision.
    pub mod pending;
}

/// Hardware-independent radio command/event and state contracts.
pub mod radio;

pub use mac::frame::{Frame, FrameError, FrameView, MAX_MAC_FRAME_LEN, MIN_MAC_FRAME_LEN};
pub use mac::header::{AddressMode, FrameAddress, FrameType, FrameVersion, PhrFrame};
pub use mac::pending::{AckPending, AutoPendingMode, PendingTable, PendingTableFull, ack_pending};
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
