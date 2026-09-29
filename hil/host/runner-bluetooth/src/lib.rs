//! Bluetooth LE HIL workloads and the Linux Bluetooth fixture they drive.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod fixture;
pub mod scenario;
pub mod workload;

pub(crate) use hil_core::Result;
pub(crate) use hil_core::emit_json;
