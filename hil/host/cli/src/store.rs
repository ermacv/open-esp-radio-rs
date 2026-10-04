//! The run store shared by every checkout of this user.
//!
//! HIL run bundles are self-contained and sealed, and the stand is shared by
//! all checkouts, so their runs, every chip's, live in one store; a run's
//! manifest names its chip. A checkout's `target/hil/runs` is a symbolic link
//! to it; everything else below `target/hil` (build caches, snapshots) stays
//! per checkout. Qualification
//! still decides per bundle whether it applies to the checkout's sources.
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

/// Overrides the store's root directory.
pub const STORE_ENV: &str = "OER_HIL_STORE";

/// `$OER_HIL_STORE/runs`, or the user's data directory's.
pub fn shared_runs() -> Result<PathBuf> {
    let root = match std::env::var_os(STORE_ENV).filter(|value| !value.is_empty()) {
        Some(root) => PathBuf::from(root),
        None => std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or("HOME is required to locate the shared HIL run store")?
            .join("open-esp-radio/hil"),
    };
    Ok(root.join("runs"))
}

#[derive(Debug, PartialEq)]
pub enum Linked {
    /// The checkout already uses a link.
    Existing,
    /// A new link, in place of no directory or an empty one.
    Created,
}

/// Make the checkout's run directory a link to the shared store. A run
/// directory of its own that holds runs is refused: every checkout has used
/// the store since it was introduced, so such runs were written past it.
pub fn link_runs(local: &Path, shared: &Path) -> Result<Linked> {
    fs::create_dir_all(shared)?;
    match fs::symlink_metadata(local) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Ok(Linked::Existing),
        Ok(metadata) if metadata.is_dir() => {
            if fs::read_dir(local)?.next().is_some() {
                return Err(format!(
                    "{} holds runs outside the shared store {}; move them there",
                    local.display(),
                    shared.display()
                )
                .into());
            }
            fs::remove_dir(local)?;
        }
        Ok(_) => return Err(format!("{} is not a directory", local.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(local.parent().ok_or("run directory has no parent")?)?;
        }
        Err(error) => return Err(error.into()),
    }
    std::os::unix::fs::symlink(shared, local)?;
    Ok(Linked::Created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_links_to_the_store_and_refuses_runs_of_its_own() {
        let directory = tempfile::tempdir().unwrap();
        let shared = directory.path().join("store/esp32s31/runs");
        let first = directory.path().join("first/target/hil/runs");
        assert_eq!(link_runs(&first, &shared).unwrap(), Linked::Created);
        assert_eq!(link_runs(&first, &shared).unwrap(), Linked::Existing);

        let empty = directory.path().join("empty/target/hil/runs");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(link_runs(&empty, &shared).unwrap(), Linked::Created);

        let own = directory.path().join("own/target/hil/runs");
        fs::create_dir_all(own.join("a-run")).unwrap();
        assert!(link_runs(&own, &shared).is_err());
        assert!(own.join("a-run").is_dir());
    }
}
