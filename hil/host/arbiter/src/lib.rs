//! Host-wide arbitration of the single physical HIL stand.
//!
//! Every hardware command asks the arbiter for a lease before it takes the
//! existing fixture `flock`s. Requests wait in one FIFO queue shared by every
//! checkout of the user; a short request may be granted ahead of the head once
//! in a row. A lease covers flashing and running only. It carries a budget
//! (explicit, estimated from earlier leases of the same work, or a default):
//! the holder is warned at the budget and terminated at twice the budget.
//!
//! The arbiter orders access; it does not replace the fixture locks, which
//! remain the final exclusion. State lives in one directory guarded by a lock
//! file, so a process that dies leaves a ticket or lease that the next reader
//! reaps. The directory also keeps the lease history and a journal of board
//! state changes (flashed firmware, startup artifact uploads and writes).
#![forbid(unsafe_code)]

mod board;
mod budget;
mod devices;
mod grant;
mod history;
pub mod maintenance;
mod notify;
mod process;
mod queue;
pub mod spectrum;
mod state;
mod status;
mod store;

pub use board::{BoardEvent, BoardEventKind};
pub use budget::{
    BRIEF_BUDGET, BudgetSource, DEFAULT_BUDGET, MAX_SHORT_BUDGET, format_duration, parse_duration,
};
pub use devices::{
    AttachedPort, Device, attached_ports, board_mac, device_label, normalize_mac, port_mac,
};
pub use grant::{BUDGET_ENV, Grant, LEASE_ENV, OWNER_ENV, Request, SHORT_ENV, default_owner};
pub use history::{LeaseOutcome, LeaseRecord};
pub use maintenance::Maintenance;
pub use process::process_started_unix_millis;
pub use state::{AIR, Claim, Mode, STAND};
pub use status::{HolderStatus, QueuedStatus, Status};
pub use store::{Arbiter, DIRECTORY_ENV};

pub type Result<T> = oer_process::Result<T>;

/// Seconds since the Unix epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
