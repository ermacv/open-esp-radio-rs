//! Optional, non-owning observation boundaries.
//!
//! Observers receive value-only events and cannot influence datapath
//! scheduling, protocol state, MMIO ownership or resource lifecycles.

#[cfg(any(feature = "diagnostics", test))]
pub mod access_point;
pub mod aggregate_tx;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_ap_rx_cycles;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_cycles;
#[cfg(any(feature = "task-poll-telemetry", feature = "core0-rx-coarse-telemetry"))]
pub mod core0_rx_performance;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_reorder_cycles;
#[cfg(feature = "task-poll-telemetry")]
pub mod core0_rx_service_histogram;
#[cfg(feature = "tx-phase-telemetry")]
pub mod egress;
#[cfg(feature = "diagnostics")]
pub mod network;
pub mod rx_pipeline;
