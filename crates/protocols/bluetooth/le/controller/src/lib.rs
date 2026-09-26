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
//! - legacy non-connectable undirected advertising, with the pseudo-random
//!   advertising delay and live advertising-data updates;
//! - legacy passive scanning with LE Advertising Reports and the duplicate
//!   filter;
//! - Direct Test Mode transmitter and receiver tests, which run alone.
//!
//! One arbiter places every event so that no two reservations overlap.
//! Advertising takes precedence: a scan window starts after busy
//! reservations and ends before the next advertising event, and an
//! advertising event that cannot start within the advertising delay range is
//! skipped. Connectable advertising and connections are not implemented;
//! enabling a connectable set completes with Unsupported Feature or
//! Parameter Value, and connection commands report an unknown connection.
//!
//! Commands complete in order. Reset, advertising and scanning enable
//! changes, advertising-data updates while advertising and Test End complete
//! only after their radio work has finished; until then
//! [`LeController::is_command_ready`] is false.

#[cfg(test)]
extern crate std;

mod advertising;
mod arbiter;
mod controller;
mod dtm;
mod output;
mod scanning;

pub use controller::{ControllerBusy, LeController, PLANNING_SLACK};
pub use output::{HCI_PACKET_CAPACITY, HciPacket};
pub use scanning::{DUPLICATE_FILTER_CAPACITY, MINIMUM_SCAN_WINDOW};

#[cfg(test)]
mod tests;
