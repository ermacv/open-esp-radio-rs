//! Host-owned persistence for one opaque HIL startup artifact.

use std::{
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::Path,
};

use open_esp_radio_hil_protocol::{
    STARTUP_ARTIFACT_CHUNK_MAX_LEN, StartupArtifactChunk, startup_artifact_crc32c,
};

use crate::Result;

pub(crate) fn load_if_present(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => {
            Err(format!("startup artifact `{}` is empty", path.display()).into())
        }
        Ok(bytes) if bytes.len() > usize::from(u16::MAX) => Err(format!(
            "startup artifact `{}` exceeds the protocol length limit",
            path.display()
        )
        .into()),
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(format!("cannot read startup artifact `{}`: {error}", path.display()).into())
        }
    }
}

pub(crate) fn chunks(bytes: &[u8]) -> Result<Vec<StartupArtifactChunk>> {
    let total_length = u16::try_from(bytes.len())
        .map_err(|_| "startup artifact exceeds the protocol length limit")?;
    if total_length == 0 {
        return Err("startup artifact must not be empty".into());
    }
    let checksum = startup_artifact_crc32c(bytes);
    bytes
        .chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN)
        .enumerate()
        .map(|(index, part)| {
            let offset = u16::try_from(index * STARTUP_ARTIFACT_CHUNK_MAX_LEN)
                .map_err(|_| "startup artifact chunk offset overflow")?;
            StartupArtifactChunk::try_new(total_length, offset, checksum, part)
                .map_err(|error| format!("cannot frame startup artifact: {error}").into())
        })
        .collect()
}

pub(crate) struct Assembler {
    bytes: Vec<u8>,
    total_length: Option<usize>,
    expected_crc32c: u32,
}

impl Assembler {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            total_length: None,
            expected_crc32c: 0,
        }
    }

    pub(crate) fn push(&mut self, chunk: &StartupArtifactChunk) -> Result<Option<Vec<u8>>> {
        chunk
            .validate()
            .map_err(|error| format!("target returned an invalid startup artifact: {error}"))?;
        if chunk.offset() == 0 {
            self.bytes.clear();
            self.bytes.reserve_exact(usize::from(chunk.total_length()));
            self.total_length = Some(usize::from(chunk.total_length()));
            self.expected_crc32c = chunk.crc32c();
        }
        if self.total_length != Some(usize::from(chunk.total_length()))
            || self.expected_crc32c != chunk.crc32c()
            || self.bytes.len() != usize::from(chunk.offset())
        {
            return Err("target returned non-contiguous startup artifact chunks".into());
        }
        self.bytes.extend_from_slice(chunk.bytes());
        if !chunk.is_final() {
            return Ok(None);
        }
        if self.bytes.len() != self.total_length.unwrap_or_default()
            || startup_artifact_crc32c(&self.bytes) != self.expected_crc32c
        {
            return Err("target startup artifact checksum mismatch".into());
        }
        Ok(Some(std::mem::take(&mut self.bytes)))
    }
}

pub(crate) fn persist_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        format!(
            "startup artifact path `{}` has no file name",
            path.display()
        )
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot persist startup artifact `{}`: {error}",
            path.display()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_assembly_preserves_non_aligned_artifact() {
        let bytes = (0..777).map(|index| index as u8).collect::<Vec<_>>();
        let chunks = chunks(&bytes).unwrap();
        assert_eq!(
            chunks.len(),
            bytes.len().div_ceil(STARTUP_ARTIFACT_CHUNK_MAX_LEN)
        );
        let mut assembler = Assembler::new();
        let mut completed = None;
        for chunk in &chunks {
            completed = assembler.push(chunk).unwrap().or(completed);
        }
        assert_eq!(completed.as_deref(), Some(bytes.as_slice()));
    }

    #[test]
    fn assembly_rejects_missing_middle_chunk() {
        let bytes = vec![0xa5; 900];
        let chunks = chunks(&bytes).unwrap();
        let mut assembler = Assembler::new();
        assert!(assembler.push(&chunks[0]).unwrap().is_none());
        assert!(assembler.push(&chunks[2]).is_err());
    }

    #[test]
    fn assembly_rejects_complete_artifact_with_wrong_digest() {
        let bytes = vec![0x3c; 500];
        let wrong_checksum = startup_artifact_crc32c(&bytes) ^ 1;
        let mut assembler = Assembler::new();
        for (index, part) in bytes.chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN).enumerate() {
            let chunk = StartupArtifactChunk::try_new(
                500,
                u16::try_from(index * STARTUP_ARTIFACT_CHUNK_MAX_LEN).unwrap(),
                wrong_checksum,
                part,
            )
            .unwrap();
            if chunk.is_final() {
                assert!(assembler.push(&chunk).is_err());
            } else {
                assert!(assembler.push(&chunk).unwrap().is_none());
            }
        }
    }
}
