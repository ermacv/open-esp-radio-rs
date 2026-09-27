//! Remove rebuildable build caches that no build has used recently.
//!
//! Every checkout keeps its own `target/`; incremental session data of every
//! feature set ever built and per-image HIL build caches accumulate until the
//! shared disk fills. Only caches Cargo recreates are removed, never run
//! bundles, evidence or archives, and never a directory whose Cargo build
//! lock is currently held.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::Result;

/// Network implementations whose HIL image caches can never be reused.
const REMOVED_NETWORKS: &[&str] = &["upstream-xarxa", "patched-xarxa", "upstream-smoltcp"];

/// How old unused caches must be before they are removed.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub incremental: Duration,
    pub image_caches: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            incremental: Duration::from_secs(3 * 24 * 3600),
            image_caches: Duration::from_secs(7 * 24 * 3600),
        }
    }
}

/// One removable cache directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub reason: String,
}

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    now.duration_since(modified).ok()
}

/// Whether a Cargo build currently holds a lock inside `directory`, within
/// three levels (`<dir>/<triple>/<profile>/.cargo-lock`).
fn build_running(directory: &Path) -> bool {
    fn visit(directory: &Path, depth: usize) -> bool {
        let lock = directory.join(".cargo-lock");
        if lock.is_file()
            && let Ok(file) = fs::File::open(&lock)
            && fs2::FileExt::try_lock_shared(&file).is_err()
        {
            return true;
        }
        depth > 0
            && fs::read_dir(directory)
                .into_iter()
                .flatten()
                .flatten()
                .any(|entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_dir())
                        && visit(&entry.path(), depth - 1)
                })
    }
    visit(directory, 3)
}

/// Incremental crate directories older than the policy, below every
/// `incremental` directory of `target` up to four levels deep, skipping a
/// profile whose build lock is held.
fn incremental(target: &Path, policy: Policy, now: SystemTime, found: &mut Vec<Candidate>) {
    fn visit(
        directory: &Path,
        depth: usize,
        policy: Policy,
        now: SystemTime,
        found: &mut Vec<Candidate>,
    ) {
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            if entry.file_name() == "incremental" {
                if build_running(directory) {
                    continue;
                }
                for crate_directory in fs::read_dir(&path).into_iter().flatten().flatten() {
                    let crate_path = crate_directory.path();
                    if crate_path.is_dir()
                        && age(&crate_path, now).is_some_and(|age| age > policy.incremental)
                    {
                        found.push(Candidate {
                            path: crate_path,
                            reason: "incremental data unused".into(),
                        });
                    }
                }
            } else if depth > 0 {
                visit(&path, depth - 1, policy, now, found);
            }
        }
    }
    visit(target, 4, policy, now, found);
}

/// Per-image HIL build caches: always when built for a removed network,
/// otherwise when unused for the policy's age and not building.
fn image_caches(target: &Path, policy: Policy, now: SystemTime, found: &mut Vec<Candidate>) {
    let chip = target.join("hil/esp32s31");
    let groups = [
        chip.clone(),
        chip.join("build-cache"),
        chip.join("snapshot-builds"),
    ];
    for group in groups {
        for entry in fs::read_dir(&group).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || build_running(&path) {
                continue;
            }
            let image_cache = name.starts_with("psram-") || group != chip;
            if !image_cache {
                continue;
            }
            if let Some(network) = REMOVED_NETWORKS
                .iter()
                .find(|network| name.ends_with(*network))
            {
                found.push(Candidate {
                    path,
                    reason: format!("image cache for the removed {network} network"),
                });
            } else if age(&path, now).is_some_and(|age| age > policy.image_caches) {
                found.push(Candidate {
                    path,
                    reason: "image cache unused".into(),
                });
            }
        }
    }
}

/// Every removable cache of the checkout at `root`.
pub fn candidates(root: &Path, policy: Policy, now: SystemTime) -> Vec<Candidate> {
    let target = root.join("target");
    let mut found = Vec::new();
    incremental(&target, policy, now, &mut found);
    image_caches(&target, policy, now, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Sibling checkouts of this repository: directories next to `root` whose
/// name starts with its name up to the first `-` suffix and that contain
/// this repository's xtask.
pub fn checkouts(root: &Path) -> Vec<PathBuf> {
    let Some(parent) = root.parent() else {
        return vec![root.to_owned()];
    };
    let stem = "open-esp-radio-rs";
    let mut found = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(stem))
                && path.join("tools/xtask/Cargo.toml").is_file()
        })
        .collect::<Vec<_>>();
    found.sort();
    found
}

fn size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_dir() {
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| size(&entry.path()))
            .sum()
    } else {
        metadata.len()
    }
}

/// How often [`automatically`] sweeps every checkout.
const AUTOMATIC_INTERVAL: Duration = Duration::from_secs(24 * 3600);

/// Sweep every sibling checkout at most once a day for the whole host,
/// without measuring sizes. Called from routine commands so that no agent has
/// to remember it; a concurrent sweep holds the marker's lock and this one
/// returns.
pub fn automatically(root: &Path) -> Result<()> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or("HOME is required to locate the sweep marker")?
        .join("open-esp-radio");
    fs::create_dir_all(&base)?;
    let marker = base.join("last-sweep");
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&marker)?;
    if fs2::FileExt::try_lock_exclusive(&file).is_err() {
        return Ok(());
    }
    let now = SystemTime::now();
    let recent = fs::metadata(&marker)?
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| {
            age < AUTOMATIC_INTERVAL && fs::metadata(&marker).is_ok_and(|m| m.len() > 0)
        });
    if recent {
        return Ok(());
    }
    let mut removed = 0;
    for checkout in checkouts(root) {
        for candidate in candidates(&checkout, Policy::default(), now) {
            if fs::remove_dir_all(&candidate.path).is_ok() {
                removed += 1;
            }
        }
    }
    fs::write(&marker, format!("{removed}\n"))?;
    if removed > 0 {
        eprintln!("sweep: removed {removed} unused build caches across checkouts (daily)");
    }
    Ok(())
}

/// List, or with `apply` remove, the caches of `roots`; returns the bytes
/// that were (or would be) freed.
pub fn run(roots: &[PathBuf], policy: Policy, apply: bool) -> Result<u64> {
    let now = SystemTime::now();
    let mut total = 0;
    for root in roots {
        let found = candidates(root, policy, now);
        let mut bytes = 0;
        for candidate in &found {
            let length = size(&candidate.path);
            bytes += length;
            if apply {
                if let Err(error) = fs::remove_dir_all(&candidate.path) {
                    eprintln!("sweep: cannot remove {}: {error}", candidate.path.display());
                    continue;
                }
            } else if found.len() <= 20 {
                println!("  {} ({})", candidate.path.display(), candidate.reason);
            }
        }
        println!(
            "{} {}: {} caches, {:.1} GiB",
            if apply { "removed" } else { "would remove" },
            root.display(),
            found.len(),
            bytes as f64 / (1u64 << 30) as f64
        );
        total += bytes;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(path: &Path, days: u64) {
        let time = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
        fs::File::open(path).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn only_old_or_dead_caches_are_candidates_and_bundles_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let old = target.join("debug/incremental/crate-old");
        let fresh = target.join("debug/incremental/crate-fresh");
        let dead = target.join("hil/esp32s31/psram-code-performance-upstream-xarxa");
        let live = target.join("hil/esp32s31/psram-code-performance-owned-xarxa");
        let runs = target.join("hil/esp32s31/runs.before-shared-store");
        for directory in [&old, &fresh, &dead, &live, &runs] {
            fs::create_dir_all(directory).unwrap();
        }
        aged(&old, 10);
        aged(&runs, 30);
        let found = candidates(root.path(), Policy::default(), SystemTime::now());
        let paths = found.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths, [old, dead]);
    }

    #[test]
    fn a_held_build_lock_protects_its_caches() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("target/debug");
        let old = profile.join("incremental/crate-old");
        fs::create_dir_all(&old).unwrap();
        aged(&old, 10);
        let lock = fs::File::create(profile.join(".cargo-lock")).unwrap();
        fs2::FileExt::lock_exclusive(&lock).unwrap();
        assert!(candidates(root.path(), Policy::default(), SystemTime::now()).is_empty());
        fs2::FileExt::unlock(&lock).unwrap();
        assert_eq!(
            candidates(root.path(), Policy::default(), SystemTime::now()).len(),
            1
        );
    }
}
