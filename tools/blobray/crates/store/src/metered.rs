//! Budget-aware I/O; all buffers and syscall portions are bounded.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

pub(crate) fn read_payload(path: &Path, control: &mut dyn RunControl) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(io)?;
    let length = file.metadata().map_err(io)?.len();
    if length > DEFAULT_WORKING_BYTES {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "materialized payload exceeds default working capacity; use open_payload",
        ));
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(
            usize::try_from(length).map_err(|_| {
                Error::new(ErrorCode::ResourceLimited, "payload exceeds address space")
            })?,
        )
        .map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "host allocation refused materialized payload",
            )
        })?;
    let mut remaining = length;
    let mut buffer = [0; WORK_BLOCK];
    while remaining != 0 {
        let size = remaining.min(WORK_BLOCK as u64) as usize;
        control.bytes(size)?;
        file.read_exact(&mut buffer[..size]).map_err(io)?;
        control.bytes(size)?;
        output.extend_from_slice(&buffer[..size]);
        remaining -= size as u64;
    }
    if file.metadata().map_err(io)?.len() != length {
        return Err(integrity("payload changed during read"));
    }
    Ok(output)
}

pub(crate) fn hash_file(path: &Path, control: &mut dyn RunControl) -> Result<(ArtifactId, u64)> {
    if !fs::symlink_metadata(path).map_err(io)?.is_file() {
        return Err(integrity("retained payload is not a regular file"));
    }
    let mut file = File::open(path).map_err(io)?;
    let length = file.metadata().map_err(io)?.len();
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0; WORK_BLOCK];
    while total < length {
        let size = (length - total).min(WORK_BLOCK as u64) as usize;
        control.bytes(size)?;
        file.read_exact(&mut buffer[..size]).map_err(io)?;
        control.bytes(size)?;
        hash.update(&buffer[..size]);
        total += size as u64;
    }
    if file.metadata().map_err(io)?.len() != length {
        return Err(integrity("payload changed during hash"));
    }
    Ok((format!("{:x}", hash.finalize()).parse()?, total))
}

/// Restore the original typed control error after serde wraps it as an I/O error.
pub(crate) fn write_json<T: serde::Serialize>(
    file: &mut dyn Write,
    value: &T,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut failure = None;
    let mut writer = MeteredWriter {
        file,
        control,
        failure: &mut failure,
    };
    let result = {
        let mut buffered = std::io::BufWriter::with_capacity(WORK_BLOCK, &mut writer);
        serde_json::to_writer(&mut buffered, value)
            .and_then(|()| buffered.flush().map_err(serde_json::Error::io))
    };
    if let Some(error) = failure {
        return Err(error);
    }
    result.map_err(super::jobs::json)
}
struct MeteredWriter<'a> {
    file: &'a mut dyn Write,
    control: &'a mut dyn RunControl,
    failure: &'a mut Option<Error>,
}
impl Write for MeteredWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let size = bytes.len().min(WORK_BLOCK);
        if let Err(error) = self.control.bytes(size) {
            *self.failure = Some(error.clone());
            return Err(std::io::Error::other(error));
        }
        self.file.write(&bytes[..size]).inspect_err(|error| {
            *self.failure = Some(storage_io(std::io::Error::new(
                error.kind(),
                error.to_string(),
            )));
            if let Some(typed) = error.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
                *self.failure = Some(typed.clone());
            }
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub(crate) struct CheckedReader<'a> {
    pub file: File,
    pub remaining: u64,
    pub control: &'a mut dyn RunControl,
    pub failure: &'a mut Option<Error>,
}
impl Read for CheckedReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let size = bytes
            .len()
            .min(WORK_BLOCK)
            .min(self.remaining.min(WORK_BLOCK as u64) as usize);
        if let Err(error) = self.control.bytes(size) {
            *self.failure = Some(error.clone());
            return Err(std::io::Error::other(error));
        }
        self.file.read_exact(&mut bytes[..size])?;
        self.remaining -= size as u64;
        Ok(size)
    }
}
