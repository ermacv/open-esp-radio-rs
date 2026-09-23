//! Verified file handles with bounded positional reads; no materialized payload.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    io::{Read, Seek, SeekFrom},
};

fn retained_io(error: std::io::Error) -> Error {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof
    ) {
        integrity(format!("retained payload missing or truncated: {error}"))
    } else {
        io(error)
    }
}

pub struct FileLease {
    file: RefCell<File>,
    length: u64,
}
impl FileLease {
    pub(crate) fn open(
        path: &Path,
        expected: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        if !fs::symlink_metadata(path).map_err(retained_io)?.is_file() {
            return Err(integrity("retained payload is not a regular file"));
        }
        let file = File::open(path).map_err(retained_io)?;
        let length = file.metadata().map_err(io)?.len();
        let lease = Self {
            file: RefCell::new(file),
            length,
        };
        let mut digest = Sha256::new();
        let mut buffer = [0; WORK_BLOCK];
        let mut offset = 0;
        while offset < length {
            let count = (length - offset).min(WORK_BLOCK as u64) as usize;
            lease.read_at(offset, &mut buffer[..count], control)?;
            control.bytes(count)?;
            digest.update(&buffer[..count]);
            offset += count as u64;
        }
        let actual: ArtifactId = format!("{:x}", digest.finalize()).parse()?;
        if actual != *expected || lease.file.borrow().metadata().map_err(io)?.len() != length {
            return Err(integrity(format!(
                "retained object {expected} failed integrity verification"
            )));
        }
        Ok(lease)
    }
}
impl ByteSource for FileLease {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(integrity("captured byte range out of bounds"));
        }
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        for chunk in bytes.chunks_mut(WORK_BLOCK) {
            control.bytes(chunk.len())?;
            file.read_exact(chunk).map_err(retained_io)?;
        }
        Ok(())
    }
}
impl Staging {
    pub fn open_payload(&self, id: &ArtifactId, control: &mut dyn RunControl) -> Result<FileLease> {
        FileLease::open(&self.object_path(id), id, control)
    }
}
impl Project {
    pub fn open_payload(&self, id: &ArtifactId, control: &mut dyn RunControl) -> Result<FileLease> {
        FileLease::open(&self.object_path(id), id, control)
    }
}
