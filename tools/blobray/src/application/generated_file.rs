//! One streaming write/check lifecycle for generated files and CAS restores.
//!
//! A request is consumed once. Writers stage beside the destination, validate
//! the complete stream and sync the file before replacing the destination.
//! This is a single-file primitive, not a transaction over a project snapshot.

use std::{
    fs,
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
    sync::OnceLock,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Result;

/// A caller-selected destination and operation, consumed by one emission.
/// Ownership/admission of the destination belongs to the calling coordinator.
/// This request itself does not grant project-wide publication authority.
pub(crate) struct GeneratedOutput<'a> {
    path: &'a Path,
    check: bool,
    kind: &'a str,
    completed: Option<&'a OnceLock<ContentIdentity>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContentIdentity {
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

impl ContentIdentity {
    pub(super) fn read(mut source: impl Read) -> Result<Self> {
        let mut writer = DigestWriter::new(std::io::sink());
        std::io::copy(&mut source, &mut writer)?;
        Ok(writer.finish().1)
    }
}

impl<'a> GeneratedOutput<'a> {
    pub(crate) fn new(path: &'a Path, check: bool, kind: &'a str) -> Self {
        Self {
            path,
            check,
            kind,
            completed: None,
        }
    }

    pub(super) fn tracked(
        path: &'a Path,
        check: bool,
        kind: &'a str,
        completed: &'a OnceLock<ContentIdentity>,
    ) -> Self {
        Self {
            path,
            check,
            kind,
            completed: Some(completed),
        }
    }

    pub(crate) fn text(self, contents: &str) -> Result<()> {
        self.bytes(contents.as_bytes())
    }

    pub(crate) fn bytes(self, contents: &[u8]) -> Result<()> {
        self.emit(None, |writer| {
            writer.write_all(contents)?;
            Ok(())
        })
    }

    /// Serialize directly into the output sink, without a second JSON copy.
    pub(crate) fn json<T: Serialize>(self, document: &T, pretty: bool) -> Result<()> {
        self.emit(None, |mut writer| {
            if pretty {
                serde_json::to_writer_pretty(&mut writer, document)?;
            } else {
                serde_json::to_writer(&mut writer, document)?;
            }
            writer.write_all(b"\n")?;
            Ok(())
        })
    }

    /// Copy one framed immutable object. Both length and digest must match
    /// before any staged bytes can replace the destination. Bytes following
    /// this frame belong to other pack records and are not consumed.
    pub(crate) fn verified_stream(self, source: impl Read, bytes: u64, sha256: &str) -> Result<()> {
        let expected = ContentIdentity {
            bytes,
            sha256: sha256.to_owned(),
        };
        self.emit(Some(&expected), |writer| {
            std::io::copy(&mut source.take(bytes), writer)?;
            Ok(())
        })
    }

    fn emit(
        self,
        expected: Option<&ContentIdentity>,
        serialize: impl FnOnce(&mut dyn Write) -> Result<()>,
    ) -> Result<()> {
        if self.check {
            let file = fs::File::open(self.path).map_err(|error| {
                crate::Error::invalid(format!(
                    "cannot check generated {} {}: {error}",
                    self.kind,
                    self.path.display()
                ))
            })?;
            let mut writer = DigestWriter::new(CompareWriter::new(BufReader::new(file)));
            serialize(&mut writer)?;
            writer.flush()?;
            let (comparison, identity) = writer.finish();
            self.validate_identity(expected, &identity)?;
            if !comparison.finish()? {
                return Err(crate::Error::invalid(format!(
                    "generated {} differs from {}; rerun without --check",
                    self.kind,
                    self.path.display()
                )));
            }
            self.trace(&identity, "checked");
            self.complete(identity)?;
            return Ok(());
        }

        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        // NamedTempFile owns only the file it created; error/unwind cleanup
        // cannot remove another process's staging file after a name collision.
        let mut builder = tempfile::Builder::new();
        builder.prefix(".blobray-output-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Match ordinary generated-file creation; the OS applies umask.
            builder.permissions(fs::Permissions::from_mode(0o666));
        }
        let mut stage = builder.tempfile_in(parent)?;
        let mut writer = DigestWriter::new(BufWriter::new(stage.as_file_mut()));
        serialize(&mut writer)?;
        writer.flush()?;
        let (buffer, identity) = writer.finish();
        drop(buffer);
        self.validate_identity(expected, &identity)?;
        stage.as_file().sync_all()?;
        stage
            .persist(self.path)
            .map_err(|error| crate::Error::from(error.error))?;
        self.trace(&identity, "published");
        self.complete(identity)?;
        Ok(())
    }

    fn complete(&self, identity: ContentIdentity) -> Result<()> {
        if let Some(completed) = self.completed {
            completed
                .set(identity)
                .map_err(|_| crate::Error::invalid("output receipt was already completed"))?;
        }
        Ok(())
    }

    fn validate_identity(
        &self,
        expected: Option<&ContentIdentity>,
        actual: &ContentIdentity,
    ) -> Result<()> {
        if let Some(expected) = expected
            && expected != actual
        {
            return Err(crate::Error::invalid(format!(
                "generated {} failed content identity before publication: expected {} bytes with digest {}, got {} bytes with digest {}",
                self.kind, expected.bytes, expected.sha256, actual.bytes, actual.sha256
            )));
        }
        Ok(())
    }

    fn trace(&self, identity: &ContentIdentity, outcome: &str) {
        tracing::debug!(path = %self.path.display(), bytes = identity.bytes,
            sha256 = %identity.sha256, outcome, kind = self.kind, "generated output");
    }
}

struct DigestWriter<W> {
    inner: W,
    digest: Sha256,
    bytes: u64,
}

impl<W> DigestWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
            bytes: 0,
        }
    }
    fn finish(self) -> (W, ContentIdentity) {
        (
            self.inner,
            ContentIdentity {
                bytes: self.bytes,
                sha256: format!("{:x}", self.digest.finalize()),
            },
        )
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.digest.update(&buffer[..written]);
        self.bytes = self
            .bytes
            .checked_add(written as u64)
            .ok_or_else(|| std::io::Error::other("generated output length overflow"))?;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct CompareWriter<R> {
    inner: R,
    equal: bool,
}

impl<R: Read> CompareWriter<R> {
    fn new(inner: R) -> Self {
        Self { inner, equal: true }
    }
    fn finish(mut self) -> std::io::Result<bool> {
        let mut trailing = [0_u8; 1];
        Ok(self.equal && self.inner.read(&mut trailing)? == 0)
    }
}

impl<R: Read> Write for CompareWriter<R> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if self.equal {
            let mut offset = 0;
            let mut existing = [0_u8; 64 * 1024];
            while offset < buffer.len() {
                let length = (buffer.len() - offset).min(existing.len());
                match self.inner.read_exact(&mut existing[..length]) {
                    Ok(()) => {
                        if existing[..length] != buffer[offset..offset + length] {
                            self.equal = false;
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                        self.equal = false;
                        break;
                    }
                    Err(error) => return Err(error),
                }
                offset += length;
            }
        }
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_never_creates_or_updates_an_output() {
        let directory =
            std::env::temp_dir().join(format!("blobray-generated-output-{}", std::process::id()));
        let path = directory.join("nested/report.txt");
        let missing = GeneratedOutput::new(&path, true, "fixture")
            .text("expected\n")
            .unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("cannot check generated fixture")
        );
        assert!(!path.exists());

        GeneratedOutput::new(&path, false, "fixture")
            .text("original\n")
            .unwrap();
        let stale = GeneratedOutput::new(&path, true, "fixture")
            .text("changed\n")
            .unwrap_err();
        assert!(stale.to_string().contains("rerun without --check"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");

        GeneratedOutput::new(&path, true, "fixture")
            .text("original\n")
            .unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn binary_check_is_atomic_and_exact() {
        let directory =
            std::env::temp_dir().join(format!("blobray-generated-binary-{}", std::process::id()));
        let path = directory.join("nested/report.bin");
        GeneratedOutput::new(&path, false, "fixture binary")
            .bytes(b"\x00expected\xff")
            .unwrap();
        GeneratedOutput::new(&path, true, "fixture binary")
            .bytes(b"\x00expected\xff")
            .unwrap();
        let error = GeneratedOutput::new(&path, true, "fixture binary")
            .bytes(b"\x00changed\xff")
            .unwrap_err();
        assert!(error.to_string().contains("rerun without --check"));
        assert_eq!(fs::read(&path).unwrap(), b"\x00expected\xff");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn streamed_json_is_atomic_and_checkable() {
        let directory =
            std::env::temp_dir().join(format!("blobray-generated-json-{}", std::process::id()));
        let path = directory.join("nested/report.json");
        let document = serde_json::json!({"schema": 1, "items": [1, 2, 3]});
        GeneratedOutput::new(&path, false, "fixture JSON")
            .json(&document, true)
            .unwrap();
        GeneratedOutput::new(&path, true, "fixture JSON")
            .json(&document, true)
            .unwrap();
        let stale = serde_json::json!({"schema": 1, "items": [1, 2, 4]});
        let error = GeneratedOutput::new(&path, true, "fixture JSON")
            .json(&stale, true)
            .unwrap_err();
        assert!(error.to_string().contains("rerun without --check"));
        GeneratedOutput::new(&path, true, "fixture JSON")
            .json(&document, true)
            .unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("\n"));
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_or_unwound_emission_keeps_previous_file_and_cleans_only_its_stage() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result");
        let foreign = directory.path().join(".blobray-output-foreign");
        fs::write(&output, b"previous").unwrap();
        fs::write(&foreign, b"another writer").unwrap();
        let partial = vec![b'x'; 128 * 1024];
        let failed = GeneratedOutput::new(&output, false, "failure fixture").emit(None, |writer| {
            writer.write_all(&partial)?;
            Err(crate::Error::invalid("encoder failed after output"))
        });
        assert!(failed.unwrap_err().to_string().contains("encoder failed"));
        let unwound = std::panic::catch_unwind(|| {
            let _ = GeneratedOutput::new(&output, false, "panic fixture").emit(None, |writer| {
                writer.write_all(&partial)?;
                panic!("encoder unwound after output");
            });
        });
        assert!(unwound.is_err());
        assert_eq!(fs::read(&output).unwrap(), b"previous");
        assert_eq!(fs::read(&foreign).unwrap(), b"another writer");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn verified_stream_rejects_bad_digest_and_truncation_before_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result");
        fs::write(&output, b"previous").unwrap();
        let digest = format!("{:x}", Sha256::digest(b"expected"));
        for contents in [b"corrupt!".as_slice(), b"short".as_slice()] {
            let error = GeneratedOutput::new(&output, false, "cached output")
                .verified_stream(contents, 8, &digest)
                .unwrap_err();
            assert!(error.to_string().contains("failed content identity"));
            assert_eq!(fs::read(&output).unwrap(), b"previous");
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
        let mut framed = std::io::Cursor::new(b"expectednext-object");
        GeneratedOutput::new(&output, false, "cached output")
            .verified_stream(&mut framed, 8, &digest)
            .unwrap();
        assert_eq!(framed.position(), 8);
        assert_eq!(fs::read(&output).unwrap(), b"expected");
    }

    #[test]
    fn check_stream_never_stages_and_rejects_trailing_or_missing_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result");
        fs::write(&output, b"expected").unwrap();
        let modified = fs::metadata(&output).unwrap().modified().unwrap();
        for contents in [b"expect".as_slice(), b"expected-extra".as_slice()] {
            assert!(
                GeneratedOutput::new(&output, true, "check fixture")
                    .bytes(contents)
                    .is_err()
            );
        }
        let digest = format!("{:x}", Sha256::digest(b"expected"));
        GeneratedOutput::new(&output, true, "check fixture")
            .verified_stream(b"expected".as_slice(), 8, &digest)
            .unwrap();
        assert_eq!(fs::metadata(&output).unwrap().modified().unwrap(), modified);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let missing = directory.path().join("missing/child");
        assert!(
            GeneratedOutput::new(&missing, true, "check fixture")
                .bytes(b"")
                .is_err()
        );
        assert!(!missing.parent().unwrap().exists());
    }

    #[test]
    fn failed_replacement_cleans_stage_and_preserves_destination_directory() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("existing"), b"preserved").unwrap();
        assert!(
            GeneratedOutput::new(&output, false, "failure fixture")
                .text("new")
                .is_err()
        );
        assert_eq!(fs::read(output.join("existing")).unwrap(), b"preserved");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn text_publication_replaces_destination_without_mutating_link_targets_or_open_readers() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"original").unwrap();
        for symbolic in [false, true] {
            if symbolic {
                std::os::unix::fs::symlink(&source, &destination).unwrap();
            } else {
                fs::hard_link(&source, &destination).unwrap();
            }
            let mut existing_reader = fs::File::open(&destination).unwrap();
            GeneratedOutput::new(&destination, false, "text fixture")
                .text("replacement")
                .unwrap();
            assert_eq!(fs::read(&source).unwrap(), b"original");
            assert_eq!(fs::read(&destination).unwrap(), b"replacement");
            let mut old_bytes = Vec::new();
            existing_reader.read_to_end(&mut old_bytes).unwrap();
            assert_eq!(old_bytes, b"original");
            fs::remove_file(&destination).unwrap();
        }
    }
}
