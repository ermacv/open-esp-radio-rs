//! Wi-Fi HIL workloads and the laptop and OpenWrt fixtures they drive.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod evidence;
pub mod fixture;
pub mod scenario;
pub mod workload;

pub(crate) use hil_core::Result;
