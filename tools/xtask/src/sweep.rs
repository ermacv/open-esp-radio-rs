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

/// How old unused caches must be before they are removed.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub incremental: Duration,
    pub image_caches: Duration,
    /// A checkout with no Git activity or build for this long loses its
    /// whole `target/`.
    pub idle_checkout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            incremental: Duration::from_secs(24 * 3600),
            image_caches: Duration::from_secs(3 * 24 * 3600),
            idle_checkout: Duration::from_secs(7 * 24 * 3600),
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
        idle_checkout: Duration::from_secs(2 * 24 * 3600),
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
    if let Some(idle) = idle_for(root, now)
        && idle > policy.idle_checkout
        && target.is_dir()
        && !build_running(&target)
    {
        found.push(Candidate {
            path: target,
            reason: format!("checkout idle for {} days", idle.as_secs() / 86_400),
        });
        return found;
    }
    incremental(&target, policy, now, &mut found);
    image_caches(&target, policy, now, &mut found);
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// Sibling checkouts of this repository: directories next to `root` whose
/// name starts with its name up to the first `-` suffix and that contain
/// this repository's xtask.
/// How long ago the checkout at `root` last changed its Git state (index,
/// HEAD or its reflog) or built anything, when that can be read.
fn idle_for(root: &Path, now: SystemTime) -> Option<Duration> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--absolute-git-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let git = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    [
        git.join("index"),
        git.join("HEAD"),
        git.join("logs/HEAD"),
        root.join("target/.rustc_info.json"),
    ]
    .iter()
    .filter_map(|path| age(path, now))
    .min()
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

/// Every checkout of this repository on the host: the worktrees Git knows
/// and the sibling clones named after the repository.
pub fn checkouts(root: &Path) -> Vec<PathBuf> {
    let mut found = siblings(root);
    if let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        found.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.strip_prefix("worktree "))
                .map(PathBuf::from)
                .filter(|path| path.is_dir()),
        );
    }
    found.sort();
    found.dedup();
    found
}

fn siblings(root: &Path) -> Vec<PathBuf> {
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
    let held = held_by_jobs()?;
    let mut removed = 0;
    for checkout in checkouts(root) {
        for candidate in unheld(candidates(&checkout, policy, now), &held) {
            if fs::remove_dir_all(&candidate.path).is_ok() {
                removed += 1;
            }
        }
    }
    for candidate in unheld(host_candidates(policy, now), &held) {
        if fs::remove_dir_all(&candidate.path).is_ok() {
            removed += 1;
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
    let held = held_by_jobs()?;
    let mut total = 0;
    for root in roots {
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
        total += bytes;
    }
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
    fn an_idle_checkout_loses_its_whole_target_and_an_active_one_only_old_caches() {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(checkout)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        fs::create_dir_all(checkout.join("target/debug/deps")).unwrap();
        let now = SystemTime::now();
        assert!(
            candidates(checkout, Policy::default(), now).is_empty(),
            "a checkout just initialised is active"
        );
        let later = now + Duration::from_secs(8 * 24 * 3600);
        let found = candidates(checkout, Policy::default(), later);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, checkout.join("target"));
        assert!(found[0].reason.starts_with("checkout idle for 8 days"));
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
