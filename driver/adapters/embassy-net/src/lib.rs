#![no_std]
#![forbid(unsafe_code)]

//! Owned-packet network boundary for the optimized Embassy/Xarxa integration.
//!
//! This crate transfers general-memory packet owners between the network stack
//! and the radio. It contains no Wi-Fi scheduling, physical SRAM allocator or
//! compatibility implementation of the released Embassy driver API.

pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use owned_embassy_net_driver::LinkState;

mod owned;

pub use owned::{
    OwnedEndpointResources, OwnedLinkController, OwnedNetworkDevice, OwnedNetworkRunner,
    OwnedNetworkTxFrame, OwnedRxPublisher, OwnedTxFrameSource,
};

/// Ethernet header length, excluding an FCS.
pub const ETHERNET_HEADER_LEN: usize = 14;

/// Opaque identity of one logical network endpoint sharing a physical radio.
///
/// The network adapter preserves this value but never assigns Wi-Fi meaning
/// to it. The radio composition owns the mapping to STA, AP, or another VIF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceId(u8);

impl NetworkInterfaceId {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Why a byte slice cannot be represented by an owned Ethernet frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameLengthError {
    /// The slice is shorter than an Ethernet header.
    TooShort,
    /// The slice exceeds the configured frame storage.
    TooLong,
}

/// Why a received frame was not admitted to the network stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxEnqueueError {
    /// The supplied Ethernet frame length is invalid.
    InvalidLength(FrameLengthError),
    /// The fixed receive queue is full.
    QueueFull,
    /// The dedicated receive packet pool has no free owner.
    PoolExhausted,
    /// The logical network interface is not active.
    LinkDown,
}
