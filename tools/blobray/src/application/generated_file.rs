//! Shared write/check policy for project-owned generated artifacts.

use std::{
    fs,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Result;

static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_or_check(path: &Path, contents: &str, check: bool, kind: &str) -> Result<()> {
    if check {
        let existing = fs::read_to_string(path)
            .map_err(|error| format!("cannot check generated {kind} {}: {error}", path.display()))
            .map_err(crate::Error::invalid)?;
        if existing != contents {
            return Err(crate::Error::invalid(format!(
                "generated {kind} differs from {}; rerun without --check",
                path.display()
            )));
        }
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

/// Serialize a generated JSON document without buffering a second full copy.
///
/// Write mode hashes into a sibling staged file and publishes it atomically.
/// Check mode compares serialized chunks directly with the existing file; it
/// does not require free space in the system temporary directory or write to
/// the project tree.
pub(crate) fn write_or_check_json<T: Serialize>(
    path: &Path,
    document: &T,
    check: bool,
    kind: &str,
    pretty: bool,
) -> Result<()> {
    if check && !path.is_file() {
        return Err(crate::Error::invalid(format!(
            "cannot check generated {kind} {}: file does not exist",
            path.display()
        )));
    }
    if check {
        return check_json(path, document, kind, pretty);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stage = stage_path(path);
    let result = (|| -> Result<()> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        let mut writer = DigestWriter::new(BufWriter::new(file));
        if pretty {
            serde_json::to_writer_pretty(&mut writer, document)?;
        } else {
            serde_json::to_writer(&mut writer, document)?;
        }
        writer.write_all(b"\n")?;
        writer.flush()?;
        let (bytes, sha256) = writer.finish();
        tracing::debug!(
            path = %path.display(),
            bytes,
            sha256 = %sha256,
            "staged generated JSON"
        );

        fs::rename(&stage, path)?;
        Ok(())
    })();
    if stage.exists() {
        let _ = fs::remove_file(&stage);
    }
    result
}

fn check_json<T: Serialize>(path: &Path, document: &T, kind: &str, pretty: bool) -> Result<()> {
    let file = fs::File::open(path)?;
    let mut writer = DigestCompareWriter::new(BufReader::new(file));
    if pretty {
        serde_json::to_writer_pretty(&mut writer, document)?;
    } else {
        serde_json::to_writer(&mut writer, document)?;
    }
    writer.write_all(b"\n")?;
    let (bytes, sha256, equal) = writer.finish()?;
    tracing::debug!(
        path = %path.display(),
        bytes,
        sha256 = %sha256,
        "stream-checked generated JSON"
    );
    if !equal {
        return Err(crate::Error::invalid(format!(
            "generated {kind} differs from {}; rerun without --check",
            path.display()
        )));
    }
    Ok(())
}

fn stage_path(path: &Path) -> PathBuf {
    let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("generated");
    let name = format!(".{name}.stage-{}-{sequence}", std::process::id());
    path.with_file_name(name)
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

    fn finish(self) -> (u64, String) {
        (self.bytes, format!("{:x}", self.digest.finalize()))
    }
}

impl<W: Write> Write for DigestWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.digest.update(&buffer[..written]);
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct DigestCompareWriter<R> {
    inner: R,
    digest: Sha256,
    bytes: u64,
    equal: bool,
}

impl<R: Read> DigestCompareWriter<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
            bytes: 0,
            equal: true,
        }
    }

    fn finish(mut self) -> std::io::Result<(u64, String, bool)> {
        let mut trailing = [0_u8; 1];
        let no_trailing_bytes = self.inner.read(&mut trailing)? == 0;
        Ok((
            self.bytes,
            format!("{:x}", self.digest.finalize()),
            self.equal && no_trailing_bytes,
        ))
    }
}

impl<R: Read> Write for DigestCompareWriter<R> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.digest.update(buffer);
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
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
        let missing = write_or_check(&path, "expected\n", true, "fixture").unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("cannot check generated fixture")
        );
        assert!(!path.exists());

        write_or_check(&path, "original\n", false, "fixture").unwrap();
        let stale = write_or_check(&path, "changed\n", true, "fixture").unwrap_err();
        assert!(stale.to_string().contains("rerun without --check"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");

        write_or_check(&path, "original\n", true, "fixture").unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn streamed_json_is_atomic_and_checkable() {
        let directory =
            std::env::temp_dir().join(format!("blobray-generated-json-{}", std::process::id()));
        let path = directory.join("nested/report.json");
        let document = serde_json::json!({"schema": 1, "items": [1, 2, 3]});
        write_or_check_json(&path, &document, false, "fixture JSON", true).unwrap();
        write_or_check_json(&path, &document, true, "fixture JSON", true).unwrap();
        let stale = serde_json::json!({"schema": 1, "items": [1, 2, 4]});
        let error = write_or_check_json(&path, &stale, true, "fixture JSON", true).unwrap_err();
        assert!(error.to_string().contains("rerun without --check"));
        write_or_check_json(&path, &document, true, "fixture JSON", true).unwrap();
        assert!(fs::read_to_string(&path).unwrap().ends_with("\n"));
        assert!(fs::read_dir(path.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".stage-")
        }));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
