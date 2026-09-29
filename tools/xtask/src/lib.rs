//! Repository orchestration; policies remain separate from Cargo/process mechanics.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

pub mod blobray;
pub mod cargo;
pub mod checks;
pub mod chips;
pub mod ci_status;
pub mod compare_images;
pub mod doc;
pub mod evidence;
pub mod evidence_diff;
pub mod firmware;
pub mod firmware_catalog;
pub mod graph;
pub mod hil;
pub mod hil_ab;
pub mod hil_bisect;
pub mod hil_board;
pub mod hil_dashboard;
pub mod hil_evidence;
pub mod hil_fixtures;
pub mod hil_flash;
pub mod hil_jobs;
pub mod hil_jtag;
pub mod hil_perf;
pub mod hil_runs;
pub mod hil_store;
pub mod paths;
pub mod phase;
pub mod process;
pub mod push;
pub mod register_inventory;
pub mod source_citation;
pub mod stand_install;
pub mod sweep;
pub mod vendor_diff;
pub mod vendor_fetch;
pub mod vendor_fingerprint;
pub mod vendor_firmware;
pub mod vendor_provenance;
pub mod vendor_scenario;
pub mod worktree;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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
        let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if let Some(invoked) = std::env::current_dir()
            .ok()
            .and_then(|directory| checkout_of(&directory))
            && let Ok(built) = built.canonicalize()
            && invoked != built
        {
            return Err(format!(
                "this xtask was built from {} but runs in {}; rebuild it there or pass --root",
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
/// `directory`: the nearest ancestor with an xtask package of its own.
fn checkout_of(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|candidate| candidate.join("tools/xtask/Cargo.toml").is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}
