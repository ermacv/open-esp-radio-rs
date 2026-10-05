//! Repository operations behind `cargo xtask`: the check registry and the
//! gate that selects from it, push, CI state, locks, worktrees and sweeps,
//! and the repository checks. Domain work (images, vendor evidence,
//! registers) is its owners'; the binary parses arguments and calls them.

pub mod cargo;
pub mod checks;
pub mod ci_status;
pub mod doc;
pub mod firmware;
pub mod gate;
pub mod graph;
pub mod hooks;
pub mod push;
pub mod registry;
pub mod report;
pub mod stand_install;
pub mod sweep;
pub mod worktree;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
