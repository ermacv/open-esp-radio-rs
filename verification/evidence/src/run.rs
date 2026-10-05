//! The producers' orchestration, behind the `producers` feature so the
//! Blobray workspace and the host stands keep the light format crate:
//!
//! - [`probes`]: each chip's Rust comparison probe images, built and their
//!   probe catalogs validated;
//! - [`scenario`]: the typed Blobray vendor scenarios, built in the Blobray
//!   workspace and run one at a time;
//! - [`regenerate`]: which producer reruns which stale or named shard, the
//!   rerun check against committed shards, and the untriaged listing.
//!
//! Each long step prints a `phase <name>: <seconds> s` line ([`phase`]).

pub mod phase;
pub mod probes;
pub mod regenerate;
pub mod scenario;
