//! Fixture ownership shared by every radio family: per-repetition cleanup
//! evidence and installed fixture software leases.
pub mod cleanup;
pub mod software;

pub use crate::lab::Error;
