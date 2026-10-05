//! The flash operation, the only way the host writes a board's flash:
//!
//! 1. **lease**: the caller holds the board's lock ([`BoardLock`]), which it
//!    takes only under the arbiter's grant; the operation refuses a lock of
//!    another board;
//! 2. **write**: board I/O writes the image bundle's segments, the OTA
//!    selection last, retrying a failed link and skipping unchanged
//!    segments;
//! 3. **journal**: the board journal records the image the board now
//!    carries ([`oer_hil_arbiter::Arbiter::record_flash`]), with the digest
//!    of the bundle's application, computed here;
//! 4. **start**: the image starts as the chip profile's `[flash] start`
//!    says, unless the write's own reset started it.
//!
//! `cargo hil flash`, `cargo hil firmware flash`, the runner's flash of the
//! device under test and of the reference peers, and the calibration
//! capture call [`flash`] or [`flash_if_changed`]; [`catalog`] flashes an
//! ESP-IDF catalog image.
#![forbid(unsafe_code)]

use std::path::Path;

use oer_chip_profile::Start;
use oer_hil_arbiter::{Arbiter, ImageIdentity, lock::BoardLock};
use oer_hil_board::Via;
use oer_image::ImageBundle;

pub mod catalog;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A board the operation writes: board I/O's [`oer_hil_board::Board`], or a
/// test's fake.
pub trait Target {
    /// The board's MAC.
    fn mac(&self) -> &str;
    /// Write `bundle` through `via`; the start the image still needs.
    fn write(&self, bundle: &ImageBundle, via: Via) -> Result<Start>;
    /// Start an image a write left for `start`.
    fn start(&self, start: Start) -> Result<()>;
}

impl Target for oer_hil_board::Board {
    fn mac(&self) -> &str {
        oer_hil_board::Board::mac(self)
    }

    fn write(&self, bundle: &ImageBundle, via: Via) -> Result<Start> {
        oer_hil_board::Board::write(self, bundle, via)
    }

    fn start(&self, start: Start) -> Result<()> {
        oer_hil_board::Board::start(self, start)
    }
}

/// The source an image was built from: its commit and whether the tree
/// differed from it, when known.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Revision {
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

impl Revision {
    /// The checkout at `root`'s commit and whether `directory` differs from
    /// it.
    pub fn of_checkout(root: &Path, directory: &Path) -> Self {
        let git = |args: &[&str]| oer_process::git::text(root, args).ok();
        Self {
            commit: git(&["rev-parse", "HEAD"]),
            dirty: git(&["status", "--porcelain", "--", &directory.to_string_lossy()])
                .map(|status| !status.is_empty()),
        }
    }
}

/// What the operation writes and journals.
pub struct Image<'a> {
    pub bundle: &'a ImageBundle,
    /// The name the journal knows the image by.
    pub name: &'a str,
    pub revision: Revision,
    /// The run or command that writes it.
    pub origin: String,
}

impl Image<'_> {
    fn identity(&self) -> Result<ImageIdentity> {
        Ok(ImageIdentity {
            name: self.name.to_owned(),
            sha256: oer_durable::sha256_file(&self.bundle.application())?,
            commit: self.revision.commit.clone(),
            dirty: self.revision.dirty,
            origin: self.origin.clone(),
        })
    }
}

/// What [`flash_if_changed`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Flashed {
    Written,
    /// The journal says the board already carries the image; nothing was
    /// written.
    Carried,
}

/// Write `image` into `target` under `lock`, journal it for `owner` in
/// `journal`, then start it.
pub fn flash(
    journal: &Arbiter,
    owner: &str,
    lock: &BoardLock,
    target: &dyn Target,
    image: &Image<'_>,
    via: Via,
) -> Result<()> {
    if lock.mac() != target.mac() {
        return Err(format!(
            "the lock of board {} does not cover board {}",
            lock.mac(),
            target.mac()
        )
        .into());
    }
    let identity = image.identity()?;
    let start = target.write(image.bundle, via)?;
    journal.record_flash(owner.to_owned(), target.mac(), &identity)?;
    target.start(start)
}

/// [`flash`], unless the journal says the board already carries the image
/// with this digest.
pub fn flash_if_changed(
    journal: &Arbiter,
    owner: &str,
    lock: &BoardLock,
    target: &dyn Target,
    image: &Image<'_>,
    via: Via,
) -> Result<Flashed> {
    let sha256 = oer_durable::sha256_file(&image.bundle.application())?;
    if journal.carries(target.mac(), image.name, &sha256)? {
        return Ok(Flashed::Carried);
    }
    flash(journal, owner, lock, target, image, via)?;
    Ok(Flashed::Written)
}

#[cfg(test)]
mod tests;
