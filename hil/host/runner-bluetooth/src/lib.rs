//! Bluetooth LE HIL workloads and the Linux Bluetooth fixture they drive.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod fixture;
pub mod scenario;
pub mod workload;

pub(crate) use oer_hil_execution::Result;
pub(crate) use oer_hil_execution::emit_json;
