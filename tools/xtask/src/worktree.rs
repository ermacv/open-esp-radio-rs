//! Warm Git worktrees of this repository.
//!
//! A new worktree otherwise starts with an empty `target/` and rebuilds every
//! dependency (about 10 minutes and tens of gigabytes). `add` seeds its
//! `target/` from this checkout's build outputs, so only the workspace's own
//! crates rebuild. When this checkout's `target/` is a btrfs subvolume (see
//! `prepare`) the seed is an instant snapshot; otherwise it is a reflink copy,
//! which takes minutes for a large `target/`, and on a filesystem without
//! reflinks `target/` stays empty. The seed is private to the new worktree:
//! two checkouts never share one `target/`, whose build scripts and
//! fingerprints record absolute paths.
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{Context, Result, process};

/// Subdirectories of `target/` that are never worth cloning: incremental
/// session data is keyed to this checkout's paths, HIL outputs live in the
/// shared run store or are rebuilt per image, and vendor firmware builds keep
/// CMake caches that record this checkout's absolute source directory.
const SKIPPED: &[&str] = &["incremental", "hil", "vendor-firmware"];

/// Whether a `target/` entry at `relative` is left out of the seed.
pub fn skipped(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| SKIPPED.iter().any(|skip| component.as_os_str() == *skip))
}

/// Creates `path` as a worktree on `branch` (new, from `from`) and seeds its
/// build outputs.
pub fn add(ctx: &Context, path: &Path, branch: &str, from: &str) -> Result<()> {
    if path.exists() {
        return Err(format!("worktree: {} already exists", path.display()).into());
    }
    process::run(
        ctx.command("git")
            .args(["worktree", "add", "-b", branch])
            .arg(path)
            .arg(from),
    )?;
    let source = ctx.root.join("target");
    if !source.is_dir() {
        println!("worktree: this checkout has no target/ to seed from");
        return Ok(());
    }
    let destination = path.join("target");
    if is_subvolume(&source)? {
        process::capture(
            std::process::Command::new("btrfs")
                .args(["subvolume", "snapshot"])
                .arg(&source)
                .arg(&destination),
        )?;
        prune(&destination)?;
        println!(
            "worktree: {} on {branch}; target/ is a snapshot of {}",
            path.display(),
            source.display()
        );
    } else {
        let seeded = seed(&source, &destination)?;
        println!(
            "worktree: {} on {branch}; seeded {seeded} build output entries from {} by reflink copy; `cargo xtask worktree prepare` makes the next seed an instant snapshot",
            path.display(),
            source.display()
        );
    }
    Ok(())
}

/// A btrfs subvolume root always has inode number 256.
fn is_subvolume(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    const BTRFS_SUBVOLUME_ROOT_INODE: u64 = 256;
    let metadata = fs::metadata(path)?;
    Ok(metadata.is_dir()
        && metadata.ino() == BTRFS_SUBVOLUME_ROOT_INODE
        && fs_type(path)? == "btrfs")
}

fn fs_type(path: &Path) -> Result<String> {
    Ok(String::from_utf8(
        process::capture(
            std::process::Command::new("findmnt")
                .args(["-no", "FSTYPE", "-T"])
                .arg(path),
        )?
        .stdout,
    )?
    .trim()
    .to_owned())
}

/// Drops the [`SKIPPED`] entries from a snapshot.
fn prune(target: &Path) -> Result<()> {
    for entry in fs::read_dir(target)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if skipped(Path::new(&entry.file_name())) {
            fs::remove_dir_all(entry.path())?;
        } else {
            remove_incremental(&entry.path())?;
        }
    }
    Ok(())
}

/// Turns this checkout's `target/` into a btrfs subvolume once, so every
/// later `add` snapshots it instantly. Run it while no build uses `target/`.
pub fn prepare(ctx: &Context) -> Result<()> {
    let target = ctx.root.join("target");
    if target.exists() && is_subvolume(&target)? {
        println!("worktree: {} is already a subvolume", target.display());
        return Ok(());
    }
    let staging = ctx.root.join("target.subvolume");
    let retired = ctx.root.join("target.retired");
    if staging.exists() || retired.exists() {
        return Err(format!(
            "worktree: remove the leftover {} / {} of an interrupted prepare first",
            staging.display(),
            retired.display()
        )
        .into());
    }
    process::capture(
        std::process::Command::new("btrfs")
            .args(["subvolume", "create"])
            .arg(&staging),
    )?;
    if target.exists() {
        println!(
            "worktree: moving {} into a subvolume by reflink copy (minutes, once)",
            target.display()
        );
        let mut copy = std::process::Command::new("cp");
        copy.args(["-a", "--reflink=always", "-T"])
            .arg(&target)
            .arg(&staging);
        if let Err(error) = process::capture(&mut copy) {
            let _ = fs::remove_dir_all(&staging);
            return Err(
                format!("worktree: reflink copy into the subvolume failed: {error}").into(),
            );
        }
        fs::rename(&target, &retired)?;
    }
    fs::rename(&staging, &target)?;
    if retired.exists() {
        fs::remove_dir_all(&retired)?;
    }
    println!("worktree: {} is now a subvolume", target.display());
    Ok(())
}

/// Clones every top-level build profile directory with reflinks, skipping
/// [`SKIPPED`] entries. Fails without copying when reflinks are unsupported.
fn seed(source: &Path, destination: &Path) -> Result<usize> {
    fs::create_dir_all(destination)?;
    let mut seeded = 0;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = PathBuf::from(entry.file_name());
        if skipped(&name) || !entry.file_type()?.is_dir() {
            continue;
        }
        let mut copy = std::process::Command::new("cp");
        copy.args(["-a", "--reflink=always"])
            .arg(entry.path())
            .arg(destination.join(&name));
        if let Err(error) = process::capture(&mut copy) {
            let _ = fs::remove_dir_all(destination.join(&name));
            return Err(format!(
                "worktree: reflink copy of {} failed (copy-on-write unsupported?); target/ left unseeded: {error}",
                entry.path().display()
            )
            .into());
        }
        remove_incremental(&destination.join(&name))?;
        seeded += 1;
    }
    Ok(seeded)
}

/// Incremental session data below a profile directory, as `cp` copied it.
fn remove_incremental(profile: &Path) -> Result<()> {
    for entry in fs::read_dir(profile)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if entry.file_name() == "incremental" {
            fs::remove_dir_all(entry.path())?;
        } else if entry.path().join("incremental").is_dir() {
            // Cross-compiled profiles: target/<triple>/<profile>/incremental.
            fs::remove_dir_all(entry.path().join("incremental"))?;
        }
    }
    Ok(())
}

/// Removes a worktree added by [`add`], including its private `target/`.
pub fn remove(ctx: &Context, path: &Path) -> Result<()> {
    let listed = String::from_utf8(
        process::capture(ctx.command("git").args(["worktree", "list", "--porcelain"]))?.stdout,
    )?;
    let path = path.canonicalize()?;
    if !listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| Path::new(listed) == path)
    {
        return Err(format!(
            "worktree: {} is not a worktree of this repository",
            path.display()
        )
        .into());
    }
    if path == ctx.root.canonicalize()? {
        return Err("worktree: refusing to remove the checkout running this command".into());
    }
    // `git worktree remove` refuses untracked build outputs; drop them first.
    let target = path.join("target");
    if target.is_dir() {
        fs::remove_dir_all(&target)?;
    }
    process::run(ctx.command("git").args(["worktree", "remove"]).arg(&path))?;
    println!("worktree: removed {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_skip_incremental_data_and_hil_outputs() {
        assert!(skipped(Path::new("hil")));
        assert!(skipped(Path::new("vendor-firmware")));
        assert!(skipped(Path::new("debug/incremental")));
        assert!(!skipped(Path::new("debug")));
        assert!(!skipped(Path::new("riscv32imafc-unknown-none-elf")));
    }

    #[test]
    fn a_seed_clones_profiles_without_incremental_data() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("debug/deps")).unwrap();
        fs::create_dir_all(source.join("debug/incremental/crate")).unwrap();
        fs::create_dir_all(source.join("hil/esp32s31")).unwrap();
        fs::write(source.join("debug/deps/libx.rlib"), b"rlib").unwrap();
        let destination = dir.path().join("destination");
        match seed(&source, &destination) {
            Ok(seeded) => {
                assert_eq!(seeded, 1);
                assert_eq!(
                    fs::read(destination.join("debug/deps/libx.rlib")).unwrap(),
                    b"rlib"
                );
                assert!(!destination.join("debug/incremental").exists());
                assert!(!destination.join("hil").exists());
            }
            // tmpfs and ext4 have no reflinks: the seed must leave nothing.
            Err(_) => assert!(!destination.join("debug").exists()),
        }
    }
}
