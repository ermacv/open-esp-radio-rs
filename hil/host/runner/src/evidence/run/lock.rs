//! Coordinate run-directory publication and snapshots of the shared run index.

use crate::Result;
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct IndexGuard(File);

impl IndexGuard {
    pub(crate) fn acquire(directory: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("index.lock"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            oer_process::check_cancelled()?;
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("run index publication is busy".into());
                    }
                    oer_process::sleep(Duration::from_millis(20))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for IndexGuard {
    fn drop(&mut self) {
        // Release the logical owner even if a concurrent fork retained an fd.
        let _ = FileExt::unlock(&self.0);
    }
}
