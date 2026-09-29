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
            incremental: Duration::from_secs(24 * 3600),
            image_caches: Duration::from_secs(3 * 24 * 3600),
        }
    }
}

impl Policy {
    /// When the build disk runs low: caches no build touched for hours.
    /// Incremental data of every feature set grows by tens of GiB per
    /// checkout and hour of active work, faster than a daily sweep.
    pub const PRESSURE: Self = Self {
        incremental: Duration::from_secs(2 * 3600),
        image_caches: Duration::from_secs(12 * 3600),
    };
}

/// How full the build disk is. A level needs both an absolute and a
/// relative shortage, so a small file system (a test's temporary root)
/// with ample room for its size is not reported as full.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Space {
    Ample,
    /// Below 200 GiB and a tenth of the disk: [`automatically`] sweeps with
    /// [`Policy::PRESSURE`] at once instead of waiting for the daily sweep.
    Low,
    /// Below 20 GiB and a fiftieth of the disk: [`ensure_space`] refuses to
    /// start a build.
    Critical,
}

impl Space {
    fn of(available: u64, total: u64) -> Self {
        if available < 20 << 30 && available < total / 50 {
            Self::Critical
        } else if available < 200 << 30 && available < total / 10 {
            Self::Low
        } else {
            Self::Ample
        }
    }

    fn measure(root: &Path) -> Result<(Self, u64)> {
        let available = fs2::available_space(root)?;
        Ok((Self::of(available, fs2::total_space(root)?), available))
    }
}

/// The sweep [`automatically`] runs: `None` when the last one is recent and
/// space is ample.
fn automatic_policy(space: Space, recent: bool) -> Option<Policy> {
    if space != Space::Ample {
        Some(Policy::PRESSURE)
    } else if recent {
        None
    } else {
        Some(Policy::default())
    }
}

/// Fail before a build when the build disk is nearly full: builds then die
/// with ENOSPC midway, often in another agent's checkout.
pub fn ensure_space(root: &Path) -> Result<()> {
    if let Some(unallocated) = btrfs_unallocated(root)
        && unallocated < 2 << 30
    {
        return Err(format!(
            "the btrfs file system holding the build disk has only {} MiB unallocated: its metadata cannot grow, so every file creation and removal stalls in the kernel; run `sudo btrfs balance start -dusage=30 /home` (the daily btrfs-balance-home timer does this)",
            unallocated >> 20
        )
        .into());
    }
    let (space, available) = Space::measure(root)?;
    if space == Space::Critical {
        return Err(format!(
            "only {:.1} GiB free on the build disk; run `cargo xtask sweep --all-checkouts --apply` or free space first",
            available as f64 / (1u64 << 30) as f64
        )
        .into());
    }
    Ok(())
}

/// Unallocated bytes of the btrfs file system holding `root`, or `None` on
/// another file system. Free space inside allocated data chunks is invisible
/// to metadata, which grows only into unallocated space; `statvfs` reports the
/// data space and misses this shortage.
fn btrfs_unallocated(root: &Path) -> Option<u64> {
    let output = std::process::Command::new("btrfs")
        .args(["filesystem", "usage", "-b"])
        .arg(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_unallocated(&String::from_utf8_lossy(&output.stdout))
}

fn parse_unallocated(usage: &str) -> Option<u64> {
    usage
        .lines()
        .find_map(|line| line.trim().strip_prefix("Device unallocated:"))
        .and_then(|value| value.trim().parse().ok())
}

/// Starts [`automatically`] as a detached process of this executable, so a
/// check never waits for a sweep of other checkouts.
pub fn in_background(root: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(std::env::current_exe()?)
        .arg("--root")
        .arg(root)
        .args(["sweep", "--automatic"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
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

/// Sweep every sibling checkout at most once a day for the whole host, and
/// at once with [`Policy::PRESSURE`] while the build disk is low, without
/// measuring sizes. Called from routine commands so that no agent has to
/// remember it; a concurrent sweep holds the marker's lock and this one
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
    let (space, _) = Space::measure(root)?;
    let Some(policy) = automatic_policy(space, recent) else {
        return Ok(());
    };
    let mut removed = 0;
    for checkout in checkouts(root) {
        for candidate in candidates(&checkout, policy, now) {
            if fs::remove_dir_all(&candidate.path).is_ok() {
                removed += 1;
            }
        }
    }
    fs::write(&marker, format!("{removed}\n"))?;
    if removed > 0 {
        eprintln!(
            "sweep: removed {removed} unused build caches across checkouts ({})",
            if space == Space::Ample {
                "daily"
            } else {
                "the build disk is low"
            }
        );
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
    fn btrfs_unallocated_space_is_read_from_its_usage_report() {
        let report = "Overall:\n    Device size:\t\t1999\n    Device unallocated:\t\t1101004\n    Used:\t\t12\n";
        assert_eq!(parse_unallocated(report), Some(1_101_004));
        assert_eq!(parse_unallocated("not btrfs"), None);
    }

    #[test]
    fn low_space_sweeps_at_once_with_the_pressure_policy() {
        const GIB: u64 = 1 << 30;
        let disk = 1900 * GIB;
        assert_eq!(Space::of(500 * GIB, disk), Space::Ample);
        assert_eq!(Space::of(150 * GIB, disk), Space::Low);
        assert_eq!(Space::of(5 * GIB, disk), Space::Critical);
        // A small file system with room for its size is not full.
        assert_eq!(Space::of(5 * GIB, 16 * GIB), Space::Ample);
        assert!(automatic_policy(Space::Ample, true).is_none());
        assert_eq!(
            automatic_policy(Space::Ample, false).map(|policy| policy.incremental),
            Some(Policy::default().incremental)
        );
        for (space, recent) in [(Space::Low, true), (Space::Critical, false)] {
            assert_eq!(
                automatic_policy(space, recent).map(|policy| policy.incremental),
                Some(Policy::PRESSURE.incremental)
            );
        }
        let root = tempfile::tempdir().unwrap();
        let hours_old = root.path().join("target/debug/incremental/crate-hours-old");
        fs::create_dir_all(&hours_old).unwrap();
        let time = SystemTime::now() - Duration::from_secs(5 * 3600);
        fs::File::open(&hours_old)
            .unwrap()
            .set_modified(time)
            .unwrap();
        assert!(candidates(root.path(), Policy::default(), SystemTime::now()).is_empty());
        assert_eq!(
            candidates(root.path(), Policy::PRESSURE, SystemTime::now()).len(),
            1
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
