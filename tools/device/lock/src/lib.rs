//! The device lock: one process owns a board at a time.
//!
//! The key is the board's [`DeviceId`] (its MAC, an Espressif USB
//! Serial/JTAG port's serial number), the file
//! `$XDG_RUNTIME_DIR/open-esp-radio/devices/<MAC>.lock` (the XDG cache
//! directory when `XDG_RUNTIME_DIR` is unset), locked with the foundation's
//! `flock` ([`oer_process::lock::FileLock`]). The holder writes its
//! [`Holder`] record into the file, so a refused process names who holds
//! the board. A lock is held for a whole operation: a flash with its reopen
//! and start, a reset, a whole monitor, or a HIL lease with its power cycles.
//! The kernel releases it when its holder exits, however it exits.
//!
//! Two kinds of access, with distinct promises:
//!
//! - [`DeviceGuard`], the owning guard: this process holds the `flock`, and
//!   holds it until the last clone of the guard drops. Within one process
//!   there is exactly one owning guard per board: a registry keyed by the
//!   lock file hands a second acquisition a clone of the live guard (an
//!   `Arc`), never a handle without the lock, so no holder can outlive the
//!   exclusion it relies on.
//! - [`DelegatedDevice`], a delegate: an ancestor process holds the `flock`
//!   and handed the board to this process. Across processes the contract is
//!   **the parent keeps the lease alive until its children exit**: the
//!   holder records its delegation token, it exports the token in
//!   [`DELEGATION_ENV`] ([`delegation`]) only to the commands it starts and
//!   waits for (a stand lease's command, a HIL run's `--then`), and releases
//!   its guard only after they ended. No descriptor is handed over, since a
//!   duplicated `flock` descriptor would be unlocked by whichever copy drops
//!   first. Because an ancestor can still die abnormally, a delegate checks
//!   [`DelegatedDevice::ensure_held`] before each operation on the board: the
//!   lock must still be held under its token. This check is not atomic with
//!   the following I/O: parent death after the check can release exclusion
//!   during that operation. Delegated access therefore requires the parent
//!   lifetime; a broker retaining ownership through admitted I/O is needed
//!   for exclusion independent of it.
//!
//! [`DeviceAccess`] is either; board I/O takes it.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use std::{
    collections::HashMap,
    io::{Read as _, Seek as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Duration,
};

pub use oer_device_mac::DeviceId;
use oer_process::lock::{FileLock, Mode};
use serde::{Deserialize, Serialize};

/// Who holds a device lock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Holder {
    pub pid: u32,
    /// What the holder does with the board: its command line or lease.
    pub command: String,
    /// When it took the lock, in Unix milliseconds.
    pub started_unix_millis: u64,
    /// The token its delegates carry in [`DELEGATION_ENV`].
    pub token: String,
}

/// Carries the delegation token of the device locks a process's ancestor
/// holds: such a process uses those boards as [`DelegatedDevice`]s.
pub const DELEGATION_ENV: &str = "OER_DEVICE_DELEGATION";

/// The token this process inherited in [`DELEGATION_ENV`], if any.
fn inherited() -> Option<&'static str> {
    static INHERITED: OnceLock<Option<String>> = OnceLock::new();
    INHERITED
        .get_or_init(|| {
            std::env::var(DELEGATION_ENV)
                .ok()
                .filter(|token| !token.is_empty())
        })
        .as_deref()
}

/// This process's delegation token: the one it inherited in
/// [`DELEGATION_ENV`], else its own.
fn token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        inherited().map_or_else(
            || {
                oer_durable::sha256_bytes(
                    format!(
                        "{}:{}:{:?}",
                        std::process::id(),
                        oer_durable::unix_millis(),
                        std::time::Instant::now()
                    )
                    .as_bytes(),
                )
            },
            str::to_owned,
        )
    })
}

/// The environment that hands every device lock this process holds to a
/// command it starts. The caller keeps its guards until that command exits.
pub fn delegation() -> (&'static str, String) {
    (DELEGATION_ENV, token().to_owned())
}

impl std::fmt::Display for Holder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let age = oer_durable::unix_millis().saturating_sub(self.started_unix_millis) / 1000;
        write!(formatter, "pid {} `{}` for {age} s", self.pid, self.command)
    }
}

/// The directory of the device locks.
pub fn directory() -> crate::Result<PathBuf> {
    oer_durable::xdg::path(oer_durable::xdg::Base::Runtime, "devices")
}

/// The lock file of the board `id`.
pub fn path(id: &DeviceId) -> crate::Result<PathBuf> {
    Ok(path_in(&directory()?, id))
}

fn path_in(directory: &Path, id: &DeviceId) -> PathBuf {
    directory.join(format!("{}.lock", id.compact()))
}

/// The owning guards of this process, by lock file: one per board.
fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Owned>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<Owned>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// The held `flock` behind every clone of a [`DeviceGuard`].
#[derive(Debug)]
struct Owned {
    id: DeviceId,
    path: PathBuf,
    /// Taken in `drop` under the registry's mutex, so a concurrent
    /// acquisition in this process never sees the lock still held without
    /// its guard.
    lock: Option<FileLock>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        let mut registry = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry
            .get(&self.path)
            .is_some_and(|weak| weak.strong_count() == 0)
        {
            registry.remove(&self.path);
        }
        drop(self.lock.take());
    }
}

/// The owning guard of a board: this process holds its `flock` until the
/// last clone drops. Every acquisition of the board in this process returns
/// a clone of the same guard.
#[derive(Clone, Debug)]
pub struct DeviceGuard {
    owned: Arc<Owned>,
}

impl DeviceGuard {
    pub fn id(&self) -> &DeviceId {
        &self.owned.id
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.owned.path
    }
}

/// A board an ancestor process holds and handed to this one through
/// [`DELEGATION_ENV`]. It holds no lock itself: the exclusion lasts while
/// the ancestor's guard does, which the ancestor keeps until this process
/// exits; [`Self::ensure_held`] checks it before an operation.
#[derive(Clone, Debug)]
pub struct DelegatedDevice {
    id: DeviceId,
    path: PathBuf,
}

impl DelegatedDevice {
    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fail unless the board's lock is still held under this process's
    /// inherited token: an ancestor that ended released the board, and a
    /// delegate must not touch it then.
    pub fn ensure_held(&self) -> crate::Result<()> {
        match holder_at(&self.path)? {
            Some(holder) if inherited() == Some(holder.token.as_str()) => Ok(()),
            Some(holder) => Err(format!(
                "board {} was delegated to this process, but {holder} holds it now",
                self.id
            )
            .into()),
            None => Err(format!(
                "board {} was delegated to this process, but its delegating lease ended",
                self.id
            )
            .into()),
        }
    }
}

/// Access to a board: owned by this process or delegated by an ancestor.
#[derive(Clone, Debug)]
pub enum DeviceAccess {
    Owned(DeviceGuard),
    Delegated(DelegatedDevice),
}

/// Why a lock was not taken.
#[derive(Debug)]
pub enum Busy {
    /// Another process holds it; its record, when it wrote one.
    Held(Option<Holder>),
}

impl std::fmt::Display for Busy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held(Some(holder)) => write!(formatter, "held by {holder}"),
            Self::Held(None) => write!(formatter, "held by another process"),
        }
    }
}

impl DeviceAccess {
    /// The board `id` for `command`: this process's guard of it, a new
    /// guard, or the delegation of an ancestor; else who holds it.
    pub fn try_acquire(
        id: &DeviceId,
        command: &str,
    ) -> crate::Result<std::result::Result<Self, Busy>> {
        Self::try_acquire_in(&directory()?, id, command)
    }

    /// [`Self::try_acquire`] with the lock files in `directory`: a test's
    /// own.
    pub fn try_acquire_in(
        directory: &Path,
        id: &DeviceId,
        command: &str,
    ) -> crate::Result<std::result::Result<Self, Busy>> {
        let path = path_in(directory, id);
        // Held for the whole decision, so this process never takes a second
        // lock of a board it holds, nor mistakes its own lock for an
        // ancestor's.
        let mut registry = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owned) = registry.get(&path).and_then(Weak::upgrade) {
            return Ok(Ok(Self::Owned(DeviceGuard { owned })));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match FileLock::try_acquire(&path, Mode::Exclusive)? {
            Some(lock) => {
                record(&lock, command)?;
                let owned = Arc::new(Owned {
                    id: id.clone(),
                    path: path.clone(),
                    lock: Some(lock),
                });
                registry.insert(path, Arc::downgrade(&owned));
                Ok(Ok(Self::Owned(DeviceGuard { owned })))
            }
            None => match holder_at(&path)? {
                // Only an inherited token delegates: a lock under this
                // process's own token without a live guard is not ours.
                Some(holder) if inherited() == Some(holder.token.as_str()) => {
                    Ok(Ok(Self::Delegated(DelegatedDevice {
                        id: id.clone(),
                        path,
                    })))
                }
                holder => Ok(Err(Busy::Held(holder))),
            },
        }
    }

    /// The board `id` for `command`; a busy board is an error naming its
    /// holder.
    pub fn acquire(id: &DeviceId, command: &str) -> crate::Result<Self> {
        Self::try_acquire(id, command)?
            .map_err(|busy| format!("device {id} is busy: {busy}").into())
    }

    /// The board `id` for `command`, waiting while another process holds
    /// it; the holder is printed once when the wait starts.
    pub fn wait(id: &DeviceId, command: &str) -> crate::Result<Self> {
        let mut announced = false;
        loop {
            match Self::try_acquire(id, command)? {
                Ok(access) => return Ok(access),
                Err(busy) if !announced => {
                    eprintln!("device {id} is busy ({busy}); waiting");
                    announced = true;
                }
                Err(_) => {}
            }
            oer_process::sleep(Duration::from_millis(250))?;
        }
    }

    /// The board.
    pub fn id(&self) -> &DeviceId {
        match self {
            Self::Owned(guard) => guard.id(),
            Self::Delegated(delegate) => delegate.id(),
        }
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        match self {
            Self::Owned(guard) => guard.path(),
            Self::Delegated(delegate) => delegate.path(),
        }
    }

    /// Whether an ancestor holds the lock and this process uses it as its
    /// delegate.
    pub fn is_delegated(&self) -> bool {
        matches!(self, Self::Delegated(_))
    }

    /// Fail unless this access still excludes every other process: always
    /// for an owning guard, while the delegating lease lives for a
    /// delegate. Every board operation calls it first.
    pub fn ensure_held(&self) -> crate::Result<()> {
        match self {
            Self::Owned(_) => Ok(()),
            Self::Delegated(delegate) => delegate.ensure_held(),
        }
    }

    /// Fail unless this access is the board `id`'s.
    pub fn ensure_covers(&self, id: &DeviceId) -> crate::Result<()> {
        if self.id() == id {
            Ok(())
        } else {
            Err(format!("the lock of board {} does not cover board {id}", self.id()).into())
        }
    }
}

/// Write this process's [`Holder`] record for `command` into `lock`'s file.
fn record(lock: &FileLock, command: &str) -> crate::Result<()> {
    let holder = Holder {
        pid: std::process::id(),
        command: command.to_owned(),
        started_unix_millis: oer_durable::unix_millis(),
        token: token().to_owned(),
    };
    let mut file = lock.file();
    file.set_len(0)?;
    file.rewind()?;
    file.write_all(&serde_json::to_vec(&holder)?)?;
    file.flush()?;
    Ok(())
}

/// Who holds the lock of board `id`, or `None` when it is free.
pub fn holder(id: &DeviceId) -> crate::Result<Option<Holder>> {
    holder_at(&path(id)?)
}

/// The holder record of a held lock at `path`, read strictly: a holder that
/// has not written its record yet is `None`, a record that does not parse
/// is an error (it decides delegation).
fn holder_at(path: &Path) -> crate::Result<Option<Holder>> {
    if !path.exists() {
        return Ok(None);
    }
    // A free lock has no holder, whatever an earlier holder left in it.
    if FileLock::try_acquire(path, Mode::Shared)?.is_some() {
        return Ok(None);
    }
    let mut text = String::new();
    std::fs::File::open(path)?.read_to_string(&mut text)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&text).map_err(|error| {
        format!("{}: unreadable holder record: {error}", path.display())
    })?))
}

/// Who holds the lock of board `id` when this process may not use it: held,
/// neither by this process nor by the ancestor that delegated to it.
/// `Some(None)` is a holder that wrote no record yet.
pub fn foreign_holder(id: &DeviceId) -> crate::Result<Option<Option<Holder>>> {
    let path = path(id)?;
    if registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&path)
        .is_some_and(|weak| weak.strong_count() > 0)
    {
        return Ok(None);
    }
    if !is_held(id)? {
        return Ok(None);
    }
    Ok(match holder(id)? {
        Some(holder) if inherited() == Some(holder.token.as_str()) => None,
        holder => Some(holder),
    })
}

/// Whether the board `id` is locked by any process.
pub fn is_held(id: &DeviceId) -> crate::Result<bool> {
    let path = path(id)?;
    Ok(path.exists() && FileLock::try_acquire(&path, Mode::Shared)?.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in the child process of [`a_delegate_works_while_its_lease_lives`]:
    /// the lock directory it uses.
    const CHILD_DIRECTORY_ENV: &str = "OER_DEVICE_LOCK_TEST_DIRECTORY";

    fn id(text: &str) -> DeviceId {
        DeviceId::parse(text).unwrap()
    }

    fn owned(access: DeviceAccess) -> DeviceGuard {
        match access {
            DeviceAccess::Owned(guard) => guard,
            DeviceAccess::Delegated(_) => panic!("expected an owning guard"),
        }
    }

    #[test]
    fn every_spelling_of_a_mac_is_one_lock_and_one_owning_guard() {
        let directory = tempfile::tempdir().unwrap();
        let first = owned(
            DeviceAccess::try_acquire_in(directory.path(), &id("38:44:be:aa:25:64"), "first")
                .unwrap()
                .unwrap(),
        );
        let second = owned(
            DeviceAccess::try_acquire_in(directory.path(), &id("3844BEAA2564"), "second")
                .unwrap()
                .unwrap(),
        );
        assert_eq!(first.path(), second.path());
        assert_eq!(first.path(), directory.path().join("3844BEAA2564.lock"));
        // The second acquisition shares the live guard instead of holding
        // nothing.
        assert!(Arc::ptr_eq(&first.owned, &second.owned));
        // Another open file description (another process) is refused while
        // either clone lives, and the holder is the first acquisition's.
        drop(first);
        assert!(
            FileLock::try_acquire(second.path(), Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        let holder = holder_at(second.path()).unwrap().unwrap();
        assert_eq!(
            (holder.pid, holder.command.as_str()),
            (std::process::id(), "first")
        );
        let path = second.path().to_owned();
        drop(second);
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_some()
        );
        assert_eq!(holder_at(&path).unwrap(), None);
    }

    #[test]
    fn a_board_another_holder_locked_is_busy_with_that_holder() {
        let directory = tempfile::tempdir().unwrap();
        let board = id("00:11:22:33:44:55");
        let path = path_in(directory.path(), &board);
        let foreign = FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .unwrap();
        let busy = DeviceAccess::try_acquire_in(directory.path(), &board, "mine").unwrap();
        assert!(matches!(busy, Err(Busy::Held(None))));
        record(&foreign, "theirs").unwrap();
        // The holder's token is this process's own, not an inherited one:
        // never a delegation.
        match DeviceAccess::try_acquire_in(directory.path(), &board, "mine").unwrap() {
            Err(Busy::Held(Some(holder))) => assert_eq!(holder.command, "theirs"),
            other => panic!("{other:?}"),
        }
        // An unparsable record is an error, not a free board.
        foreign.file().set_len(0).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(DeviceAccess::try_acquire_in(directory.path(), &board, "mine").is_err());
        drop(foreign);
        assert!(
            DeviceAccess::try_acquire_in(directory.path(), &board, "mine")
                .unwrap()
                .is_ok()
        );
    }

    #[test]
    fn a_delegate_works_while_its_lease_lives() {
        let Ok(directory) = std::env::var(CHILD_DIRECTORY_ENV) else {
            // The parent: run this test again in a child that inherited a
            // delegation token.
            let directory = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::a_delegate_works_while_its_lease_lives",
                    "--test-threads=1",
                ])
                .env(DELEGATION_ENV, "lease-token")
                .env(CHILD_DIRECTORY_ENV, directory.path())
                .status()
                .unwrap();
            assert!(status.success());
            return;
        };
        let directory = Path::new(&directory);
        assert_eq!(inherited(), Some("lease-token"));
        let board = id("00:11:22:33:44:66");
        // The ancestor's lock, recorded under the token it handed down.
        let ancestor = FileLock::try_acquire(&path_in(directory, &board), Mode::Exclusive)
            .unwrap()
            .unwrap();
        record(&ancestor, "ancestor lease").unwrap();
        let access = DeviceAccess::try_acquire_in(directory, &board, "child")
            .unwrap()
            .unwrap();
        assert!(access.is_delegated());
        access.ensure_held().unwrap();
        access.ensure_covers(&board).unwrap();
        assert!(access.ensure_covers(&id("00:11:22:33:44:77")).is_err());
        // The lease ended: the delegate must not touch the board.
        drop(ancestor);
        let error = access.ensure_held().unwrap_err().to_string();
        assert!(error.contains("lease ended"), "{error}");
        // Another holder took it.
        let other = FileLock::try_acquire(&path_in(directory, &board), Mode::Exclusive)
            .unwrap()
            .unwrap();
        other.file().set_len(0).unwrap();
        let mut file = other.file();
        file.write_all(
            &serde_json::to_vec(&Holder {
                pid: 1,
                command: "someone else".into(),
                started_unix_millis: 0,
                token: "another-token".into(),
            })
            .unwrap(),
        )
        .unwrap();
        let error = access.ensure_held().unwrap_err().to_string();
        assert!(error.contains("someone else"), "{error}");
    }
}
