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
//! The kernel releases it when its broker exits, after admitted I/O closes.
//!
//! An independent lifetime broker owns the lock's file description. Owned
//! guards keep its admission channel alive; delegates carry an explicit
//! [`oer_process::Context`] capability for that board. [`DeviceAccess::operation`]
//! atomically admits I/O and retains exclusion until its returned guard drops.
//! After owner loss the broker rejects new operations and drains admitted ones.
//! Ports and their reader threads must close before their operation guard.
//!
//! Every acquisition in one process shares its owning guard through a registry.
//! Delegates never receive the flock descriptor and cannot unlock another I/O.
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
use oer_process::{
    Context,
    lock::{BrokerOperation, FileLock, LockBroker, Mode},
};
use serde::{Deserialize, Serialize};

/// Who holds a device lock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Holder {
    pub pid: u32,
    /// What the holder does with the board: its command line or lease.
    pub command: String,
    /// When it took the lock, in Unix milliseconds.
    pub started_unix_millis: u64,
    /// The capability explicitly attached to delegates of this board.
    pub token: String,
}

fn context_key(path: &Path) -> crate::Result<String> {
    Ok(format!(
        "device:{}",
        path.to_str().ok_or("device lock path is not UTF-8")?
    ))
}

fn inherited(path: &Path) -> crate::Result<Option<String>> {
    Ok(Context::current()?
        .get(&context_key(path)?)
        .map(str::to_owned))
}

fn token() -> String {
    oer_durable::sha256_bytes(
        format!(
            "{}:{}:{:?}",
            std::process::id(),
            oer_durable::unix_millis(),
            std::time::Instant::now()
        )
        .as_bytes(),
    )
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

/// The broker owner behind every clone of a [`DeviceGuard`].
#[derive(Debug)]
struct Owned {
    id: DeviceId,
    path: PathBuf,
    /// Taken in `drop` under the registry's mutex, so a concurrent
    /// acquisition in this process never sees the lock still held without
    /// its guard.
    broker: Option<LockBroker>,
    token: String,
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
        drop(self.broker.take());
    }
}

/// The owning guard of a board: the broker admits operations until the last
/// clone drops, then retains exclusion until admitted I/O ends. Every local
/// acquisition returns a clone of this same guard.
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

/// A board whose owner explicitly delegated a capability to this process.
/// It admits each operation through the lifetime broker, never by a racy
/// holder-record check. The operation survives owner loss; fresh work does not.
#[derive(Clone, Debug)]
pub struct DelegatedDevice {
    id: DeviceId,
    path: PathBuf,
    token: String,
}

impl DelegatedDevice {
    pub fn id(&self) -> &DeviceId {
        &self.id
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Exclusion for admitted I/O. Close all ports and child readers before this
/// guard drops. It keeps the broker's operation connection, and for local I/O
/// the owning guard too, without lending a flock descriptor to either caller.
#[derive(Debug)]
pub struct DeviceOperation {
    _broker: BrokerOperation,
    _owner: Option<DeviceGuard>,
}

impl DeviceOperation {
    /// Retain this admitted operation through an external hardware process.
    pub fn lifetime(&self) -> &oer_process::IoLifetime {
        self._broker.lifetime()
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
        if let Some(token) = inherited(&path)? {
            let delegate = DelegatedDevice {
                id: id.clone(),
                path: path.clone(),
                token,
            };
            let _operation =
                LockBroker::operation(&path.with_extension("broker"), &delegate.token)?;
            return Ok(Ok(Self::Delegated(delegate)));
        }
        match FileLock::try_acquire(&path, Mode::Exclusive)? {
            Some(lock) => {
                let token = token();
                record(&lock, command, &token)?;
                let broker = LockBroker::start(lock, &token)?;
                let owned = Arc::new(Owned {
                    id: id.clone(),
                    path: path.clone(),
                    broker: Some(broker),
                    token,
                });
                registry.insert(path, Arc::downgrade(&owned));
                Ok(Ok(Self::Owned(DeviceGuard { owned })))
            }
            None => Ok(Err(Busy::Held(holder_at(&path)?))),
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

    /// Admit I/O while its owner lives and retain exclusion through that I/O.
    /// The guard must outlive ports, resets, flashes and console readers.
    pub fn operation(&self) -> crate::Result<DeviceOperation> {
        let (token, owner) = match self {
            Self::Owned(guard) => (&guard.owned.token, Some(guard.clone())),
            Self::Delegated(delegate) => (&delegate.token, None),
        };
        let broker = LockBroker::operation(&self.path().with_extension("broker"), token)
            .map_err(|error| format!("board {}: {error}", self.id()))?;
        Ok(DeviceOperation {
            _broker: broker,
            _owner: owner,
        })
    }

    /// Add only this board's capability to the intended child's context.
    pub fn delegate(&self, context: &mut Context) -> crate::Result<()> {
        let _operation = self.operation()?;
        let token = match self {
            Self::Owned(guard) => &guard.owned.token,
            Self::Delegated(delegate) => &delegate.token,
        };
        context.set(context_key(self.path())?, token);
        Ok(())
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
fn record(lock: &FileLock, command: &str, token: &str) -> crate::Result<()> {
    let holder = Holder {
        pid: std::process::id(),
        command: command.to_owned(),
        started_unix_millis: oer_durable::unix_millis(),
        token: token.to_owned(),
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
        Some(holder)
            if inherited(&path)?.as_deref() == Some(holder.token.as_str())
                && LockBroker::operation(&path.with_extension("broker"), &holder.token).is_ok() =>
        {
            None
        }
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
        record(&foreign, "theirs", &token()).unwrap();
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
    fn owner_loss_drains_real_io_before_a_third_process_can_acquire() {
        use std::{
            process::{Command, Stdio},
            time::Instant,
        };
        const ROLE: &str = "OER_BROKER_TEST_ROLE";
        const DIRECTORY: &str = "OER_BROKER_TEST_DIRECTORY";
        const TEST: &str = "tests::owner_loss_drains_real_io_before_a_third_process_can_acquire";
        fn wait_for(path: &Path) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !path.exists() {
                assert!(
                    Instant::now() < deadline,
                    "fixture did not produce {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        fn child(role: &str, directory: &Path) -> Command {
            let mut command = oer_process::command(std::env::current_exe().unwrap());
            command
                .args(["--exact", TEST, "--test-threads=1"])
                .env(ROLE, role)
                .env(DIRECTORY, directory)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit());
            command
        }
        if let Ok(role) = std::env::var(ROLE) {
            let directory = PathBuf::from(std::env::var_os(DIRECTORY).unwrap());
            let board = id("00:11:22:33:44:66");
            match role.as_str() {
                "owner" => {
                    let access = DeviceAccess::try_acquire_in(&directory, &board, "owner")
                        .unwrap()
                        .unwrap();
                    let mut context = Context::default();
                    access.delegate(&mut context).unwrap();
                    let mut delegate = child("delegate", &directory);
                    if std::env::var_os("OER_BROKER_TEST_PIN_IO").is_some() {
                        delegate.env("OER_BROKER_TEST_PIN_IO", "1");
                    }
                    context.apply(&mut delegate).unwrap();
                    let mut delegate = delegate.spawn().unwrap();
                    std::fs::write(directory.join("delegate-pid"), delegate.id().to_string())
                        .unwrap();
                    // Reap while the owner lives, without joining: the fixture
                    // deliberately exits before its admitted delegate finishes.
                    std::thread::spawn(move || {
                        let _ = delegate.wait();
                    });
                    wait_for(&directory.join("drop-owner"));
                    drop(access);
                }
                "delegate" => {
                    let access = DeviceAccess::try_acquire_in(&directory, &board, "delegate")
                        .unwrap()
                        .unwrap();
                    assert!(access.is_delegated());
                    let operation = access.operation().unwrap();
                    if std::env::var_os("OER_BROKER_TEST_PIN_IO").is_some() {
                        let mut writer = child("writer", &directory);
                        operation.lifetime().pin(&mut writer).unwrap();
                        let mut writer = writer.spawn().unwrap();
                        assert!(writer.wait().unwrap().success());
                        return;
                    }
                    // A real writer stays open across owner death, rather than
                    // a test of the source spelling or of an instantaneous check.
                    let mut io = std::fs::File::create(directory.join("io")).unwrap();
                    std::fs::write(directory.join("admitted"), "ready").unwrap();
                    let deadline = Instant::now() + Duration::from_secs(15);
                    while !directory.join("finish-io").exists() {
                        assert!(Instant::now() < deadline, "I/O fixture was not released");
                        io.write_all(b"still writing\n").unwrap();
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    assert!(access.operation().is_err(), "owner loss admitted fresh I/O");
                    assert!(
                        DeviceAccess::try_acquire_in(&directory, &board, "stale delegate").is_err()
                    );
                    drop(io);
                    drop(operation);
                    std::fs::write(directory.join("finished"), "closed").unwrap();
                }
                "writer" => {
                    // This exec receives only a lifetime descriptor, never a
                    // capability to start new work. It outlives both callers.
                    assert_eq!(Context::current().unwrap(), &Context::default());
                    let mut io = std::fs::File::create(directory.join("io")).unwrap();
                    std::fs::write(directory.join("admitted"), "ready").unwrap();
                    let deadline = Instant::now() + Duration::from_secs(15);
                    while !directory.join("finish-io").exists() {
                        assert!(Instant::now() < deadline, "external I/O was not released");
                        io.write_all(b"external I/O still running\n").unwrap();
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    drop(io);
                    std::fs::write(directory.join("finished"), "closed").unwrap();
                }
                "contender-busy" => {
                    assert!(matches!(
                        DeviceAccess::try_acquire_in(&directory, &board, "third").unwrap(),
                        Err(Busy::Held(_))
                    ));
                    assert_eq!(Context::current().unwrap(), &Context::default());
                }
                "contender-free" => {
                    let access = DeviceAccess::try_acquire_in(&directory, &board, "third")
                        .unwrap()
                        .unwrap();
                    assert!(!access.is_delegated());
                    let _operation = access.operation().unwrap();
                }
                _ => panic!("unknown broker fixture role"),
            }
            return;
        }
        for (kill_owner, kill_delegate) in [(false, false), (true, false), (true, true)] {
            let directory = tempfile::tempdir().unwrap();
            let mut owner = child("owner", directory.path());
            if kill_delegate {
                owner.env("OER_BROKER_TEST_PIN_IO", "1");
            }
            let mut owner = owner.spawn().unwrap();
            wait_for(&directory.path().join("admitted"));
            if kill_owner {
                owner.kill().unwrap();
            } else {
                std::fs::write(directory.path().join("drop-owner"), "drop").unwrap();
            }
            let status = owner.wait().unwrap();
            assert_eq!(status.success(), !kill_owner);
            if kill_delegate {
                let pid = std::fs::read_to_string(directory.path().join("delegate-pid")).unwrap();
                oer_process::capture(oer_process::command("kill").args(["-KILL", pid.trim()]))
                    .unwrap();
            }
            let endpoint = directory.path().join("001122334466.broker");
            let deadline = Instant::now() + Duration::from_secs(5);
            while endpoint.exists() {
                assert!(Instant::now() < deadline, "broker did not close admission");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                child("contender-busy", directory.path())
                    .status()
                    .unwrap()
                    .success()
            );
            let size = std::fs::metadata(directory.path().join("io"))
                .unwrap()
                .len();
            std::thread::sleep(Duration::from_millis(30));
            assert!(
                std::fs::metadata(directory.path().join("io"))
                    .unwrap()
                    .len()
                    > size
            );
            std::fs::write(directory.path().join("finish-io"), "finish").unwrap();
            wait_for(&directory.path().join("finished"));
            let lock = directory.path().join("001122334466.lock");
            let deadline = Instant::now() + Duration::from_secs(5);
            while FileLock::try_acquire(&lock, Mode::Exclusive)
                .unwrap()
                .is_none()
            {
                assert!(
                    Instant::now() < deadline,
                    "broker did not release finished I/O"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                child("contender-free", directory.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
}
