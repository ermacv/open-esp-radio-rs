//! Optional, non-owning observation boundaries.
//!
//! Observers receive value-only events and cannot influence datapath
//! scheduling, protocol state, MMIO ownership or resource lifecycles.

#[cfg(any(feature = "diagnostics", test))]
pub mod access_point;
pub mod aggregate_tx;
pub mod network;
pub mod rx_pipeline;
