//! Linux containment adapter. Each guard is a separate subreaper process; no
//! global process attributes or signal handlers are installed in an API caller.
mod linker;
pub use linker::Lld22;
mod guard;
mod procfs;
mod progress;
pub use progress::{WorkerEnvironment, now_ms};
#[cfg(test)]
mod tests;
use blobray_application::{OperationHost, OperationWorker, OwnerIdentity, WorkerReport};

use blobray_domain::*;
pub use guard::run_guard;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
};

pub fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
fn unavailable(message: impl ToString) -> Error {
    Error::new(ErrorCode::Unavailable, message.to_string())
}

#[derive(Clone)]
pub struct LinuxHost {
    binary: PathBuf,
    cgroup_root: Option<PathBuf>,
}
impl LinuxHost {
    pub fn new(binary: PathBuf, cgroup_root: Option<PathBuf>) -> Self {
        Self {
            binary,
            cgroup_root,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct GuardConfig {
    budget: ResourceBudget,
    cgroup: Option<PathBuf>,
    // Old stages remain reclaimable; only execution requires a deadline.
    #[serde(default)]
    deadline_ms: u64,
}

impl OperationHost for LinuxHost {
    fn temporary_root(&self, requested: Option<&Path>) -> Result<PathBuf> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        // SAFETY: geteuid has no preconditions and changes no process state.
        let uid = unsafe { libc::geteuid() };
        let root = requested
            .map(Path::to_owned)
            .unwrap_or_else(|| std::env::temp_dir().join(format!("blobray-next-{uid}")));
        match fs::DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(storage_io(e)),
        }
        let metadata = fs::symlink_metadata(&root).map_err(storage_io)?;
        if !metadata.is_dir() || metadata.uid() != uid || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(Error::new(
                ErrorCode::Unavailable,
                "temporary root must be a private owned directory (0700), not a symlink",
            ));
        }
        root.canonicalize().map_err(storage_io)
    }
    fn now_ms(&self) -> u64 {
        now_ms()
    }
    fn save_progress(&self, stage: &Path, record: &ProgressRecord) -> Result<()> {
        progress::save(stage, record)
    }
    fn owner(&self) -> Result<OwnerIdentity> {
        procfs::identity(std::process::id())
    }
    fn alive(&self, owner: &OwnerIdentity) -> Result<bool> {
        procfs::alive(owner)
    }
    fn reclaim(&self, stage: &Path) -> Result<()> {
        let path = stage.join("guard.json");
        if !path.exists() {
            return Ok(());
        }
        let config: GuardConfig = serde_json::from_slice(&read_control(&path)?)
            .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?;
        if let Some(group) = config.cgroup {
            if !group.exists() {
                return Ok(());
            }
            let expected = format!(
                "blobray-{}",
                stage
                    .file_name()
                    .ok_or_else(|| unavailable("stage identity missing"))?
                    .to_string_lossy()
            );
            if group.file_name() != Some(std::ffi::OsStr::new(&expected)) {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "containment identity differs from run",
                ));
            }
            let events = fs::read_to_string(group.join("cgroup.events")).map_err(io)?;
            if !events.lines().any(|line| line == "populated 0") {
                return Err(Error::new(
                    ErrorCode::Busy,
                    "run cgroup is still populated; recovery cannot remove active containment",
                ));
            }
            fs::remove_dir(group).map_err(io)?;
        }
        Ok(())
    }
    fn launch(
        &self,
        stage: &Path,
        budget: &ResourceBudget,
        deadline_ms: u64,
    ) -> Result<Box<dyn OperationWorker>> {
        budget.validate()?;
        let cgroup = if budget.mode == LimitMode::Kernel {
            let root = match &self.cgroup_root {
                Some(root) => root.clone(),
                None => procfs::current_cgroup()?,
            };
            Some(create_cgroup(
                &root,
                stage
                    .file_name()
                    .ok_or_else(|| unavailable("missing run identity"))?,
                budget.memory_bytes,
            )?)
        } else {
            None
        };
        let launch = (|| {
            let config = GuardConfig {
                deadline_ms,
                budget: budget.clone(),
                cgroup: cgroup.clone(),
            };
            blobray_application::write_control_message(
                File::create(stage.join("guard.json")).map_err(io)?,
                &config,
            )?;
            let output = File::create(stage.join("guard-report.json")).map_err(io)?;
            let mut child = Command::new(&self.binary)
                .arg("__guard")
                .arg(stage)
                .stdin(Stdio::piped())
                .stdout(output)
                .stderr(Stdio::null())
                .spawn()
                .map_err(io)?;
            let input = child.stdin.take();
            Ok(Box::new(Session {
                child,
                input,
                stage: stage.to_owned(),
                done: false,
                cgroup: cgroup.clone(),
            }) as Box<dyn OperationWorker>)
        })();
        if launch.is_err()
            && let Some(path) = cgroup
        {
            let _ = fs::remove_dir(path);
        }
        launch
    }
}

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    stage: PathBuf,
    done: bool,
    cgroup: Option<PathBuf>,
}
impl OperationWorker for Session {
    fn progress(&self) -> Result<Option<RunProgress>> {
        let run = self
            .stage
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| unavailable("invalid stage identity"))?
            .parse()?;
        blobray_application::read_progress(&self.stage, &run)
    }
    fn poll(&mut self) -> Result<Option<WorkerReport>> {
        let Some(status) = self.child.try_wait().map_err(io)? else {
            return Ok(None);
        };
        self.done = true;
        if !status.success() {
            return Err(Error::new(
                ErrorCode::WorkerExited,
                format!("guard exited unexpectedly: {status}"),
            ));
        }
        let bytes = read_control(&self.stage.join("guard-report.json"))?;
        let report: WorkerReport = serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?;
        if report.schema != 6 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported worker report",
            ));
        }
        Ok(Some(report))
    }
    fn cancel(&mut self) -> Result<()> {
        if let Some(input) = &mut self.input {
            match input.write_all(b"c") {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => (),
                Err(e) => return Err(io(e)),
            }
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.input.take(); // EOF cancels even when the coordinator disappears.
        if !self.done {
            let _ = self.child.wait();
        }
        if let Some(path) = &self.cgroup {
            let _ = fs::remove_dir(path);
        }
    }
}

fn read_control(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > 65536 {
        return Err(Error::new(
            ErrorCode::Integrity,
            "worker control message exceeds 64 KiB",
        ));
    }
    Ok(bytes)
}
fn create_cgroup(root: &Path, name: &std::ffi::OsStr, memory: u64) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(root.as_os_str().as_bytes())
        .map_err(|_| unavailable("NUL in cgroup path"))?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: NUL-terminated pathname and valid output storage are passed to statfs.
    if unsafe { libc::statfs(cpath.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(unavailable(std::io::Error::last_os_error()));
    }
    // SAFETY: statfs succeeded and initialized the structure.
    if unsafe { stat.assume_init() }.f_type != libc::CGROUP2_SUPER_MAGIC {
        return Err(unavailable(
            "kernel mode requires a delegated cgroup v2 directory",
        ));
    }
    OpenOptions::new()
        .write(true)
        .open(root.join("cgroup.subtree_control"))
        .and_then(|mut file| file.write_all(b"+memory"))
        .map_err(|e| unavailable(format!("cannot enable delegated memory controller: {e}")))?;
    let path = root.join(format!("blobray-{}", name.to_string_lossy()));
    fs::create_dir(&path).map_err(|e| {
        unavailable(format!(
            "cannot create delegated cgroup: {e}; select watchdog explicitly if intended"
        ))
    })?;
    let configured = (|| {
        for (file, value) in [
            ("memory.max", memory.to_string()),
            ("memory.swap.max", "0".into()),
            ("memory.oom.group", "1".into()),
        ] {
            OpenOptions::new()
                .write(true)
                .open(path.join(file))
                .and_then(|mut f| f.write_all(value.as_bytes()))
                .map_err(|e| unavailable(format!("cannot enforce {file}: {e}")))?;
        }
        OpenOptions::new()
            .write(true)
            .open(path.join("cgroup.kill"))
            .map_err(|e| unavailable(format!("cgroup.kill unavailable: {e}")))?;
        Ok(())
    })();
    if let Err(error) = configured {
        let _ = fs::remove_dir(&path);
        return Err(error);
    }
    Ok(path)
}
