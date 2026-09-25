#![no_std]
//! Application-owned IP services used by the ESP32-S31 AP example.

pub mod dhcp;
pub mod services;

#[cfg(any(
    all(feature = "owned-network", feature = "upstream-network"),
    all(feature = "owned-network", feature = "embassy-network"),
    all(feature = "upstream-network", feature = "embassy-network"),
))]
compile_error!("select exactly one network integration");
#[cfg(not(any(
    feature = "owned-network",
    feature = "upstream-network",
    feature = "embassy-network"
)))]
compile_error!("select exactly one network integration");
#[cfg(feature = "owned-network")]
extern crate embassy_net_owned as embassy_net;
#[cfg(feature = "embassy-network")]
extern crate embassy_net_released as embassy_net;
#[cfg(feature = "upstream-network")]
extern crate embassy_net_upstream as embassy_net;
pub mod network;
