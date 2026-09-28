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

pub mod balance;
mod board;
pub mod control;
mod devices;
mod estimate;
mod grant;
pub mod health;
mod history;
pub mod maintenance;
mod notify;
pub mod preempt;
mod process;
mod queue;
pub mod spectrum;
mod state;
mod status;
mod store;
mod unknown;

pub use balance::HARD_LIMIT;
pub use board::{BoardEvent, BoardEventKind, RecoveryStep, ResetPath};
pub use control::{BootMode, Control, ResetControl};
pub use devices::{
    AttachedPort, Device, attached_ports, board_mac, device_label, normalize_mac, port_mac,
};
pub use estimate::{DEFAULT_ESTIMATE, EstimateSource, format_duration, parse_duration};
pub use grant::{Grant, LEASE_ENV, NO_BUDGETS, OWNER_ENV, Request, default_owner};
pub use history::{GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance};
pub use maintenance::{Confirmation, Maintenance, QuarantineTrigger, ServiceKind};
pub use process::process_started_unix_millis;
pub use state::{AIR, Claim, Mode, STAND};
pub use status::{HolderStatus, QueuedStatus, Status};
pub use store::{Arbiter, DIRECTORY_ENV};
pub use unknown::Unknown;

pub type Result<T> = oer_process::Result<T>;

/// Milliseconds since the Unix epoch.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Seconds since the Unix epoch.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
