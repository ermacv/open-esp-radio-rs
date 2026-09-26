//! IEEE 802.15.4 HIL workloads.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod peer;
pub mod scenario;
pub mod workload;

pub(crate) use hil_core::Result;
