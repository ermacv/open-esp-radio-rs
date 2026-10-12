//! Role-local protocol runtimes.
//!
//! These modules own the access point's and the monitor's policy. Physical
//! RX, TX and IRQ arbitration stays in [`crate::datapath`]. The station runs
//! as the portable station machine on the lower-MAC port (#563).

pub mod access_point;
pub mod monitor;
#[cfg(target_arch = "riscv32")]
pub mod radio_channel;
