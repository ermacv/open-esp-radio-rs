#![no_std]
#![forbid(unsafe_code)]

//! Sans-IO Bluetooth LE Controller core.
//!
//! [`LeController`] serves the Host's HCI commands and runs the Link Layer
//! roles over the portable radio event contract of `oer-bluetooth-radio`. It
//! owns no transport, executor, clock or hardware: its owner moves command
//! packets in, response and event packets out, and radio requests and
//! outcomes between the roles and one radio backend.
//!
//! The roles run concurrently where the specification allows:
//!
//! - legacy undirected advertising, non-connectable or connectable with its
//!   scan response, with the pseudo-random advertising delay and live
//!   advertising-data updates;
//! - one peripheral connection created by a connection indication to the
//!   connectable set: connection events widened for clock drift, supervision
//!   and establishment timeouts, Channel Map and Connection Update instants,
//!   feature, version, ping and termination procedures, LE encryption start,
//!   pause and restart, and ACL data both ways with Host flow control;
//! - legacy passive scanning with LE Advertising Reports and the duplicate
//!   filter;
//! - Direct Test Mode transmitter and receiver tests, which run alone.
//!
//! One arbiter places every event so that no two reservations overlap.
//! Connection events come first; advertising takes precedence over scanning:
//! a scan window starts after busy reservations and ends before the next
//! advertising or connection event, and an advertising event that cannot
//! start within the advertising delay range is skipped. The connection uses
//! no peripheral latency, the Data Length Extension or a PHY other than LE 1M,
//! and connectable advertising needs the random source that encryption draws
//! from.
//!
//! Commands complete in order. Reset, advertising and scanning enable
//! changes, advertising-data updates while advertising and Test End complete
//! only after their radio work has finished; until then
//! [`LeController::is_command_ready`] is false. Host ACL packets enter through
//! [`LeController::acl`] while [`LeController::is_acl_ready`].

#[cfg(test)]
extern crate std;

mod advertising;
mod arbiter;
mod controller;
mod dtm;
mod output;
mod peripheral;
mod scanning;

pub use controller::{ControllerBusy, LeController, LeControllerConfig, PLANNING_SLACK};
pub use oer_bluetooth_ll::control::LeVersionInformation;
pub use output::{HCI_PACKET_CAPACITY, HciPacket};
pub use scanning::{DUPLICATE_FILTER_CAPACITY, MINIMUM_SCAN_WINDOW};

#[cfg(test)]
mod tests;
