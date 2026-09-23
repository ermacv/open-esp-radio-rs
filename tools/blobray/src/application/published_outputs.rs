//! An immutable output manifest and read capability for one published epoch.

use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::query_store::QueryReader;
use crate::Result;

/// One logical destination and the exact content published for it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedOutput {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

/// Complete set of emitted outputs retained by a successful project run.
///
/// This records output content, not a complete source/evidence snapshot.
/// Paths identify declared destinations; reading does not reopen those paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedOutputManifest {
    pub schema: u32,
    pub epoch: String,
    pub project_manifest: PathBuf,
    pub outputs: Vec<PublishedOutput>,
}

impl PublishedOutputManifest {
    pub(super) fn validate(&self, epoch: &str) -> Result<()> {
        if self.schema != 2 || self.epoch != epoch || !self.project_manifest.is_absolute() {
            return Err(crate::Error::invalid(
                "published output manifest identity mismatch",
            ));
        }
        let mut previous = None;
        for output in &self.outputs {
            if previous.is_some_and(|path| path >= &output.path)
                || output.sha256.len() != 64
                || !output
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(crate::Error::invalid(
                    "invalid or duplicate published output binding",
                ));
            }
            previous = Some(&output.path);
        }
        Ok(())
    }
}

pub(super) fn manifest_key(epoch: &str) -> String {
    format!("analysis-output-manifest:v2:{epoch}")
}

/// Manifest locator within its resolved parent directory. The file itself may
/// not exist for low-level cache callers; this is ownership, not a content hash.
pub(super) fn project_locator(manifest: &Path) -> Result<PathBuf> {
    let parent = manifest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = manifest
        .file_name()
        .ok_or_else(|| crate::Error::invalid("project manifest has no filename"))?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

pub(super) fn output_key(digest: &str) -> String {
    format!("analysis-output:v1:{digest}")
}

/// Read-only handle retaining the published epoch observed at open time.
///
/// Its SQLite transaction and pack descriptors remain valid while another run
/// writes or publishes. Deleting/replacing generated files does not change its
/// content. Opening never starts analysis or restores a generated file; SQLite
/// may perform WAL/SHM reader coordination. Drop the handle to release its read
/// transaction and pack descriptors.
pub struct PublishedAnalysisOutputs {
    reader: QueryReader,
    manifest: PublishedOutputManifest,
}

/// Authenticated output payload with its own bounded seek cursor.
///
/// Opening verifies the full payload in bounded memory. Subsequent reads and
/// seeks access only this payload and do not hash it again. The pinned file
/// descriptor remains usable after the parent snapshot is dropped or its pack
/// pathname is removed by compaction. CAS payload bytes are immutable under the
/// store's writer contract; external in-place modification after opening is not
/// prevented by a file descriptor.
pub struct PublishedOutputReader {
    cursor: crate::file_view::FileCursor,
}

impl Read for PublishedOutputReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buffer)
    }
}

impl Seek for PublishedOutputReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

impl PublishedAnalysisOutputs {
    /// Open the current published outputs. A missing cache or a cache without
    /// a completed project epoch returns None. A published epoch without its
    /// manifest is an error, never a fallback to mutable generated paths.
    pub fn open(project_manifest: &Path) -> super::ApplicationResult<Option<Self>> {
        Ok(Self::open_inner(project_manifest)?)
    }

    fn open_inner(project_manifest: &Path) -> Result<Option<Self>> {
        let Some(reader) = QueryReader::open(project_manifest)? else {
            return Ok(None);
        };
        let Some(epoch) = reader.published_epoch() else {
            return Ok(None);
        };
        let bytes = reader.get(&manifest_key(epoch))?.ok_or_else(|| {
            crate::Error::invalid(
                "published analysis epoch has no output manifest; rebuild analysis",
            )
        })?;
        let manifest: PublishedOutputManifest = serde_json::from_slice(&bytes)?;
        manifest.validate(epoch)?;
        if manifest.project_manifest != project_locator(project_manifest)? {
            return Err(crate::Error::invalid(format!(
                "published analysis epoch belongs to another project manifest: {}",
                manifest.project_manifest.display()
            )));
        }
        Ok(Some(Self { reader, manifest }))
    }

    pub fn manifest(&self) -> &PublishedOutputManifest {
        &self.manifest
    }

    /// Read one declared logical destination from the pinned CAS generation.
    /// Missing declarations return None; missing/corrupt declared content is
    /// an error. Paths must match the manifest exactly, without live filesystem
    /// canonicalization that could resolve into a different generation.
    pub fn read(&self, path: &Path) -> super::ApplicationResult<Option<Vec<u8>>> {
        Ok(self.read_inner(path)?)
    }

    /// Open one declared payload for streaming and indexed reads. Offsets are
    /// relative to the payload; seeking outside its bounds is an error.
    pub fn open_output(
        &self,
        path: &Path,
    ) -> super::ApplicationResult<Option<PublishedOutputReader>> {
        Ok(self.output_view(path)?.map(|view| PublishedOutputReader {
            cursor: view.cursor(),
        }))
    }

    fn read_inner(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let Some(view) = self.output_view(path)? else {
            return Ok(None);
        };
        let mut value = Vec::new();
        view.cursor().read_to_end(&mut value)?;
        // Buffered reads authenticate the returned allocation itself. Streaming
        // readers authenticate at open and rely on immutable CAS bytes later.
        let output = self.binding(path).expect("validated output binding");
        let identity = super::generated_file::ContentIdentity::read(value.as_slice())?;
        if identity.bytes != output.bytes || identity.sha256 != output.sha256 {
            return Err(crate::Error::invalid(
                "published output changed during buffered read",
            ));
        }
        Ok(Some(value))
    }

    pub(super) fn output_view(&self, path: &Path) -> Result<Option<crate::file_view::FileView>> {
        let Some(output) = self.binding(path) else {
            return Ok(None);
        };
        Ok(Some(self.reader.output_view(&output.sha256, output.bytes)?))
    }

    fn binding(&self, path: &Path) -> Option<&PublishedOutput> {
        self.manifest
            .outputs
            .binary_search_by(|output| output.path.as_path().cmp(path))
            .ok()
            .map(|index| &self.manifest.outputs[index])
    }
}
