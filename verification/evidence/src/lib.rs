//! The native vendor-comparison evidence index: a directory of shards, one
//! per scenario, so each scenario run rewrites only its own file. A scenario
//! writes its shard after it passed; qualification consumes the directory.
//!
//! - [`index`]: the shard format ([`Index`]), its validation and whether its
//!   recorded sources are still current, and the cross-scenario views of a
//!   whole directory ([`Evidence`]);
//! - [`store`]: the one reader and writer of shard files, and which shards
//!   of a directory are stale;
//! - [`policy`]: the verdict source policy — the path closure a shard may
//!   record, without the report packages that decide no verdict;
//! - [`diff`]: what two versions of one shard say differently;
//! - [`producer`]: the contract of whatever compares vendor code with
//!   compiled production code and writes shards (the Blobray scenario
//!   engine, a host stand), and [`producer::host_stand`], the host stands'
//!   shard;
//! - `run` (feature `producers`): building the probes and the Blobray
//!   scenarios, regenerating and checking a chip's shards.
//!
//! Its dependencies stay light (the repository model, serde, SHA-256), so
//! the Blobray workspace, the host stands, xtask and the qualification
//! evaluator all read and write shards through it.
#![forbid(unsafe_code)]

pub mod diff;
pub mod index;
pub mod policy;
pub mod producer;
#[cfg(feature = "producers")]
pub mod run;
pub mod store;

pub use index::*;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
