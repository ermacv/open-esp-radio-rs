//! Repository operations behind `cargo xtask`: the check registry and the
//! gate that selects from it, push, CI state, locks, worktrees and sweeps,
//! and the repository checks. Domain work (images, vendor evidence,
//! registers) is its owners'; the binary parses arguments and calls them.

pub mod cargo;
pub mod checks;
pub mod ci;
pub mod ci_status;
pub mod doc;
pub mod gate;
pub mod graph;
pub mod hooks;
pub mod push;
pub mod registry;
pub mod report;
pub mod review;
pub mod worktree;

#[cfg(all(test, unix))]
mod test_support {
    use std::path::Path;

    /// Write `contents` as an executable script at `path` without this
    /// process ever holding it open for writing: a test thread that forks
    /// while another writes a script would inherit that descriptor, and
    /// executing the script then fails with `Text file busy` until the child
    /// execs. `install` writes the file in a process of its own.
    pub fn executable(path: &Path, contents: &str) {
        let staged = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(staged.path(), contents).unwrap();
        let status = std::process::Command::new("install")
            .args(["-m", "755"])
            .arg(staged.path())
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "install {}", path.display());
    }
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
