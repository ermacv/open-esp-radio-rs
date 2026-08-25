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

mod capabilities;
mod channel;
mod contract;
mod frame;
mod types;

pub use capabilities::{CapabilityBitsError, RadioCapabilities};
pub use channel::{Channel, ChannelError};
pub use contract::{
    AcceptedCommand, CommandError, CommandKind, EventError, RadioCommand, RadioEvent, RadioState,
    RadioStateMachine, RestingState,
};
pub use frame::{Frame, FrameError, FrameView, MAX_MAC_FRAME_LEN, MIN_MAC_FRAME_LEN};
pub use types::{
    AcknowledgementPolicy, Configuration, EnergyScanRequest, FcsStatus, FramePending, RadioFault,
    RadioTimestamp, ReceivedFrame, RequestId, RxMetadata, SecurityStatus, TxMode, TxRequest,
    TxStatus,
};
