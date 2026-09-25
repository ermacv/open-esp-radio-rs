//! A shared compilation cache and a private, retained image bundle.

use crate::Result;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

/// The exact completed bundle selected for flashing; later builds use new paths.
pub struct FirmwareBuild {
    directory: PathBuf,
}

impl FirmwareBuild {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

pub(super) struct Workspace {
    cache: PathBuf,
    bundle: tempfile::TempDir,
    _lease: ArtifactLease,
}

impl Workspace {
    pub(super) fn acquire(directory: &Path) -> Result<Self> {
        fs::create_dir_all(directory)?;
        let lease = ArtifactLease::acquire(&directory.join("build.lock"))?;
        let bundle = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(directory)?;
        Ok(Self {
            cache: directory.join("cargo"),
            bundle,
            _lease: lease,
        })
    }

    pub(super) fn cache(&self) -> &Path {
        &self.cache
    }

    pub(super) fn output(&self) -> &Path {
        self.bundle.path()
    }

    pub(super) fn snapshot(&self, source: &Path, name: &str) -> Result<PathBuf> {
        let destination = self.output().join(name);
        fs::copy(source, &destination)?;
        Ok(destination)
    }

    pub(super) fn finish(self) -> FirmwareBuild {
        FirmwareBuild {
            directory: self.bundle.keep(),
        }
    }
}

struct ArtifactLease(File);

impl ArtifactLease {
    fn acquire(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        loop {
            oer_process::check_cancelled()?;
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    oer_process::sleep(Duration::from_millis(20))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for ArtifactLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests;
