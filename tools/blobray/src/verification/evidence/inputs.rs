//! Private, per-execution copies of explicitly selected comparison inputs.
//! Loaders and report hashes read the same files even if the working tree changes.

use crate::{Result, verification::VerificationCommandReport};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct ExecutionInputs {
    directory: tempfile::TempDir,
    paths: BTreeMap<PathBuf, PathBuf>,
}

impl ExecutionInputs {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            directory: tempfile::tempdir()?,
            paths: BTreeMap::new(),
        })
    }

    pub(crate) fn capture(&mut self, path: &mut PathBuf) -> Result<()> {
        let original = fs::canonicalize(&*path)?;
        if let Some(copy) = self.paths.get(&original) {
            *path = copy.clone();
            return Ok(());
        }
        let directory = self.directory.path().join(self.paths.len().to_string());
        fs::create_dir(&directory)?;
        let copy = directory.join(
            original
                .file_name()
                .ok_or_else(|| crate::Error::invalid("comparison input has no filename"))?,
        );
        // Copy bytes, never hard-link a mutable execution input.
        let mut source = fs::File::open(&original)?;
        let metadata = source.metadata()?;
        let mut destination = fs::File::create_new(&copy)?;
        std::io::copy(&mut source, &mut destination)?;
        // Preserve the age used by artifact freshness checks; copying must not
        // make an old production image look newer than its source files.
        destination.set_times(std::fs::FileTimes::new().set_modified(metadata.modified()?))?;
        let mut permissions = metadata.permissions();
        permissions.set_readonly(true);
        destination.set_permissions(permissions)?;
        self.paths.insert(original, copy.clone());
        *path = copy;
        Ok(())
    }

    pub(crate) fn original(&self, path: &mut String) {
        if let Some((original, _)) = self
            .paths
            .iter()
            .find(|(_, copy)| copy.as_path() == Path::new(path))
        {
            *path = original.display().to_string();
        }
    }

    /// Restore diagnostic locations only; retain identities of the executed copies.
    pub(crate) fn restore_paths(&self, report: &mut VerificationCommandReport) {
        for artifact in &mut report.verification.artifacts {
            self.original(&mut artifact.path);
        }
        for comparison in report
            .sources
            .iter_mut()
            .flat_map(|s| &mut s.functions)
            .filter_map(|f| f.execution.as_mut())
        {
            for artifact in [&mut comparison.vendor, &mut comparison.rust] {
                self.original(&mut artifact.path);
                if let Some(companion) = &mut artifact.companion {
                    self.original(&mut companion.path);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_copy_survives_replacement_and_reuse_of_original_path() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("profile.toml");
        fs::write(&original, "scope = 'A'").unwrap();
        let original_time =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::OpenOptions::new()
            .write(true)
            .open(&original)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(original_time))
            .unwrap();
        let mut inputs = ExecutionInputs::new().unwrap();
        let mut loaded = original.clone();
        inputs.capture(&mut loaded).unwrap();
        fs::write(&original, "scope = 'B'").unwrap();
        let mut repeated = original.clone();
        inputs.capture(&mut repeated).unwrap();
        assert_eq!(loaded, repeated);
        let mut nested = ExecutionInputs::new().unwrap();
        let mut nested_copy = loaded.clone();
        nested.capture(&mut nested_copy).unwrap();
        assert_eq!(fs::read_to_string(nested_copy).unwrap(), "scope = 'A'");
        assert_eq!(
            fs::metadata(&loaded).unwrap().modified().unwrap(),
            original_time
        );
        assert_eq!(fs::read_to_string(&loaded).unwrap(), "scope = 'A'");
        let mut display = loaded.display().to_string();
        inputs.original(&mut display);
        assert_eq!(Path::new(&display), original);
    }
}
