//! Remove rebuildable build caches that no build has used recently, when
//! asked (`cargo hil sweep`); nothing sweeps implicitly. It belongs to
//! HIL: it keeps what queued HIL jobs were fixed with and knows the image
//! classes' build outputs.
//!
//! Incremental session data of every feature set ever built and per-image
//! HIL build caches accumulate in a checkout's `target/` until the disk
//! fills. Only caches a build recreates are removed: in this checkout's
//! `target/`, and in the host build root the incremental data of this
//! checkout's compile cache and what no checkout owns (below);
//! never run bundles, evidence or archives, never a directory whose Cargo build lock is held and never what
//! a queued HIL job was fixed with. Another checkout's `target/` is its
//! owner's to sweep.
//!
//! The host build root holds what no checkout owns once its tree is gone:
//! the firmware compile cache of a tree that no longer exists (a removed
//! checkout, the base worktree of `cargo hil images compare`) and an installed image
//! linker no build has used for the policy's age, which the next image build
//! installs again. A sweep from any checkout removes those, and the stale
//! incremental data of its own compile cache's linker build, never a compile
//! cache a build holds.
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

/// The files that queued or running HIL jobs were fixed with, and the
/// source snapshot each names with `--source-snapshot`, which it reads when
/// it starts.
pub(crate) fn held_by_jobs() -> Result<Vec<PathBuf>> {
    Ok(oer_stand_arbiter::Arbiter::open()?
        .jobs()
        .unfinished()
        .into_iter()
        .flat_map(|job| {
            let snapshot = named_snapshot(&job.command).map(|path| job.checkout.join(path));
            job.fixed.into_iter().chain(snapshot)
        })
        .collect())
}

/// The value of `--source-snapshot` in the `cargo hil` arguments `command`.
fn named_snapshot(command: &[String]) -> Option<&str> {
    let mut arguments = command.iter();
    while let Some(argument) = arguments.next() {
        if argument == "--source-snapshot" {
            return arguments.next().map(String::as_str);
        }
        if let Some(value) = argument.strip_prefix("--source-snapshot=") {
            return Some(value);
        }
    }
    None
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
            && !matches!(
                oer_process::lock::FileLock::try_acquire(&lock, oer_process::lock::Mode::Shared),
                Ok(Some(_))
            )
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

/// Whether `name`, a directory of `target/hil/<chip>`, is an image class's
/// build output (named after a class's runtime profile) or type-check
/// output (`check-<class>-<network>`).
fn class_output(name: &str) -> bool {
    name.starts_with("check-")
        || oer_hil_schema::image::ImageClass::ALL
            .iter()
            .any(|class| name.starts_with(&format!("{}-", class.runtime_profile())))
}

/// Per-image HIL build outputs of every chip in `chips`, unused for the
/// policy's age and not building: the class outputs of `target/hil/<chip>`.
/// The checkout's compile cache is below the host build root
/// ([`firmware_caches`]).
fn image_caches(
    target: &Path,
    chips: &[String],
    policy: Policy,
    now: SystemTime,
    found: &mut Vec<Candidate>,
) {
    for chip in chips {
        let directory = target.join("hil").join(chip);
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || build_running(&path) || !class_output(&name) {
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

/// Whether a build holds the firmware compile cache `cache`.
fn cache_held(cache: &Path) -> bool {
    !matches!(
        oer_process::lock::FileLock::try_acquire(
            &cache.join("build.lock"),
            oer_process::lock::Mode::Shared
        ),
        Ok(Some(_))
    )
}

/// The firmware compile caches below `build_root` whose tree is gone, and
/// the incremental data of the linker build in the cache of the tree at
/// `root`; never a cache a build holds.
fn firmware_caches(
    build_root: &Path,
    root: &Path,
    policy: Policy,
    now: SystemTime,
    found: &mut Vec<Candidate>,
) {
    let own = fs::canonicalize(root).ok();
    for recorded in oer_toolchain::image::compile_caches(build_root).unwrap_or_default() {
        if cache_held(&recorded.cache) {
            continue;
        }
        if recorded.is_gone() {
            found.push(Candidate {
                path: recorded.cache,
                reason: "compile cache of a tree that is gone".into(),
            });
        } else if recorded.tree == own {
            incremental(&recorded.cache, policy, now, found);
        }
    }
}

/// The installed image linkers below `build_root` no build has used for the
/// policy's age; each use renews its file's modification time.
fn linkers(build_root: &Path, policy: Policy, now: SystemTime, found: &mut Vec<Candidate>) {
    let directory = build_root.join(oer_toolchain::image::LINKERS);
    for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
        // A removal moves a linker aside under a dot name first; an
        // interrupted one is `interrupted_removals`'.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && linker_unused(&path, policy, now) {
            found.push(Candidate {
                path,
                reason: "image linker unused".into(),
            });
        }
    }
}

/// Whether the installed linker in `directory` was unused for the policy's
/// age: its file's modification time is its last use.
fn linker_unused(directory: &Path, policy: Policy, now: SystemTime) -> bool {
    let used = directory.join("oer-image-linker");
    let age = if used.is_file() {
        age(&used, now)
    } else {
        age(directory, now)
    };
    age.is_some_and(|age| age > policy.image_caches)
}

/// Directories a removal moved aside (`.removing-*`) and left behind when it
/// was interrupted, below the compile caches and the linkers of `build_root`:
/// not one whose remover (the process its name ends in) still runs, nor one
/// moved aside less than a minute ago.
fn interrupted_removals(build_root: &Path, now: SystemTime, found: &mut Vec<Candidate>) {
    for store in [
        oer_toolchain::image::COMPILE_CACHES,
        oer_toolchain::image::LINKERS,
    ] {
        for entry in fs::read_dir(build_root.join(store))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(".removing-") else {
                continue;
            };
            let running = rest
                .rsplit_once('-')
                .and_then(|(_, pid)| pid.parse::<u32>().ok())
                .is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists());
            let recent = age(&entry.path(), now).is_none_or(|age| age < Duration::from_secs(60));
            if !running && !recent {
                found.push(Candidate {
                    path: entry.path(),
                    reason: "interrupted removal".into(),
                });
            }
        }
    }
}

/// Every removable cache of the checkout at `root` whose chips are `chips`,
/// and the host build root's at `build_root`.
pub fn candidates(
    root: &Path,
    build_root: &Path,
    chips: &[String],
    policy: Policy,
    now: SystemTime,
) -> Vec<Candidate> {
    let target = root.join("target");
    let mut found = Vec::new();
    incremental(&target, policy, now, &mut found);
    image_caches(&target, chips, policy, now, &mut found);
    firmware_caches(build_root, root, policy, now, &mut found);
    linkers(build_root, policy, now, &mut found);
    interrupted_removals(build_root, now, &mut found);
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

/// Remove `candidate`, deciding again at removal what the listing decided;
/// `Ok(false)` keeps it.
///
/// - A firmware compile cache below `build_root` goes only under its build
///   lock and while its tree is still gone.
/// - An installed linker is moved aside first and its age read again there:
///   a build that took it meanwhile renewed it, so it goes back, unless a
///   build installed it anew in the meantime.
fn remove(candidate: &Candidate, build_root: &Path, policy: Policy) -> Result<bool> {
    let name = candidate
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = candidate.path.parent();
    if name.starts_with(".removing-") {
        fs::remove_dir_all(&candidate.path)?;
        return Ok(true);
    }
    if parent == Some(&build_root.join(oer_toolchain::image::COMPILE_CACHES)) {
        return oer_toolchain::image::remove_orphaned_compile_cache(&candidate.path);
    }
    if parent == Some(&build_root.join(oer_toolchain::image::LINKERS)) {
        let aside = candidate
            .path
            .with_file_name(format!(".removing-{name}-{}", std::process::id()));
        fs::rename(&candidate.path, &aside)?;
        if !linker_unused(&aside, policy, SystemTime::now()) {
            if fs::rename(&aside, &candidate.path).is_err() {
                fs::remove_dir_all(&aside)?;
            }
            return Ok(false);
        }
        fs::remove_dir_all(&aside)?;
        return Ok(true);
    }
    fs::remove_dir_all(&candidate.path)?;
    Ok(true)
}

/// List, or with `apply` remove, the caches of the checkout at `root`, and
/// in the host build root the compile caches of trees that are gone and the
/// unused image linkers; returns the bytes that were (or would be) freed.
pub fn run(root: &Path, policy: Policy, apply: bool) -> Result<u64> {
    let chips: Vec<String> = oer_chip_profile::Profile::all(root)?
        .into_iter()
        .map(|profile| profile.id)
        .collect();
    let build_root = oer_image::host_build_root()?;
    let found = unheld(
        candidates(root, &build_root, &chips, policy, SystemTime::now()),
        &held_by_jobs()?,
    );
    let (mut bytes, mut count) = (0, 0);
    for candidate in &found {
        let candidate_bytes = size(&candidate.path);
        if apply {
            match remove(candidate, &build_root, policy) {
                Ok(true) => {
                    bytes += candidate_bytes;
                    count += 1;
                }
                Ok(false) => eprintln!(
                    "sweep: kept {}: a build took it since it was listed",
                    candidate.path.display()
                ),
                Err(error) => {
                    eprintln!("sweep: cannot remove {}: {error}", candidate.path.display());
                }
            }
        } else {
            bytes += candidate_bytes;
            count += 1;
            if found.len() <= 20 {
                println!("  {} ({})", candidate.path.display(), candidate.reason);
            }
        }
    }
    println!(
        "{} {}: {} caches, {:.1} GiB",
        if apply { "removed" } else { "would remove" },
        root.display(),
        count,
        bytes as f64 / (1u64 << 30) as f64
    );
    Ok(bytes)
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
        let stale =
            target.join("hil/chip-a/psram-code-psram-data-psram-stack-correctness-owned-xarxa");
        let live =
            target.join("hil/chip-a/psram-code-psram-data-psram-stack-performance-owned-xarxa");
        let runs = target.join("hil/chip-a/runs.before-shared-store");
        let peer = target.join("hil/chip-b/check-system-watchdog-owned-xarxa");
        let replay = target.join("hil/chip-b/replay");
        let runners = target.join("hil/runners");
        for directory in [&old, &fresh, &stale, &live, &runs, &peer, &replay, &runners] {
            fs::create_dir_all(directory).unwrap();
        }
        for directory in [&old, &stale, &peer] {
            aged(directory, 10);
        }
        for directory in [&runs, &replay, &runners] {
            aged(directory, 30);
        }
        let chips = ["chip-b", "chip-a"].map(String::from);
        let found = candidates(
            root.path(),
            &root.path().join("build"),
            &chips,
            Policy::default(),
            SystemTime::now(),
        );
        let paths = found.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths, [old, stale, peer]);
    }

    #[test]
    fn files_a_queued_job_was_fixed_with_are_kept_with_what_holds_them() {
        let candidate = |path: &str| Candidate {
            path: PathBuf::from(path),
            reason: String::from("idle"),
        };
        let held = [PathBuf::from("/c/target/hil/jobs/cli/ab/oer-hil-cli")];
        assert_eq!(
            unheld(
                vec![
                    candidate("/c/target"),
                    candidate("/c/target/hil/jobs/cli/ab/oer-hil-cli"),
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
        let lock = oer_process::lock::FileLock::acquire(
            &profile.join(".cargo-lock"),
            oer_process::lock::Mode::Exclusive,
        )
        .unwrap();
        assert!(
            candidates(
                root.path(),
                &root.path().join("build"),
                &[],
                Policy::default(),
                SystemTime::now()
            )
            .is_empty()
        );
        drop(lock);
        assert_eq!(
            candidates(
                root.path(),
                &root.path().join("build"),
                &[],
                Policy::default(),
                SystemTime::now()
            )
            .len(),
            1
        );
    }

    /// The host build root loses the compile cache of a tree that is gone
    /// and an unused linker, and keeps a live tree's cache, one a build
    /// holds and a linker in use.
    #[test]
    fn caches_of_gone_trees_and_unused_linkers_are_swept() {
        let host = tempfile::tempdir().unwrap();
        let build_root = host.path().join("build");
        let live = tempfile::tempdir().unwrap();
        let gone = tempfile::tempdir().unwrap();
        let busy = tempfile::tempdir().unwrap();
        let live_cache = oer_toolchain::image::compile_cache(&build_root, live.path())
            .unwrap()
            .path()
            .to_owned();
        let gone_cache = oer_toolchain::image::compile_cache(&build_root, gone.path()).unwrap();
        let busy_cache = oer_toolchain::image::compile_cache(&build_root, busy.path()).unwrap();
        let gone_path = gone.path().to_owned();
        drop(gone);
        drop(busy);
        let _held = busy_cache.lock().unwrap();
        let linkers = build_root.join(oer_toolchain::image::LINKERS);
        let (old, used) = (linkers.join("old"), linkers.join("used"));
        for linker in [&old, &used] {
            fs::create_dir_all(linker).unwrap();
            fs::write(linker.join("oer-image-linker"), "").unwrap();
        }
        aged(&old.join("oer-image-linker"), 10);
        let policy = Policy::default();
        let found = candidates(live.path(), &build_root, &[], policy, SystemTime::now());
        let paths = found.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        let mut expected = vec![gone_cache.path().to_owned(), old.clone()];
        expected.sort();
        assert_eq!(paths, expected);
        assert!(live_cache.is_dir());
        // At removal the tree is looked at again: a live tree keeps its cache.
        let live_candidate = Candidate {
            path: live_cache.clone(),
            reason: String::from("listed"),
        };
        assert!(!remove(&live_candidate, &build_root, policy).unwrap());
        assert!(live_cache.is_dir());
        // A build that takes a listed cache before its removal keeps it.
        let gone = found
            .iter()
            .find(|candidate| candidate.path == gone_cache.path())
            .unwrap();
        let taken = gone_cache.lock().unwrap();
        assert!(!remove(gone, &build_root, policy).unwrap());
        assert!(gone_cache.path().is_dir());
        drop(taken);
        // Another directory now at the gone tree's path is another tree.
        fs::create_dir_all(&gone_path).unwrap();
        assert!(remove(gone, &build_root, policy).unwrap());
        assert!(!gone_cache.path().exists());
        fs::remove_dir(&gone_path).unwrap();
        // A linker a build renewed since the listing stays.
        let unused = found
            .iter()
            .find(|candidate| candidate.path == old)
            .unwrap();
        fs::File::open(old.join("oer-image-linker"))
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
        assert!(!remove(unused, &build_root, policy).unwrap());
        assert!(old.join("oer-image-linker").is_file());
        aged(&old.join("oer-image-linker"), 10);
        assert!(remove(unused, &build_root, policy).unwrap());
        assert!(!old.exists());
        // A removal interrupted after moving a cache aside is swept once its
        // remover is gone and a minute has passed; a recent one is kept.
        let caches = build_root.join(oer_toolchain::image::COMPILE_CACHES);
        // Above the kernel's largest pid: no process has it.
        let (left, fresh) = (
            caches.join(".removing-0123456789abcdef-4194305"),
            caches.join(".removing-fedcba9876543210-4194305"),
        );
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&fresh).unwrap();
        aged(&left, 10);
        // A linker moved aside is listed once, as an interrupted removal.
        let aside = linkers.join(".removing-old-4194305");
        fs::create_dir_all(&aside).unwrap();
        fs::write(aside.join("oer-image-linker"), "").unwrap();
        aged(&aside.join("oer-image-linker"), 10);
        aged(&aside, 10);
        let found = candidates(live.path(), &build_root, &[], policy, SystemTime::now());
        assert!(found.iter().any(|candidate| candidate.path == left));
        assert!(!found.iter().any(|candidate| candidate.path == fresh));
        let listed: Vec<_> = found
            .iter()
            .filter(|candidate| candidate.path == aside)
            .map(|candidate| candidate.reason.as_str())
            .collect();
        assert_eq!(listed, ["interrupted removal"]);
    }

    /// A job names the snapshot it reads with `--source-snapshot`; a sweep
    /// and source collection keep it as if the job were fixed with it.
    #[test]
    fn a_job_holds_the_snapshot_it_names() {
        let command = |arguments: &[&str]| -> Vec<String> {
            arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect()
        };
        assert_eq!(
            named_snapshot(&command(&[
                "run",
                "smoke",
                "--source-snapshot",
                "/s/schema-2/ab"
            ])),
            Some("/s/schema-2/ab")
        );
        assert_eq!(
            named_snapshot(&command(&["run", "--source-snapshot=rel/cd", "smoke"])),
            Some("rel/cd")
        );
        assert_eq!(named_snapshot(&command(&["run", "smoke"])), None);
    }
}
