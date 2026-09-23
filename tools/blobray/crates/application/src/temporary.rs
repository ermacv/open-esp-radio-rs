//! Application-owned temporary capacity and identifiable read-operation workspaces.
use crate::*;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct TemporaryStoragePolicy {
    pub root: Option<PathBuf>,
    pub operation_bytes: u64,
    pub total_bytes: u64,
}
impl Default for TemporaryStoragePolicy {
    fn default() -> Self {
        Self {
            root: None,
            operation_bytes: 8 * 1024 * 1024 * 1024,
            total_bytes: 32 * 1024 * 1024 * 1024,
        }
    }
}
struct Pool {
    used: u64,
    pending: Vec<(PathBuf, u64)>,
    diagnostics: Vec<Error>,
    diagnostics_truncated: bool,
}
pub(crate) struct TemporaryRuntime {
    pub policy: TemporaryStoragePolicy,
    pool: Arc<Mutex<Pool>>,
    max_pending: usize,
}
impl TemporaryRuntime {
    pub fn new(policy: TemporaryStoragePolicy, max_pending: usize) -> Result<Self> {
        if policy.operation_bytes < TEMPORARY_CONTROL_BYTES
            || policy.total_bytes < policy.operation_bytes
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "temporary limits require operation >= 1 MiB and total >= operation",
            ));
        }
        Ok(Self {
            policy,
            max_pending,
            pool: Arc::new(Mutex::new(Pool {
                used: 0,
                pending: Vec::new(),
                diagnostics: Vec::new(),
                diagnostics_truncated: false,
            })),
        })
    }
    pub fn reserve(&self) -> Result<TemporaryReservation> {
        let mut pool = self.pool.lock().unwrap();
        let mut i = 0;
        while i < pool.pending.len() {
            if !pool.pending[i].0.try_exists().map_err(storage_io)? {
                let (_, bytes) = pool.pending.swap_remove(i);
                pool.used -= bytes;
            } else {
                i += 1;
            }
        }
        if pool.pending.len() >= self.max_pending {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "temporary residue capacity reached; reclaim reported paths before admitting more operations",
            ));
        }
        let available = self.policy.total_bytes - pool.used;
        if self.policy.operation_bytes > available {
            let mut error = Error::new(
                ErrorCode::ResourceLimited,
                "application temporary storage capacity exhausted",
            );
            error.storage = Some(Box::new(StorageFailure {
                requested_bytes: self.policy.operation_bytes,
                available_bytes: available,
                limit_bytes: self.policy.total_bytes,
                owner: None,
                position: RunPosition::default(),
            }));
            return Err(error);
        }
        pool.used += self.policy.operation_bytes;
        Ok(TemporaryReservation {
            pool: self.pool.clone(),
            bytes: self.policy.operation_bytes,
            path: None,
        })
    }
    pub fn status(&self) -> TemporaryStorageStatus {
        let pool = self.pool.lock().unwrap();
        TemporaryStorageStatus {
            reserved_bytes: pool.used,
            limit_bytes: self.policy.total_bytes,
            residue: pool.pending.iter().map(|(p, _)| p.clone()).collect(),
            diagnostics: pool.diagnostics.clone(),
            diagnostics_truncated: pool.diagnostics_truncated,
        }
    }
    pub fn root(&self, host: &dyn OperationHost) -> Result<PathBuf> {
        let root = host.temporary_root(self.policy.root.as_deref())?;
        let _lock = root_lock(&root)?;
        for (count, entry) in fs::read_dir(&root).map_err(storage_io)?.enumerate() {
            if count >= 4096 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "runtime root exceeds 4096 entries; inspect and reclaim reported residue",
                ));
            }
            let path = entry.map_err(storage_io)?.path();
            if path.file_name() == Some(std::ffi::OsStr::new("runtime.lock")) {
                continue;
            }
            match reclaim(&path, host) {
                Ok(()) => (),
                Err(e) if e.code == ErrorCode::Busy => (),
                Err(error) => self.pool.lock().unwrap().diagnostic(&path, error),
            }
        }
        Ok(root)
    }
    pub fn workspace(
        &self,
        host: Arc<dyn OperationHost>,
        root: &Path,
        mut reservation: TemporaryReservation,
    ) -> Result<Workspace> {
        let _root_lock = root_lock(root)?;
        let temporary = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(root)
            .map_err(storage_io)?;
        let run: RunId = ArtifactId::of_bytes(temporary.path().as_os_str().as_encoded_bytes())
            .as_str()
            .parse()?;
        let path = temporary.keep();
        reservation.attach(&path);
        let mut workspace = Workspace {
            stage: path.join(run.as_str()),
            path,
            run,
            _lease: None,
            reservation,
            host,
        };
        fs::create_dir(&workspace.stage).map_err(storage_io)?;
        let lease = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(workspace.stage.join("lease.lock"))
            .map_err(storage_io)?;
        lease.lock_shared().map_err(storage_io)?;
        workspace._lease = Some(lease);
        let record = RuntimeRecord {
            schema: 1,
            owner: workspace.host.owner()?,
            run: workspace.run.clone(),
        };
        crate::protocol::write_request(
            File::create(workspace.path.join("owner.json")).map_err(storage_io)?,
            &record,
        )?;
        Ok(workspace)
    }
}
#[derive(Clone, Debug)]
pub struct TemporaryStorageStatus {
    pub reserved_bytes: u64,
    pub limit_bytes: u64,
    pub residue: Vec<PathBuf>,
    pub diagnostics: Vec<Error>,
    pub diagnostics_truncated: bool,
}
pub(crate) struct TemporaryReservation {
    pool: Arc<Mutex<Pool>>,
    bytes: u64,
    path: Option<PathBuf>,
}
impl TemporaryReservation {
    pub fn capacity(&self) -> u64 {
        self.bytes
    }
    pub fn attach(&mut self, path: &Path) {
        self.path = Some(path.into());
    }
    pub fn shrink(&mut self, bytes: u64) -> Result<()> {
        if bytes > self.bytes {
            return Err(Error::new(
                ErrorCode::Integrity,
                "temporary result exceeds admitted reservation",
            ));
        }
        self.pool.lock().unwrap().used -= self.bytes - bytes;
        self.bytes = bytes;
        Ok(())
    }
    pub fn released(&mut self) {
        self.path = None;
    }
}
impl Drop for TemporaryReservation {
    fn drop(&mut self) {
        let mut pool = self.pool.lock().unwrap();
        if let Some(path) = &self.path
            && path.try_exists().unwrap_or(true)
        {
            pool.diagnostic(
                path,
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "temporary files remain charged until confirmed deletion",
                ),
            );
            pool.pending.push((path.clone(), self.bytes));
        } else {
            pool.used -= self.bytes;
        }
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeRecord {
    schema: u32,
    owner: OwnerIdentity,
    run: RunId,
}
pub(crate) struct Workspace {
    path: PathBuf,
    pub stage: PathBuf,
    pub run: RunId,
    _lease: Option<File>,
    reservation: TemporaryReservation,
    host: Arc<dyn OperationHost>,
}
impl Workspace {
    pub fn retain(&mut self) -> Result<()> {
        let bytes = tree_bytes(&self.path)?;
        // Control files use the fixed reserve, not additional data capacity.
        self.reservation.shrink(bytes)
    }
    pub fn cleanup(&mut self) -> Result<()> {
        self.host.reclaim(&self.stage)?;
        fs::remove_dir_all(&self.path).map_err(storage_io)?;
        self.reservation.released();
        Ok(())
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if self.path.try_exists().unwrap_or(true)
            && let Err(error) = self.cleanup()
        {
            self.reservation
                .pool
                .lock()
                .unwrap()
                .diagnostic(&self.path, error);
        }
    }
}
impl Pool {
    fn diagnostic(&mut self, path: &Path, mut error: Error) {
        error.message = format!("temporary residue {}: {}", path.display(), error.message);
        truncate_message(&mut error.message);
        if self.diagnostics.iter().any(|e| e == &error) {
            return;
        }
        if self.diagnostics.len() < 4 {
            self.diagnostics.push(error);
        } else {
            self.diagnostics_truncated = true;
        }
    }
}
fn regular(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path).map_err(storage_io)?.is_file() {
        return Err(Error::new(
            ErrorCode::Integrity,
            format!("not a regular runtime file: {}", path.display()),
        ));
    }
    Ok(())
}
fn root_lock(root: &Path) -> Result<File> {
    let path = root.join("runtime.lock");
    if path.symlink_metadata().is_ok() {
        regular(&path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(storage_io)?;
    file.lock().map_err(storage_io)?;
    Ok(file)
}
fn reclaim(path: &Path, host: &dyn OperationHost) -> Result<()> {
    if !path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("run-"))
        || !fs::symlink_metadata(path).map_err(storage_io)?.is_dir()
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            format!("unrecognized runtime entry: {}", path.display()),
        ));
    }
    regular(&path.join("owner.json"))?;
    let mut bytes = Vec::new();
    File::open(path.join("owner.json"))
        .map_err(storage_io)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(storage_io)?;
    if bytes.len() > 65536 {
        return Err(Error::new(
            ErrorCode::Integrity,
            "runtime owner record exceeds limit",
        ));
    }
    let owner: RuntimeRecord = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new(ErrorCode::RecoveryRequired, e.to_string()))?;
    if owner.schema != 1 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported runtime owner version",
        ));
    }
    if host.alive(&owner.owner)? {
        return Err(Error::new(ErrorCode::Busy, "runtime owner is alive"));
    }
    let stage = path.join(owner.run.as_str());
    if !fs::symlink_metadata(&stage).map_err(storage_io)?.is_dir() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "runtime stage is not a directory",
        ));
    }
    regular(&stage.join("lease.lock"))?;
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("lease.lock"))
        .map_err(storage_io)?;
    lease.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => Error::new(ErrorCode::Busy, "runtime lease is active"),
        std::fs::TryLockError::Error(e) => storage_io(e),
    })?;
    tree_bytes(path)?; // Reject any link/special file before interpreting guard metadata.
    host.reclaim(&stage)?;
    fs::remove_dir_all(path).map_err(storage_io)
}
fn tree_bytes(root: &Path) -> Result<u64> {
    // One directory iterator per level: a wide tree cannot allocate a worklist
    // proportional to its number of sibling directories in the coordinator.
    let mut stack = vec![fs::read_dir(root).map_err(storage_io)?];
    let mut bytes = 0u64;
    let mut entries = 0u64;
    while let Some(directory) = stack.last_mut() {
        let Some(entry) = directory.next() else {
            stack.pop();
            continue;
        };
        entries += 1;
        if entries > 1_000_000 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "runtime workspace exceeds 1000000 entries",
            ));
        }
        let path = entry.map_err(storage_io)?.path();
        let metadata = fs::symlink_metadata(&path).map_err(storage_io)?;
        if metadata.is_dir() {
            if stack.len() >= 7 {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "unexpected runtime nesting",
                ));
            }
            stack.push(fs::read_dir(&path).map_err(storage_io)?);
        } else if metadata.is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "temporary size overflow"))?;
        } else {
            return Err(Error::new(
                ErrorCode::Integrity,
                format!(
                    "runtime contains a link or special file: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(bytes)
}
pub(crate) fn configure(stage: &Path, run: &RunId, limit_bytes: u64) -> Result<()> {
    crate::protocol::write_request(
        File::create(stage.join("temporary.json")).map_err(storage_io)?,
        &blobray_store::TemporaryConfig {
            schema: 1,
            owner: run.clone(),
            limit_bytes,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Host {
        root: PathBuf,
        alive: AtomicBool,
        fail: AtomicBool,
        bad_owner: AtomicBool,
        reclaims: AtomicUsize,
    }
    impl OperationHost for Host {
        fn temporary_root(&self, _: Option<&Path>) -> Result<PathBuf> {
            Ok(self.root.clone())
        }
        fn now_ms(&self) -> u64 {
            0
        }
        fn save_progress(&self, _: &Path, _: &ProgressRecord) -> Result<()> {
            Ok(())
        }
        fn owner(&self) -> Result<OwnerIdentity> {
            if self.bad_owner.load(Ordering::SeqCst) {
                return Err(Error::new(
                    ErrorCode::Unavailable,
                    "owner identity unavailable",
                ));
            }
            Ok(OwnerIdentity {
                pid: 1,
                start_ticks: 2,
                boot_id: "fixture".into(),
            })
        }
        fn alive(&self, _: &OwnerIdentity) -> Result<bool> {
            Ok(self.alive.load(Ordering::SeqCst))
        }
        fn reclaim(&self, _: &Path) -> Result<()> {
            self.reclaims.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err(Error::new(ErrorCode::Io, "injected cleanup failure"))
            } else {
                Ok(())
            }
        }
        fn launch(&self, _: &Path, _: &ResourceBudget, _: u64) -> Result<Box<dyn OperationWorker>> {
            unreachable!()
        }
    }
    fn setup() -> (tempfile::TempDir, TemporaryRuntime, Arc<Host>) {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(Host {
            root: dir.path().into(),
            alive: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            bad_owner: AtomicBool::new(false),
            reclaims: AtomicUsize::new(0),
        });
        let runtime = TemporaryRuntime::new(
            TemporaryStoragePolicy {
                root: None,
                operation_bytes: TEMPORARY_CONTROL_BYTES,
                total_bytes: 2 * TEMPORARY_CONTROL_BYTES,
            },
            2,
        )
        .unwrap();
        (dir, runtime, host)
    }
    fn abandoned(root: &Path, host: &Host, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        let run: RunId = ArtifactId::of_bytes(name.as_bytes())
            .as_str()
            .parse()
            .unwrap();
        let stage = path.join(run.as_str());
        fs::create_dir(&stage).unwrap();
        File::create(stage.join("lease.lock")).unwrap();
        crate::write_control_message(
            File::create(path.join("owner.json")).unwrap(),
            &RuntimeRecord {
                schema: 1,
                owner: host.owner().unwrap(),
                run,
            },
        )
        .unwrap();
        path
    }
    #[test]
    fn failed_workspace_initialization_keeps_residue_and_original_error() {
        let (dir, runtime, host) = setup();
        host.bad_owner.store(true, Ordering::SeqCst);
        host.fail.store(true, Ordering::SeqCst);
        let error = runtime
            .workspace(host.clone(), dir.path(), runtime.reserve().unwrap())
            .err()
            .unwrap();
        assert_eq!(error.code, ErrorCode::Unavailable);
        let status = runtime.status();
        assert_eq!(status.reserved_bytes, TEMPORARY_CONTROL_BYTES);
        assert_eq!(status.residue.len(), 1);
        assert!(!status.diagnostics.is_empty());
        runtime.root(&*host).unwrap();
        assert!(status.residue[0].exists());
        fs::remove_dir_all(&status.residue[0]).unwrap();
        drop(runtime.reserve().unwrap());
        assert_eq!(runtime.status().reserved_bytes, 0);
    }
    #[test]
    fn orphan_reclaim_requires_dead_owner_and_inactive_lease() {
        let (dir, runtime, host) = setup();
        let active = runtime
            .workspace(host.clone(), dir.path(), runtime.reserve().unwrap())
            .unwrap();
        runtime.root(&*host).unwrap();
        assert!(active.path.exists());
        assert_eq!(host.reclaims.load(Ordering::SeqCst), 0);
        let path = abandoned(dir.path(), &host, "run-orphan");
        host.alive.store(true, Ordering::SeqCst);
        runtime.root(&*host).unwrap();
        assert!(path.exists());
        host.alive.store(false, Ordering::SeqCst);
        runtime.root(&*host).unwrap();
        assert!(!path.exists());
        assert!(active.path.exists());
        runtime.root(&*host).unwrap();
        assert_eq!(host.reclaims.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn unknown_corrupt_and_linked_workspaces_are_preserved_with_diagnostics() {
        let (dir, runtime, host) = setup();
        let unknown = dir.path().join("user-file");
        fs::write(&unknown, b"keep").unwrap();
        let corrupt = abandoned(dir.path(), &host, "run-corrupt");
        fs::write(corrupt.join("owner.json"), b"{").unwrap();
        let version = abandoned(dir.path(), &host, "run-future");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(version.join("owner.json")).unwrap()).unwrap();
        record["schema"] = 2.into();
        fs::write(
            version.join("owner.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let linked = abandoned(dir.path(), &host, "run-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&unknown, linked.join("foreign")).unwrap();
        runtime.root(&*host).unwrap();
        for path in [&unknown, &corrupt, &version, &linked] {
            assert!(path.exists());
        }
        assert_eq!(fs::read(unknown).unwrap(), b"keep");
        assert_eq!(host.reclaims.load(Ordering::SeqCst), 0);
        assert!(
            runtime
                .status()
                .diagnostics
                .iter()
                .all(|e| e.message.contains(dir.path().to_str().unwrap()))
        );
    }
    #[test]
    fn retained_result_shrinks_reservation_and_failed_cleanup_keeps_it_charged() {
        let (dir, runtime, host) = setup();
        let mut output = runtime
            .workspace(host.clone(), dir.path(), runtime.reserve().unwrap())
            .unwrap();
        fs::write(output.stage.join("query-records"), b"result").unwrap();
        output.retain().unwrap();
        let retained = runtime.status().reserved_bytes;
        assert!(retained > 6 && retained < TEMPORARY_CONTROL_BYTES);
        let reservation = runtime.reserve().unwrap();
        assert!(matches!(
            runtime.reserve(),
            Err(Error {
                code: ErrorCode::ResourceLimited,
                ..
            })
        ));
        drop(reservation);
        host.fail.store(true, Ordering::SeqCst);
        let path = output.path.clone();
        drop(output);
        assert_eq!(runtime.status().reserved_bytes, retained);
        assert_eq!(runtime.status().residue, vec![path.clone()]);
        assert!(!runtime.status().diagnostics.is_empty());
        host.fail.store(false, Ordering::SeqCst);
        runtime.root(&*host).unwrap();
        assert!(!path.exists());
        drop(runtime.reserve().unwrap());
        assert_eq!(runtime.status().reserved_bytes, 0);
    }
}
