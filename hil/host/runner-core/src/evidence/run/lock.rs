//! Coordinate run-directory publication and snapshots of the shared run index.

use crate::Result;
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::Path,
    time::{Duration, Instant},
};

pub struct IndexGuard(File);

/// How long a runner waits for another to finish publishing: a history
/// rebuild over a large store takes minutes.
const PUBLICATION_WAIT: Duration = Duration::from_secs(600);

impl IndexGuard {
    pub fn acquire(directory: &Path) -> Result<Self> {
        let path = directory.join("index.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let started = Instant::now();
        let mut announced = false;
        loop {
            oer_process::check_cancelled()?;
            match file.try_lock_exclusive() {
                Ok(()) => {
                    // Name this process for whoever waits next.
                    let _ = file.set_len(0);
                    let _ = (&file).write_all(std::process::id().to_string().as_bytes());
                    return Ok(Self(file));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let waited = started.elapsed();
                    if waited >= PUBLICATION_WAIT {
                        return Err(format!(
                            "the run index stayed locked for {}s{}",
                            waited.as_secs(),
                            holder(&path)
                        )
                        .into());
                    }
                    if !announced && waited >= Duration::from_secs(1) {
                        eprintln!("hil: waiting for the run index{}", holder(&path));
                        announced = true;
                    }
                    oer_process::sleep(Duration::from_millis(20))?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// `" (held by pid N)"` from the lock file, when its holder wrote itself.
fn holder(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .map(|pid| format!(" (held by pid {pid})"))
        .unwrap_or_default()
}

impl Drop for IndexGuard {
    fn drop(&mut self) {
        // Release the logical owner even if a concurrent fork retained an fd.
        let _ = FileExt::unlock(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_waiter_learns_who_holds_the_index() {
        let directory = tempfile::tempdir().unwrap();
        let guard = IndexGuard::acquire(directory.path()).unwrap();
        assert_eq!(
            holder(&directory.path().join("index.lock")),
            format!(" (held by pid {})", std::process::id())
        );
        drop(guard);
        // Released, it is taken again at once.
        IndexGuard::acquire(directory.path()).unwrap();
    }
}
