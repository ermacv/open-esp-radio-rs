//! Build exclusion: one mechanism, the foundation's advisory file lock
//! ([`oer_process::lock::FileLock`]), for every place builds must not
//! overlap.
//!
//! - [`BuildLock`]: one build owns the private lockfile copy in its
//!   directory; a second build of the same directory fails at once.
//! - an output directory (an example's compile cache) is built by one build
//!   at a time, the next waiting for it (`FileLock::wait`, exclusive);
//! - [`slot`]: the host-wide limit on concurrent image compilations, one of
//!   a fixed number of lock files;
//! - the ESP-IDF cache: exclusive while its tree and tools change, shared
//!   while builds read them.
//!
//! The lock is the kernel's `flock`: released when its owner exits, however
//! it exits, so a crashed build never leaves a stale lock behind.
use crate::Result;
use oer_process::lock::{FileLock, Mode};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// One of the host's image compilation slots below `directory`: each holds
/// one runtime build's fat LTO, about 1.5 GB and one core for minutes, and
/// parallel builds of several checkouts otherwise pushed the host into
/// swap. Waits for a free slot.
pub fn slot(directory: &Path) -> Result<FileLock> {
    let count = slots();
    let paths: Vec<PathBuf> = (0..count)
        .map(|slot| directory.join(format!("{slot}.lock")))
        .collect();
    FileLock::wait_any(
        &paths,
        Mode::Exclusive,
        &format!("waiting for one of the host's {count} image build slots"),
    )
}

/// The build lock of the output directory `cache`: one build at a time,
/// the next waiting for it.
pub fn output_directory(cache: &Path) -> Result<FileLock> {
    FileLock::wait(
        &cache.join("build.lock"),
        Mode::Exclusive,
        &format!("waiting for another build in {}", cache.display()),
    )
}

/// Image compilations the host runs at once: half its cores, and no more
/// than its memory holds at 2 GB each.
fn slots() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let memory_gb = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|info| {
            info.lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|kb| {
                    kb.trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
        })
        .map_or(4, |kb| kb >> 20);
    (cores / 2).min(memory_gb / 2).max(1)
}

/// A private copy of one workspace's committed `Cargo.lock` for one build.
///
/// Cargo resolves through `resolver.lockfile-path`, so a patched or locally
/// overridden resolution is written only to this copy. The committed catalog
/// is never modified, concurrent builds never observe a temporary resolution,
/// and the copy is the build's effective lockfile. One build owns the copy's
/// directory at a time.
pub struct BuildLock {
    committed: PathBuf,
    path: PathBuf,
    _lease: FileLock,
}

impl BuildLock {
    /// Copy `workspace/Cargo.lock` to `directory/Cargo.lock` for one build.
    pub fn prepare(workspace: &Path, directory: &Path) -> Result<Self> {
        let lease = FileLock::try_acquire(&directory.join("build.lease"), Mode::Exclusive)?
            .ok_or_else(|| format!("another build owns {}", directory.display()))?;
        let committed = workspace.join("Cargo.lock");
        let path = directory.join("Cargo.lock");
        fs::copy(&committed, &path)?;
        Ok(Self {
            committed,
            path,
            _lease: lease,
        })
    }

    /// The effective lockfile of this build.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Resolve `command` through this copy instead of the committed catalog.
    pub fn configure(&self, command: &mut Command) {
        let path = toml::Value::String(self.path.display().to_string());
        command
            .arg("--config")
            .arg(format!("resolver.lockfile-path={path}"));
    }

    /// Check that the build resolved exactly the committed pins.
    pub fn validate(&self) -> Result<()> {
        validate_identities(
            identities(&fs::read(&self.committed)?)?,
            identities(&fs::read(&self.path)?)?,
        )
    }
}

type Identity = (String, String, Option<String>);
fn identities(bytes: &[u8]) -> Result<BTreeSet<Identity>> {
    let lock: toml::Value = toml::from_str(std::str::from_utf8(bytes)?)?;
    lock["package"]
        .as_array()
        .ok_or("Cargo.lock has no package catalog")?
        .iter()
        .map(|p| {
            Ok((
                p["name"].as_str().ok_or("package name missing")?.to_owned(),
                p["version"]
                    .as_str()
                    .ok_or("package version missing")?
                    .to_owned(),
                p.get("source")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned),
            ))
        })
        .collect()
}
fn validate_identities(expected: BTreeSet<Identity>, actual: BTreeSet<Identity>) -> Result<()> {
    if expected != actual {
        return Err(format!(
            "the build changed dependency pins: removed {:?}; added {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
