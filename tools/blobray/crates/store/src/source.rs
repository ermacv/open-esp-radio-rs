//! Verified file handles with bounded positional reads; no materialized payload.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    os::unix::fs::{FileExt, MetadataExt},
    sync::Mutex,
};

/// Positional read size. Accounting stays per chunk; larger chunks only reduce
/// system calls for verification and sequential scans.
const READ_CHUNK: usize = STREAM_BLOCK;

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

/// A retained file's identity when this process verified its digest. Every
/// write changes the modification and status-change times, so an unchanged
/// stamp names the verified bytes and the file needs no second hash.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Stamp {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}
/// Retained files this process verified, by path, with the digest they had.
static VERIFIED: Mutex<BTreeMap<PathBuf, (ArtifactId, Stamp)>> = Mutex::new(BTreeMap::new());

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
        let metadata = file.metadata().map_err(io)?;
        let length = metadata.len();
        let stamp = Stamp::of(&metadata);
        let lease = Self {
            file: RefCell::new(file),
            length,
        };
        let known = VERIFIED
            .lock()
            .map_err(|_| integrity("verified payload index poisoned"))?
            .get(path)
            .is_some_and(|(id, verified)| id == expected && *verified == stamp);
        if known {
            // Charge the same work as the reads and hash below, chunk by
            // chunk, so an operation's accounting does not depend on what
            // this process opened before.
            let mut offset = 0;
            while offset < length {
                let count = (length - offset).min(READ_CHUNK as u64) as usize;
                control.bytes(count)?;
                control.bytes(count)?;
                offset += count as u64;
            }
            return Ok(lease);
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0; READ_CHUNK];
        let mut offset = 0;
        while offset < length {
            let count = (length - offset).min(READ_CHUNK as u64) as usize;
            lease.read_at(offset, &mut buffer[..count], control)?;
            control.bytes(count)?;
            digest.update(&buffer[..count]);
            offset += count as u64;
        }
        let actual: ArtifactId = format!("{:x}", digest.finalize()).parse()?;
        let after = Stamp::of(&lease.file.borrow().metadata().map_err(io)?);
        if actual != *expected || after != stamp {
            return Err(integrity(format!(
                "retained object {expected} failed integrity verification"
            )));
        }
        VERIFIED
            .lock()
            .map_err(|_| integrity("verified payload index poisoned"))?
            .insert(path.to_path_buf(), (actual, stamp));
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
        let file = self.file.borrow();
        let mut position = offset;
        for chunk in bytes.chunks_mut(READ_CHUNK) {
            control.bytes(chunk.len())?;
            file.read_exact_at(chunk, position).map_err(retained_io)?;
            position += chunk.len() as u64;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewrite_after_verification_is_detected_on_the_next_open() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-lease-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("payload");
        let bytes = b"retained payload".to_vec();
        fs::write(&path, &bytes).unwrap();
        let id: ArtifactId = format!("{:x}", Sha256::digest(&bytes)).parse().unwrap();
        FileLease::open(&path, &id, &mut || Ok(())).unwrap();
        FileLease::open(&path, &id, &mut || Ok(())).unwrap();
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        fs::write(&path, &corrupt).unwrap();
        let error = FileLease::open(&path, &id, &mut || Ok(())).err().unwrap();
        assert_eq!(error.code, ErrorCode::Integrity);
        fs::remove_dir_all(&directory).unwrap();
    }
}
