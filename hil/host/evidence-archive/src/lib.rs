//! The HIL evidence archive (#761 Q7): the sealed runs the repository
//! cites, kept outside the repository in immutable versions.
//!
//! - [`manifest`]: a version's [`Manifest`], every [`RunRef`] it holds and
//!   the digest of its payload, and the version's content-addressed name;
//! - [`pin`]: the [`Pin`] the repository tracks, which names the version and
//!   repeats its manifest, so rule 2 is checked offline;
//! - [`pack`]: the deterministic payload of a version, written from a run
//!   store;
//! - [`copy`]: a verified local [`Copy`](struct@Copy) of a version, laid out as a run
//!   store;
//! - [`release`]: publication and download through [`release::Releases`],
//!   a transport the caller supplies.
//!
//! A [`Refused`] error means the inputs broke a rule of the archive; any
//! other error is the environment's (I/O, the transport, cancellation).
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod copy;
pub mod manifest;
pub mod pack;
pub mod pin;
pub mod release;

pub use copy::{Copy, run_reference};
pub use manifest::{Manifest, Payload, Version};
pub use oer_hil_model::{RunId, RunRef, Sha256};
pub use pin::{PIN, Pin, Repository};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The inputs break a rule of the archive: a digest that does not match, a
/// file the seal does not cover, a version that differs from its name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// `error` behind `context`, still a [`Refused`] error when it was one; a
/// cancellation stays one, unchanged.
pub(crate) fn within(
    error: Box<dyn std::error::Error + Send + Sync>,
    context: impl std::fmt::Display,
) -> Box<dyn std::error::Error + Send + Sync> {
    if oer_process::is_cancelled(&*error) {
        return error;
    }
    let message = format!("{context}: {error}");
    if error.is::<Refused>() {
        Box::new(Refused(message))
    } else {
        message.into()
    }
}

/// A [`Refused`] error with `message`.
pub(crate) fn refused<T>(message: impl Into<String>) -> Result<T> {
    Err(Box::new(Refused(message.into())))
}

#[cfg(test)]
mod tests;
