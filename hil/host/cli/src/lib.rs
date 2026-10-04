//! The `cargo hil` command line: the stand's operations (leases, boards,
//! owners, jobs, the shared run store, evidence, performance, A/B and
//! bisection, the ESP-IDF firmware catalog) and the launch of the HIL runner
//! it builds, with that runner's observer receipt.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

pub mod ab;
pub mod bisect;
pub mod board;
pub mod command;
pub mod dashboard;
pub mod evidence;
pub mod firmware_catalog;
pub mod fixtures;
pub mod flash;
pub mod jobs;
pub mod jtag;
pub mod observer;
pub mod perf;
pub mod runs;
pub mod stand;
pub mod store;
pub mod vendor_firmware;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// This package's directory, relative to the repository root.
const PACKAGE: &str = "hil/host/cli";

#[derive(Clone, Debug)]
pub struct Context {
    pub root: PathBuf,
    pub cargo: OsString,
}

impl Context {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.join("Cargo.toml").is_file() {
            return Err(format!("repository manifest missing: {}", root.display()).into());
        }
        Ok(Self {
            root,
            cargo: std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        })
    }

    /// The checkout this binary was built from. A target directory shared by
    /// two checkouts can leave one checkout's binary in the other; running it
    /// from a different checkout of this repository is refused instead of
    /// silently acting on the tree it was built from.
    pub fn discover() -> Result<Self> {
        let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        if let Some(invoked) = std::env::current_dir()
            .ok()
            .and_then(|directory| checkout_of(&directory))
            && let Ok(built) = built.canonicalize()
            && invoked != built
        {
            return Err(format!(
                "this cargo hil was built from {} but runs in {}; rebuild it there or pass --root",
                built.display(),
                invoked.display()
            )
            .into());
        }
        Self::new(built)
    }

    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(&self.root);
        command
    }

    pub fn cargo(&self) -> Command {
        self.command(&self.cargo)
    }
}

/// The top directory of the checkout of this repository containing
/// `directory`: the nearest ancestor with this package of its own.
fn checkout_of(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|candidate| candidate.join(PACKAGE).join("Cargo.toml").is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_package_directory_is_this_crate() {
        assert!(env!("CARGO_MANIFEST_DIR").ends_with(super::PACKAGE));
    }
}
