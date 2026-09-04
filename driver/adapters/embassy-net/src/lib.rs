#![no_std]
#![forbid(unsafe_code)]

//! Owned-packet network boundary for the optimized Embassy/Xarxa integration.
//!
//! This crate transfers general-memory packet owners between the network stack
//! and the radio. It contains no Wi-Fi scheduling, physical SRAM allocator or
//! compatibility implementation of the released Embassy driver API.

pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use open_esp_radio_network::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

mod owned;

pub use owned::{
    OwnedEndpointResources, OwnedLinkController, OwnedNetworkDevice, OwnedNetworkRunner,
    OwnedNetworkTxFrame, OwnedRxPublisher, OwnedTxFrameSource,
};
