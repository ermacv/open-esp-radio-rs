//! Build exclusion: one mechanism, an advisory exclusive lock on a file
//! ([`Lease`]), for every place builds must not overlap.
//!
//! - [`BuildLock`]: one build owns the private lockfile copy in its
//!   directory; a second build of the same directory fails at once.
//! - [`Lease::wait`]: one build at a time of an output directory (an
//!   example's compile cache), the next waiting for it.
//! - [`slot`]: the host-wide limit on concurrent image compilations, one of
//!   a fixed number of lock files.
//!
//! The lock is the kernel's `flock`: released when its owner exits, however
//! it exits, so a crashed build never leaves a stale lock behind.
use crate::Result;
use fs2::FileExt;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// An exclusive lock on one file, held until dropped.
///
/// Closing one descriptor does not release flock while a forked pre-exec
/// child still holds the shared open file description, so the owner
/// unlocks explicitly when it drops the lease, failures included.
pub struct Lease(fs::File);

impl Lease {
    fn open(path: &Path) -> Result<fs::File> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?)
    }

    /// The lease of `path`, or `None` while another owner holds it.
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = Self::open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self(file))),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(format!("lock {}: {error}", path.display()).into()),
        }
    }

    /// The lease of `path`, waiting for its owner (cancellably); `waiting`
    /// is printed once when the wait starts.
    pub fn wait(path: &Path, waiting: &str) -> Result<Self> {
        Self::wait_any(&[path.to_owned()], waiting)
    }

    /// The first free lease of `paths`, waiting until one is free.
    fn wait_any(paths: &[PathBuf], waiting: &str) -> Result<Self> {
        let mut announced = false;
        loop {
            for path in paths {
                if let Some(lease) = Self::try_acquire(path)? {
                    return Ok(lease);
                }
            }
            if !announced {
                eprintln!("{waiting}");
                announced = true;
            }
            oer_process::sleep(Duration::from_millis(200))?;
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.0) {
            eprintln!("release a build lease: {error}");
        }
    }
}

/// One of the host's image compilation slots below `directory`: each holds
/// one runtime build's fat LTO, about 1.5 GB and one core for minutes, and
/// parallel builds of several checkouts otherwise pushed the host into
/// swap. Waits for a free slot.
pub fn slot(directory: &Path) -> Result<Lease> {
    let count = slots();
    let paths: Vec<PathBuf> = (0..count)
        .map(|slot| directory.join(format!("{slot}.lock")))
        .collect();
    Lease::wait_any(
        &paths,
        &format!("waiting for one of the host's {count} image build slots"),
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
    _lease: Lease,
}

impl BuildLock {
    /// Copy `workspace/Cargo.lock` to `directory/Cargo.lock` for one build.
    pub fn prepare(workspace: &Path, directory: &Path) -> Result<Self> {
        let lease = Lease::try_acquire(&directory.join("build.lease"))?
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
