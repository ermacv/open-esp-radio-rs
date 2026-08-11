//! Exclusive host ownership of the physical HIL fixture.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use fs2::FileExt;

use crate::Result;

/// Keeps the process-wide file lock alive until the hardware command returns.
pub(crate) struct FixtureLock {
    _file: File,
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

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(
            file,
            "pid={} command={}",
            std::process::id(),
            command_line()
        )?;
        file.flush()?;
        Ok(Self { _file: file })
    }
}

fn command_line() -> String {
    std::env::args().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn fixture_has_exactly_one_live_host_owner() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "open-radio-fixture-lock-{}-{nonce}",
            std::process::id()
        ));

        let owner = FixtureLock::acquire(&root).unwrap();
        assert!(FixtureLock::acquire(&root).is_err());
        drop(owner);
        FixtureLock::acquire(&root).unwrap();

        fs::remove_dir_all(root).unwrap();
    }
}
