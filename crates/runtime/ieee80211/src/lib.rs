#![no_std]
#![forbid(unsafe_code)]

//! Chip-independent Wi-Fi execution primitives shared by chip runtimes.
//!
//! These are executor-independent async ownership contracts over the same
//! `embassy-sync`/`embassy-time` interfaces as the chip runtimes; they bind no
//! executor.
//!
//! [`monitor`] provides bounded capture and injection handoffs. [`connected_tasks`]
//! coordinates finite task shutdown, and [`station_network`] retains network
//! ownership across station associations. [`stack_boundary`] provides an explicit
//! poll boundary for large async state machines. Hardware access and concrete
//! task assembly belong to the chip backend and its integration.

pub mod connected_tasks;
pub mod monitor;
pub mod stack_boundary;
pub mod station_network;

pub use monitor::injection as monitor_injection;
pub use monitor::{
    MonitorCaptureFrame, MonitorCaptureMetadata, MonitorCapturePool, MonitorCaptureReceiver,
    MonitorCaptureResources, MonitorCaptureSink,
};

#[cfg(test)]
extern crate std;
