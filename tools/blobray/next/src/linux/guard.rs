use super::*;
use std::{
    io::Read,
    os::{
        fd::AsRawFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    thread,
    time::{Duration, Instant},
};

/// Internal host entry point. Owns its subreaper status, stdin lifetime channel,
/// worker containment and descendant reaping until it writes one terminal report.
pub fn run_guard(stage: &Path) -> Result<()> {
    let config: GuardConfig = serde_json::from_slice(&read_control(&stage.join("guard.json"))?)
        .map_err(|e| unavailable(e.to_string()))?;
    let mut command = Command::new(std::env::current_exe().map_err(io)?);
    command.arg("__worker").arg(stage);
    let report = run_session(stage, &config, command)
        .unwrap_or_else(|error| failure(error.code, error.message));
    blobray_application::write_control_message(std::io::stdout().lock(), &report)?;
    Ok(())
}

pub(super) fn run_session(
    stage: &Path,
    config: &GuardConfig,
    command: Command,
) -> Result<WorkerReport> {
    config.budget.validate()?;
    if config.deadline_ms == 0 {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "guard configuration has no operation deadline",
        ));
    }
    if (config.budget.mode == LimitMode::Kernel) != config.cgroup.is_some() {
        return Err(Error::new(
            ErrorCode::Integrity,
            "guard containment does not match requested limit mode",
        ));
    }
    let lease = OpenOptions::new()
        .read(true)
        .write(true)
        .open(stage.join("lease.lock"))
        .map_err(io)?;
    lease.lock_shared().map_err(io)?;
    // SAFETY: these process-local settings are installed only in the dedicated guard.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    // SAFETY: fd 0 is the guard's private lifetime pipe, never the caller's stdin.
    let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    let mut result = execute(stage, config, command)?;
    if let Some(group) = &config.cgroup
        && let Err(error) = fs::remove_dir(group)
    {
        result.diagnostics.secondary(io(error));
    }
    Ok(result)
}

fn cancelled() -> Result<bool> {
    let mut byte = [0u8; 1];
    match std::io::stdin().read(&mut byte) {
        Ok(_) => Ok(true), // A command or EOF (coordinator death).
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(false),
        Err(e) => Err(io(e)),
    }
}
fn failure(code: ErrorCode, message: impl Into<String>) -> WorkerReport {
    let state = match code {
        ErrorCode::Cancelled => RunState::Cancelled,
        ErrorCode::TimedOut => RunState::TimedOut,
        ErrorCode::ResourceLimited => RunState::ResourceLimited,
        _ => RunState::Failed,
    };
    let mut message = message.into();
    truncate_message(&mut message);
    WorkerReport {
        schema: 6,
        diagnostics: RunDiagnostics::default(),
        state,
        prepared: None,
        error: Some(Error::new(code, message)),
    }
}
pub(super) fn execute(
    stage: &Path,
    config: &GuardConfig,
    mut command: Command,
) -> Result<WorkerReport> {
    if cancelled()? {
        return Ok(failure(
            ErrorCode::Cancelled,
            "coordinator cancelled before launch",
        ));
    }
    let cgroup_file = config
        .cgroup
        .as_ref()
        .map(|p| {
            OpenOptions::new()
                .write(true)
                .open(p.join("cgroup.procs"))
                .map_err(io)
        })
        .transpose()?;
    let parent = std::process::id();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    // SAFETY: only async-signal-safe syscalls run between fork and exec. The open
    // cgroup fd is owned by this closure and contains no Rust locks or allocators.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent as i32 {
                libc::_exit(125);
            }
            if let Some(file) = &cgroup_file
                && libc::write(file.as_raw_fd(), b"0\n".as_ptr().cast(), 2) != 2
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = OwnedTree {
        child: command.spawn().map_err(io)?,
        cgroup: config.cgroup.clone(),
    };
    let mut stderr = StderrTail::new(child.stderr.take().expect("piped stderr"))?;
    let mut peak_rss = None;
    let mut observed_oom = false;
    let mut stop: Option<WorkerReport> = None;
    let mut stopped_at = None;
    let mut status = None;
    loop {
        if let Err(error) = stderr.drain() {
            stop.get_or_insert_with(|| failure(error.code, error.message));
        }
        if status.is_none() {
            status = child.try_wait().map_err(io)?;
        }
        let processes = match procfs::descendants(std::process::id()) {
            Ok(processes) => processes,
            Err(error) => {
                // Losing observation must not silently disable enforcement.
                stop.get_or_insert_with(|| failure(error.code, error.message));
                if let Some(group) = &config.cgroup {
                    let _ = fs::write(group.join("cgroup.kill"), b"1");
                }
                let _ = child.kill();
                Vec::new()
            }
        };
        let oom = config.cgroup.as_ref().is_some_and(|group| {
            fs::read_to_string(group.join("memory.events")).is_ok_and(|text| {
                text.lines().any(|line| {
                    line.strip_prefix("oom_kill ")
                        .is_some_and(|n| n.parse::<u64>().unwrap_or(0) > 0)
                })
            })
        });
        observed_oom |= oom;
        let rss = processes
            .iter()
            .fold(0u64, |sum, p| sum.saturating_add(p.rss));
        if !processes.is_empty() {
            peak_rss = Some(peak_rss.unwrap_or(0).max(rss));
        }
        if stop.is_none() {
            if oom {
                stop = Some(failure(
                    ErrorCode::ResourceLimited,
                    "cgroup memory limit exhausted",
                ));
            } else if cancelled()? {
                stop = Some(failure(
                    ErrorCode::Cancelled,
                    "import cancelled or coordinator exited",
                ));
            } else if now_ms() >= config.deadline_ms {
                stop = Some(failure(ErrorCode::TimedOut, "import time budget exhausted"));
            } else if config.budget.mode == LimitMode::Watchdog && rss > config.budget.memory_bytes
            {
                stop = Some(failure(
                    ErrorCode::ResourceLimited,
                    "sampled process-tree RSS exceeded budget",
                ));
            }
        }
        if status.is_some() && stop.is_none() && !processes.is_empty() {
            stop = Some(failure(
                ErrorCode::Storage,
                "worker exited with live descendants",
            ));
        }
        if stop.is_some() {
            let elapsed = stopped_at.get_or_insert_with(Instant::now).elapsed();
            let signal = if elapsed >= Duration::from_millis(config.budget.grace_ms) {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            };
            for process in &processes {
                procfs::signal(process, signal);
            }
            if signal == libc::SIGKILL
                && let Some(group) = &config.cgroup
            {
                let _ = fs::write(group.join("cgroup.kill"), b"1");
            }
        }
        if status.is_some() {
            // SAFETY: the dedicated guard owns all adopted children. Never reap the
            // main worker before Child has obtained its exit status.
            loop {
                let waited = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
                if waited <= 0 {
                    break;
                }
            }
            if processes.is_empty() {
                break;
            }
        }
        thread::sleep(Duration::from_millis(config.budget.poll_ms));
    }
    let _ = stderr.drain().map_err(|error| {
        stop.get_or_insert_with(|| failure(error.code, error.message));
    });
    let worker_report = (|| -> Result<WorkerReport> {
        let report: WorkerReport = serde_json::from_slice(
            &read_control(&stage.join("worker-report.json")).map_err(|e| {
                Error::new(
                    ErrorCode::WorkerProtocol,
                    format!("missing/unreadable worker report: {e}"),
                )
            })?,
        )
        .map_err(|e| Error::new(ErrorCode::WorkerProtocol, e.to_string()))?;
        if report.schema != 6 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported worker report",
            ));
        }
        Ok(report)
    })();
    let mut report = if let Some(mut stopped) = stop {
        if let Ok(worker) = &worker_report {
            stopped.diagnostics = worker.diagnostics.clone();
            if let Some(error) = &worker.error {
                stopped.diagnostics.secondary(error.clone());
            }
        }
        stopped
    } else if !status.is_some_and(|status| status.success()) {
        failure(ErrorCode::WorkerExited, "worker exited unsuccessfully")
    } else {
        match worker_report {
            Ok(report) => report,
            Err(error) => failure(error.code, error.message),
        }
    };
    if report.diagnostics.progress.is_none() && stage.join("progress.json").exists() {
        let progress = (|| {
            let run = stage
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| unavailable("missing run identity"))?
                .parse()?;
            blobray_application::read_progress(stage, &run)
        })();
        match progress {
            Ok(progress) => report.diagnostics.progress = progress,
            Err(error) => report.diagnostics.secondary(error),
        }
    }
    report.diagnostics.exit = status.map(|status| ExitObservation {
        code: status.code(),
        signal: status.signal(),
        cgroup_oom: observed_oom,
    });
    report.diagnostics.memory = match &config.cgroup {
        Some(group) => fs::read_to_string(group.join("memory.peak"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .map(|peak_bytes| MemoryObservation {
                source: MemorySource::CgroupPeak,
                peak_bytes,
            }),
        None => peak_rss.map(|peak_bytes| MemoryObservation {
            source: MemorySource::SampledTreeRss,
            peak_bytes,
        }),
    };
    report.diagnostics.stderr_tail = stderr.bytes();
    report.diagnostics.stderr_truncated = stderr.truncated;
    if let Some(error) = &mut report.error {
        truncate_message(&mut error.message);
    }
    Ok(report)
}

// Error paths own the same teardown as cancellation; Child's default Drop would
// otherwise detach a worker when observation or result decoding fails.
struct OwnedTree {
    child: Child,
    cgroup: Option<PathBuf>,
}
impl std::ops::Deref for OwnedTree {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl std::ops::DerefMut for OwnedTree {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
impl Drop for OwnedTree {
    fn drop(&mut self) {
        if let Some(group) = &self.cgroup {
            let _ = fs::write(group.join("cgroup.kill"), b"1");
        }
        if let Ok(processes) = procfs::descendants(std::process::id()) {
            for process in &processes {
                procfs::signal(process, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        loop {
            if let Ok(processes) = procfs::descendants(std::process::id()) {
                for process in &processes {
                    procfs::signal(process, libc::SIGKILL);
                }
            }
            // SAFETY: only this dedicated guard owns these adopted children.
            let waited = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            if waited < 0 {
                break;
            }
            if waited == 0 {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Fixed-size tail; drain work per monitor iteration is bounded so continuous
/// stderr output cannot starve cancellation, timeout or child reaping.
struct StderrTail {
    stream: std::process::ChildStderr,
    buffer: [u8; 8192],
    cursor: usize,
    length: usize,
    truncated: bool,
}
impl StderrTail {
    fn new(stream: std::process::ChildStderr) -> Result<Self> {
        // SAFETY: the owned descriptor remains valid for both fcntl calls.
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(io(std::io::Error::last_os_error()));
        }
        Ok(Self {
            stream,
            buffer: [0; 8192],
            cursor: 0,
            length: 0,
            truncated: false,
        })
    }
    fn drain(&mut self) -> Result<()> {
        let mut buffer = [0; 4096];
        for _ in 0..16 {
            match self.stream.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    for byte in &buffer[..count] {
                        self.truncated |= self.length == self.buffer.len();
                        self.buffer[self.cursor] = *byte;
                        self.cursor = (self.cursor + 1) % self.buffer.len();
                        self.length = (self.length + 1).min(self.buffer.len());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(io(error)),
            }
        }
        Ok(())
    }
    fn bytes(&self) -> Vec<u8> {
        let start = (self.cursor + self.buffer.len() - self.length) % self.buffer.len();
        (0..self.length)
            .map(|i| self.buffer[(start + i) % self.buffer.len()])
            .collect()
    }
}
