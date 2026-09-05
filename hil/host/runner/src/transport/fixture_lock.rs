//! Exclusive host ownership of the physical HIL fixture.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use fs2::FileExt;

use crate::Result;

/// Holds exclusive fixture ownership until the hardware command returns.
pub(crate) struct FixtureLock {
    file: File,
}

impl FixtureLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
        let directory = root.join("target/hil/esp32s31");
        fs::create_dir_all(&directory)?;
        let path = directory.join("fixture.lock");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        if let Err(error) = file.try_lock_exclusive() {
            let mut owner = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut owner)?;
            let owner = owner.trim();
            let detail = if owner.is_empty() {
                "another HIL process".to_owned()
            } else {
                owner.to_owned()
            };
            return Err(format!(
                "physical HIL fixture is already owned by {detail} ({}): {error}",
                path.display()
            )
            .into());
        }

        // Establish the guard before fallible owner metadata writes so every
        // path after successful acquisition explicitly releases its authority.
        let mut owner = Self { file };
        let file = &mut owner.file;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(
            file,
            "pid={} command={}",
            std::process::id(),
            command_line()
        )?;
        file.flush()?;
        Ok(owner)
    }
}

impl Drop for FixtureLock {
    fn drop(&mut self) {
        // A concurrent fork inherits this open file description until exec,
        // even with close-on-exec set. Closing only our descriptor can leave
        // flock held by that child after the hardware owner has returned.
        // Release at the logical owner boundary; File still closes afterward.
        let _ = FileExt::unlock(&self.file);
    }
}

fn command_line() -> String {
    std::env::args().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
