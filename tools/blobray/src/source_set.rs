//! Run-owned immutable source bytes. Paths locate captures, never live facts.

use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use crate::{Result, artifact::CapturedArtifact};

struct CapturedSource {
    bytes: Arc<[u8]>,
    sha256: String,
    binary: OnceLock<std::result::Result<CapturedArtifact<'static>, String>>,
}

/// Read outcome retained independently of parsing a captured source.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SourceCaptureFailure {
    pub(crate) missing: bool,
    pub(crate) reason: String,
}

/// Captures each declared path once before any pass consumes its bytes.
///
/// Read failures remain attached to their declared paths; unrelated consumers
/// can still use available captures. A failed binary parse is cached separately
/// and does not erase the raw bytes. Lookups never read or retry the filesystem.
/// The application owns this set for the run, including export. Publication
/// guards separately decide whether live inputs still match that generation.
pub(crate) struct CapturedSourceSet {
    sources: BTreeMap<PathBuf, std::result::Result<CapturedSource, SourceCaptureFailure>>,
}

impl CapturedSourceSet {
    pub(crate) fn capture(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut sources = BTreeMap::new();
        for path in paths {
            sources.entry(path.clone()).or_insert_with(|| {
                std::fs::read(&path)
                    .map(|bytes| CapturedSource {
                        sha256: format!("{:x}", Sha256::digest(&bytes)),
                        bytes: bytes.into(),
                        binary: OnceLock::new(),
                    })
                    .map_err(|error| SourceCaptureFailure {
                        missing: matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ),
                        reason: error.to_string(),
                    })
            });
        }
        Self { sources }
    }

    fn source(&self, path: &Path) -> Result<&CapturedSource> {
        self.sources
            .get(path)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "source {} was not declared in this captured source set",
                    path.display()
                ))
            })?
            .as_ref()
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "source {} could not be captured: {}",
                    path.display(),
                    error.reason
                ))
            })
    }

    pub(crate) fn sha256(&self, path: &Path) -> Result<&str> {
        Ok(&self.source(path)?.sha256)
    }

    /// Captured bytes are available even when no binary parser accepts them.
    pub(crate) fn bytes(&self, path: &Path) -> Result<&[u8]> {
        Ok(&self.source(path)?.bytes)
    }

    pub(crate) fn capture_failure(&self, path: &Path) -> Option<&SourceCaptureFailure> {
        self.sources
            .get(path)
            .and_then(|source| source.as_ref().err())
    }

    pub(crate) fn artifact(&self, path: &Path) -> Result<&CapturedArtifact<'static>> {
        let source = self.source(path)?;
        source
            .binary
            .get_or_init(|| {
                CapturedArtifact::from_shared(source.bytes.clone())
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| {
                crate::Error::invalid(format!(
                    "captured source {} is not an analyzable artifact: {error}",
                    path.display()
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_failures_and_bytes_without_live_lookup_or_retry() {
        let directory = tempfile::tempdir().unwrap();
        let present = directory.path().join("source");
        let missing = directory.path().join("missing");
        std::fs::write(&present, b"not an ELF").unwrap();
        let sources =
            CapturedSourceSet::capture([present.clone(), present.clone(), missing.clone()]);
        assert_eq!(sources.sources.len(), 2);
        let digest = sources.sha256(&present).unwrap().to_owned();
        std::fs::write(&present, b"replacement").unwrap();
        std::fs::write(&missing, b"now present").unwrap();
        assert_eq!(sources.sha256(&present).unwrap(), digest);
        assert_eq!(sources.bytes(&present).unwrap(), b"not an ELF");
        assert!(sources.capture_failure(&missing).unwrap().missing);
        assert!(sources.capture_failure(&present).is_none());
        assert!(
            sources
                .sha256(&missing)
                .unwrap_err()
                .to_string()
                .contains("could not be captured")
        );
        assert!(
            sources
                .sha256(&directory.path().join("undeclared"))
                .unwrap_err()
                .to_string()
                .contains("not declared")
        );
        let failure = sources.artifact(&present).err().unwrap().to_string();
        std::fs::remove_file(&present).unwrap();
        assert_eq!(
            sources.artifact(&present).err().unwrap().to_string(),
            failure
        );
        assert_eq!(sources.sha256(&present).unwrap(), digest);
        assert_eq!(sources.bytes(&present).unwrap(), b"not an ELF");
    }
}
