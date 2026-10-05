//! The arbiter's final exclusion layer: `flock` lock files, one per board
//! and per fixture resource, in the stand model's lock directory
//! (`open-esp-radio/leases` in the XDG cache directory).
//!
//! A grant orders access; the lock files exclude every other process of the
//! host from a board or fixture while it is used, including one that did not
//! queue (an older checkout) or one of a private arbiter. A lock is taken
//! only once the grant that ordered it is held, and released before it. A
//! board's lock file is named by its MAC, so every alias of its port
//! (`/dev/ttyACM*`, `/dev/serial/by-id/…`) shares one owner.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use fs2::FileExt;

/// Another process holds the lock.
#[derive(Debug)]
pub struct Busy(String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

/// An exclusive lock file, released when dropped.
struct LockFile {
    file: File,
}

impl LockFile {
    fn try_acquire(path: &Path, what: &str) -> crate::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        if file.try_lock_exclusive().is_err() {
            let mut owner = String::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_string(&mut owner)?;
            let owner = owner.trim();
            return Err(Busy(format!(
                "{what} is held by {} ({})",
                if owner.is_empty() {
                    "another process"
                } else {
                    owner
                },
                path.display()
            ))
            .into());
        }
        // The guard exists before the fallible owner record, so every path
        // after the acquisition releases it.
        let mut lock = Self { file };
        lock.file.set_len(0)?;
        lock.file.seek(SeekFrom::Start(0))?;
        writeln!(
            lock.file,
            "pid={} command={}",
            std::process::id(),
            std::env::args().collect::<Vec<_>>().join(" ")
        )?;
        lock.file.flush()?;
        Ok(lock)
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        // A concurrent fork inherits this open file description until exec,
        // even with close-on-exec set. Closing only our descriptor can leave
        // flock held by that child after the owner returned: release at the
        // logical owner boundary; the file still closes afterwards.
        let _ = FileExt::unlock(&self.file);
    }
}

/// A board's lock, by its MAC.
pub struct BoardLock {
    mac: String,
    _file: LockFile,
}

impl BoardLock {
    /// Lock the board with `mac` now, or fail with [`Busy`].
    pub fn try_acquire(mac: &str) -> crate::Result<Self> {
        Self::try_acquire_in(&oer_hil_stand_model::paths::locks()?, mac)
    }

    /// Lock the board with `mac`, waiting while a process outside the
    /// arbiter's order still holds it.
    pub fn acquire(mac: &str) -> crate::Result<Self> {
        wait_while_busy(|| Self::try_acquire(mac))
    }

    /// [`Self::try_acquire`] with the lock files in `directory`: a test's
    /// own.
    pub fn try_acquire_in(directory: &Path, mac: &str) -> crate::Result<Self> {
        let mac = oer_hil_stand_model::mac::normalize(mac)?;
        let path = directory.join(format!(
            "board-{}.lock",
            oer_hil_stand_model::mac::compact(&mac)
        ));
        Ok(Self {
            _file: LockFile::try_acquire(&path, &format!("board {mac}"))?,
            mac,
        })
    }

    /// The locked board's MAC.
    pub fn mac(&self) -> &str {
        &self.mac
    }
}

/// A fixture resource's lock, by its key (a Bluetooth adapter, a radio, an
/// OpenWrt host's boot).
pub struct ResourceLock {
    _file: LockFile,
}

impl ResourceLock {
    /// Lock the resource `key` now, or fail with [`Busy`].
    pub fn try_acquire(key: &str) -> crate::Result<Self> {
        Self::try_acquire_in(&oer_hil_stand_model::paths::locks()?, key)
    }

    /// Lock the resource `key`, waiting while a process outside the
    /// arbiter's order still holds it.
    pub fn acquire(key: &str) -> crate::Result<Self> {
        wait_while_busy(|| Self::try_acquire(key))
    }

    /// [`Self::try_acquire`] with the lock files in `directory`: a test's
    /// own.
    pub fn try_acquire_in(directory: &Path, key: &str) -> crate::Result<Self> {
        Ok(Self {
            _file: LockFile::try_acquire(
                &resource_path(directory, key),
                &format!("the HIL fixture resource `{key}`"),
            )?,
        })
    }
}

fn resource_path(directory: &Path, key: &str) -> PathBuf {
    directory
        .join(format!(
            "resource-{}",
            oer_durable::sha256_bytes(key.as_bytes())
        ))
        .join("fixture.lock")
}

/// Retry `lock` while a process outside the arbiter (for example a checkout
/// without it) still holds what it locks; the wait is reported once.
pub fn wait_while_busy<T>(mut lock: impl FnMut() -> crate::Result<T>) -> crate::Result<T> {
    let mut reported = false;
    loop {
        match lock() {
            Err(error) if error.is::<Busy>() => {
                if !reported {
                    eprintln!("hil-arbiter: waiting for a lock held outside the queue: {error}");
                    reported = true;
                }
                oer_process::sleep(std::time::Duration::from_secs(1))?;
            }
            result => return result,
        }
    }
}

/// A board held for one command: the stand's lease on it, then its lock.
pub struct BoardLease {
    // Declared first so the lock is released before the lease that ordered it.
    pub lock: BoardLock,
    pub grant: crate::Grant,
}

impl crate::Arbiter {
    /// Wait for `request`'s lease, which must claim the board with `mac`,
    /// then lock that board.
    pub fn lease_board(&self, request: &crate::Request, mac: &str) -> crate::Result<BoardLease> {
        let board = crate::Claim::board(mac);
        if !request
            .claims
            .iter()
            .any(|claim| claim.resource == board.resource)
        {
            return Err(format!("a lease of board {mac} must claim it").into());
        }
        let grant = self.acquire(request)?;
        Ok(BoardLease {
            lock: BoardLock::acquire(mac)?,
            grant,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_has_one_lock_whatever_its_mac_s_spelling() {
        let directory = tempfile::tempdir().unwrap();
        let held = BoardLock::try_acquire_in(directory.path(), "38:44:be:aa:25:64").unwrap();
        assert_eq!(held.mac(), "38:44:BE:AA:25:64");
        let busy = BoardLock::try_acquire_in(directory.path(), "3844BEAA2564")
            .err()
            .unwrap();
        assert!(busy.is::<Busy>(), "{busy}");
        assert!(busy.to_string().contains("pid="), "{busy}");
        let other = BoardLock::try_acquire_in(directory.path(), "30:ED:A0:F3:F6:D0").unwrap();
        drop(held);
        assert!(BoardLock::try_acquire_in(directory.path(), "38:44:BE:AA:25:64").is_ok());
        drop(other);
        assert!(BoardLock::try_acquire_in(directory.path(), "/dev/ttyACM0").is_err());
    }

    #[test]
    fn a_resource_is_held_by_one_owner_until_it_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let held = ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x").unwrap();
        assert!(
            ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x")
                .err()
                .unwrap()
                .is::<Busy>()
        );
        assert!(ResourceLock::try_acquire_in(directory.path(), "local-radio:/sys/y").is_ok());
        drop(held);
        assert!(ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x").is_ok());
    }

    #[test]
    fn a_board_lease_must_claim_its_board() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = crate::Arbiter::at(directory.path()).unwrap();
        let request = crate::Request {
            owner: "stand".into(),
            work: "flash".into(),
            scenarios: Vec::new(),
            claims: vec![crate::Claim::shared(crate::AIR)],
        };
        let error = arbiter
            .lease_board(&request, "38:44:BE:AA:25:64")
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("must claim it"), "{error}");
    }

    #[test]
    fn waiting_for_a_lock_ends_once_it_is_free_or_fails_otherwise() {
        let mut attempts = 0;
        let locked = wait_while_busy(|| {
            attempts += 1;
            if attempts == 1 {
                Err(Busy(String::from("held")).into())
            } else {
                Ok(attempts)
            }
        })
        .unwrap();
        assert_eq!(locked, 2);
        assert!(wait_while_busy(|| -> crate::Result<()> { Err("other".into()) }).is_err());
    }
}
