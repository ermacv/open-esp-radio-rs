//! IEEE 802.15.4 HIL workloads.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod peer;
pub mod scenario;
pub mod thread_peer;
pub mod workload;

pub(crate) use oer_hil_execution::Result;
