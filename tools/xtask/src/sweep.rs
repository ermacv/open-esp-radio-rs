//! Remove rebuildable build caches that no build has used recently, when
//! asked (`cargo xtask sweep`); nothing sweeps implicitly.
//!
//! Incremental session data of every feature set ever built and per-image
//! HIL build caches accumulate in a checkout's `target/` until the disk
//! fills. Only caches Cargo recreates are removed, only in this checkout and
//! in the host's shared HIL snapshot builds, never run bundles, evidence or
//! archives, never a directory whose Cargo build lock is held and never what
//! a queued HIL job was fixed with. Another checkout's `target/` is its
//! owner's to sweep.
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use crate::Result;

/// How old unused caches must be before they are removed.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub incremental: Duration,
    pub image_caches: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            incremental: Duration::from_secs(24 * 3600),
            image_caches: Duration::from_secs(3 * 24 * 3600),
        }
    }
}

/// One removable cache directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub reason: String,
}

/// The files that queued or running HIL jobs were fixed with.
fn held_by_jobs() -> Result<Vec<PathBuf>> {
    Ok(crate::hil_jobs::Jobs::open()?
        .unfinished()
        .into_iter()
        .flat_map(|job| job.fixed)
        .collect())
}

/// `found` without the candidates that hold or lie inside a `held` path.
fn unheld(found: Vec<Candidate>, held: &[PathBuf]) -> Vec<Candidate> {
    found
        .into_iter()
        .filter(|candidate| {
            !held
                .iter()
                .any(|path| path.starts_with(&candidate.path) || candidate.path.starts_with(path))
        })
        .collect()
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

/// Per-image HIL build caches unused for the policy's age and not building.
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
            if age(&path, now).is_some_and(|age| age > policy.image_caches) {
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

/// The host build root's snapshot builds that no build used for longer than
/// `policy` allows.
pub fn host_candidates(policy: Policy, now: SystemTime) -> Vec<Candidate> {
    let Ok(root) = oer_hil_image::host_build_root() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    // Source snapshots are kept: a queued job can wait for days on the one
    // it was frozen with.
    let mut groups = Vec::new();
    for chip in fs::read_dir(&root).into_iter().flatten().flatten() {
        groups.push((
            chip.path().join("snapshot-builds"),
            policy.image_caches,
            "snapshot build unused",
        ));
    }
    for (group, limit, reason) in groups {
        for entry in fs::read_dir(&group).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() && age(&path, now).is_some_and(|age| age > limit) {
                found.push(Candidate {
                    path,
                    reason: reason.into(),
                });
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
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

/// List, or with `apply` remove, the caches of the checkout at `root` and
/// the host's unused snapshot builds; returns the bytes that were (or would
/// be) freed.
pub fn run(root: &Path, policy: Policy, apply: bool) -> Result<u64> {
    let now = SystemTime::now();
    let held = held_by_jobs()?;
    let total = {
        let found = unheld(candidates(root, policy, now), &held);
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
        bytes
    };
    let found = unheld(host_candidates(policy, now), &held);
    let mut bytes = 0;
    for candidate in &found {
        bytes += size(&candidate.path);
        if apply {
            if let Err(error) = fs::remove_dir_all(&candidate.path) {
                eprintln!("sweep: cannot remove {}: {error}", candidate.path.display());
            }
        } else if found.len() <= 20 {
            println!("  {} ({})", candidate.path.display(), candidate.reason);
        }
    }
    println!(
        "{} the host build root: {} entries, {:.1} GiB",
        if apply {
            "removed from"
        } else {
            "would remove from"
        },
        found.len(),
        bytes as f64 / (1u64 << 30) as f64
    );
    Ok(total + bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aged(path: &Path, days: u64) {
        let time = SystemTime::now() - Duration::from_secs(days * 24 * 3600);
        fs::File::open(path).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn only_old_caches_are_candidates_and_bundles_are_kept() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("target");
        let old = target.join("debug/incremental/crate-old");
        let fresh = target.join("debug/incremental/crate-fresh");
        let stale = target.join("hil/esp32s31/psram-code-correctness-owned-xarxa");
        let live = target.join("hil/esp32s31/psram-code-performance-owned-xarxa");
        let runs = target.join("hil/esp32s31/runs.before-shared-store");
        for directory in [&old, &fresh, &stale, &live, &runs] {
            fs::create_dir_all(directory).unwrap();
        }
        aged(&old, 10);
        aged(&stale, 10);
        aged(&runs, 30);
        let found = candidates(root.path(), Policy::default(), SystemTime::now());
        let paths = found.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths, [old, stale]);
    }

    #[test]
    fn files_a_queued_job_was_fixed_with_are_kept_with_what_holds_them() {
        let candidate = |path: &str| Candidate {
            path: PathBuf::from(path),
            reason: String::from("idle"),
        };
        let held = [PathBuf::from("/c/target/hil/jobs/xtask/ab/oer-xtask")];
        assert_eq!(
            unheld(
                vec![
                    candidate("/c/target"),
                    candidate("/c/target/hil/jobs/xtask/ab/oer-xtask"),
                    candidate("/c/target/debug/incremental/x"),
                    candidate("/d/target"),
                ],
                &held,
            ),
            [
                candidate("/c/target/debug/incremental/x"),
                candidate("/d/target")
            ]
        );
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
