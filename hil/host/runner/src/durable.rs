//! Durable host files shared by every evidence producer: atomic replacement,
//! content digests and wall-clock timestamps.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Result;

/// Distinguishes concurrent temporary files written by one process.
pub(crate) static UNIQUE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("report path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let counter = UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("report path has no file name: {}", path.display()))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn unix_millis() -> Result<u64> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    u64::try_from(millis).map_err(|_| "host timestamp exceeds the HIL report range".into())
}
