//! Host-wide arbitration of the single physical HIL stand: the one owner of
//! hardware exclusion.
//!
//! Every hardware command asks the arbiter for a lease of the boards,
//! fixtures and air it claims. Requests wait in one queue shared by every
//! checkout of the user, ordered by their owners' balances; leases on
//! disjoint resources run in parallel. Once granted, the holder takes the
//! [`lock`] files of what it uses, the arbiter's final exclusion layer. State
//! lives in one directory (the stand model's `paths::arbiter`) guarded by a
//! lock file, so a process that dies leaves a ticket or lease that the next
//! reader reaps. The directory also keeps the lease history, the [`jobs`] of
//! deferred `cargo hil` runs, whose requests are tickets of that queue, and
//! the board [`journal`].
//!
//! The arbiter reads the stand file through the stand model and returns a
//! board's hub port to its working state through board I/O; it actuates no
//! hardware itself.
#![forbid(unsafe_code)]

pub mod balance;
mod estimate;
mod grant;
pub mod health;
mod history;
pub mod jobs;
pub mod journal;
pub mod lock;
pub mod maintenance;
mod notify;
pub mod owners;
pub mod preempt;
mod process;
mod queue;
mod restore;
pub mod spectrum;
mod state;
mod status;
mod store;
mod unknown;

pub use balance::HARD_LIMIT;
pub use estimate::{DEFAULT_ESTIMATE, EstimateSource, format_duration, parse_duration};
pub use grant::{Grant, LEASE_ENV, OWNER_ENV, Request, default_owner, owner_from_environment};
pub use history::{GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance};
pub use journal::{BoardEvent, BoardEventKind, ImageIdentity};
pub use maintenance::{
    Confirmation, Maintenance, QuarantineTrigger, SERVICE_POLL, STAND_SERVICE, ServiceKind,
};
pub use owners::{NoOwner, NotAnOwner, Owner};
pub use state::{AIR, Claim, Mode, Priority, STAND};
pub use status::{HolderStatus, QueuedStatus, Status};
pub use store::Arbiter;
pub use unknown::Unknown;

pub type Result<T> = oer_process::Result<T>;
