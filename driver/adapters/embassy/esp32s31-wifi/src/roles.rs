//! Role-local protocol runtimes.
//!
//! These modules own STA/AP/scan/monitor policy. Physical RX, TX and IRQ
//! arbitration stays in [`crate::datapath`]; concurrent composition may lend
//! capabilities to roles but does not duplicate their protocol state.

pub mod access_point;
pub mod concurrent;
pub mod monitor;
pub mod scan;
pub mod station;
