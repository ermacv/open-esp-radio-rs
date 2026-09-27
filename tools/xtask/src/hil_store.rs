//! The run store shared by every checkout of this user.
//!
//! HIL run bundles are self-contained and sealed, and the stand is shared by
//! all checkouts, so their runs live in one store. A checkout's
//! `target/hil/<target>/runs` is a symbolic link to it; everything else below
//! `target/hil` (build caches, snapshots) stays per checkout. Qualification
//! still decides per bundle whether it applies to the checkout's sources.
//!
//! A checkout that still has its own run directory is migrated on its next
//! `cargo hil` command: every file is hard-linked into the store (copied when
//! linking fails), the directory is kept as `runs.before-shared-store`, and
//! the link replaces it. A run in progress defers the migration.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::Result;

/// Overrides the store's root directory.
pub const STORE_ENV: &str = "OER_HIL_STORE";
/// A run whose manifest says `running` and changed this recently is active.
const ACTIVE_WINDOW: Duration = Duration::from_secs(6 * 3600);

/// `$OER_HIL_STORE/<target>/runs`, or the user's data directory.
pub fn shared_runs(target: &str) -> Result<PathBuf> {
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
    Ok(root.join(target).join("runs"))
}

#[derive(Debug, PartialEq)]
pub enum Linked {
    /// The checkout already uses a link.
    Existing,
    /// A new link; the checkout had no runs.
    Created,
    /// Runs were moved into the store, and the old directory kept.
    Migrated { runs: usize, kept: PathBuf },
    /// A run in progress keeps the checkout's own directory for now.
    Deferred { active: String },
}

/// Make the checkout's run directory a link to the shared store.
pub fn link_runs(local: &Path, shared: &Path) -> Result<Linked> {
    fs::create_dir_all(shared)?;
    match fs::symlink_metadata(local) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Ok(Linked::Existing),
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(format!("{} is not a directory", local.display()).into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(local.parent().ok_or("run directory has no parent")?)?;
            std::os::unix::fs::symlink(shared, local)?;
            return Ok(Linked::Created);
        }
        Err(error) => return Err(error.into()),
    }
    if let Some(active) = active_run(local)? {
        return Ok(Linked::Deferred { active });
    }
    let mut runs = 0;
    for entry in fs::read_dir(local)? {
        let entry = entry?;
        merge(&entry.path(), &shared.join(entry.file_name()))?;
        runs += 1;
    }
    // The observer builds those runs name live beside them.
    let observers = |runs: &Path| {
        runs.parent()
            .map(|parent| parent.join(oer_hil_schema::observer_store::DIRECTORY))
    };
    if let (Some(from), Some(to)) = (observers(local), observers(shared))
        && from.is_dir()
    {
        fs::create_dir_all(&to)?;
        for entry in fs::read_dir(&from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            // Builds are named by their digest: an existing one is the same.
            if !target.exists() {
                fs::copy(entry.path(), target)?;
            }
        }
    }
    let mut kept = local.with_file_name("runs.before-shared-store");
    if kept.exists() {
        kept = local.with_file_name(format!(
            "runs.before-shared-store.{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)?
                .as_secs()
        ));
    }
    fs::rename(local, &kept)?;
    std::os::unix::fs::symlink(shared, local)?;
    Ok(Linked::Migrated { runs, kept })
}

/// A run whose manifest still says `running` and changed recently.
fn active_run(runs: &Path) -> Result<Option<String>> {
    for entry in fs::read_dir(runs)? {
        let entry = entry?;
        let manifest = entry.path().join("manifest.json");
        let Ok(bytes) = fs::read(&manifest) else {
            continue;
        };
        let running = serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|value| value["state"] == "running");
        let recent = fs::metadata(entry.path().join("events.jsonl"))
            .or_else(|_| fs::metadata(&manifest))
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < ACTIVE_WINDOW);
        if running && recent {
            return Ok(Some(entry.file_name().to_string_lossy().into_owned()));
        }
    }
    Ok(None)
}

/// Hard-link `source` into `destination`, recursively; existing destination
/// files are kept, since sealed files never change.
fn merge(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        if fs::symlink_metadata(destination).is_err() {
            std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
        }
    } else if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            merge(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if fs::symlink_metadata(destination).is_err()
        && fs::hard_link(source, destination).is_err()
    {
        fs::copy(source, destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(runs: &Path, id: &str, state: &str) {
        let directory = runs.join(id);
        fs::create_dir_all(directory.join("scenarios/x")).unwrap();
        fs::write(
            directory.join("manifest.json"),
            format!(r#"{{"state":"{state}"}}"#),
        )
        .unwrap();
        fs::write(directory.join("events.jsonl"), "{}\n").unwrap();
        fs::write(directory.join("scenarios/x/result.json"), id).unwrap();
    }

    #[test]
    fn a_checkout_links_to_the_store_and_migrates_its_runs_once() {
        let directory = tempfile::tempdir().unwrap();
        let shared = directory.path().join("store/esp32s31/runs");
        let first = directory.path().join("first/target/hil/esp32s31/runs");
        assert_eq!(link_runs(&first, &shared).unwrap(), Linked::Created);
        assert_eq!(link_runs(&first, &shared).unwrap(), Linked::Existing);
        run(&shared, "from-first", "completed");

        let second = directory.path().join("second/target/hil/esp32s31/runs");
        run(&second, "from-second", "completed");
        let Linked::Migrated { runs, kept } = link_runs(&second, &shared).unwrap() else {
            panic!("the second checkout migrates");
        };
        assert_eq!(runs, 1);
        assert!(kept.join("from-second/manifest.json").is_file());
        for checkout in [&first, &second] {
            assert!(checkout.join("from-first/manifest.json").is_file());
            assert_eq!(
                fs::read_to_string(checkout.join("from-second/scenarios/x/result.json")).unwrap(),
                "from-second"
            );
        }
        use std::os::unix::fs::MetadataExt as _;
        assert_eq!(
            fs::metadata(kept.join("from-second/events.jsonl"))
                .unwrap()
                .ino(),
            fs::metadata(shared.join("from-second/events.jsonl"))
                .unwrap()
                .ino(),
            "migration links rather than copies"
        );
    }

    #[test]
    fn a_run_in_progress_defers_the_migration() {
        let directory = tempfile::tempdir().unwrap();
        let shared = directory.path().join("store/runs");
        let local = directory.path().join("checkout/runs");
        run(&local, "busy", "running");
        assert_eq!(
            link_runs(&local, &shared).unwrap(),
            Linked::Deferred {
                active: "busy".into()
            }
        );
        assert!(
            !fs::symlink_metadata(&local)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
