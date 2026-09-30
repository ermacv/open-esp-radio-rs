//! System HIL workloads: boot, timebase, watchdog and memory benchmarks.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod scenario;
pub mod workload;

pub(crate) use oer_hil_execution::Result;
