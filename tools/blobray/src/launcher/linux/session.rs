use super::cancellation;
use super::procfs::{self, Member, Sampler};
use crate::launcher::Limits;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

struct Session {
    child: Child,
    id: i32,
    members: Vec<Member>,
    armed: bool,
}

impl Session {
    fn spawn(mut command: Command) -> Result<Self, String> {
        // SAFETY: the child hook only invokes async-signal-safe setsid and returns
        // an OS error. It performs no allocations or locking after fork.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|error| format!("cannot launch selected Blobray: {error}"))?;
        Ok(Self {
            id: child.id() as i32,
            child,
            members: Vec::new(),
            armed: true,
        })
    }

    fn signal(&self, signal: i32) {
        // SAFETY: this private session/process group belongs to this guard.
        unsafe {
            libc::kill(-self.id, signal);
        }
        for member in &self.members {
            // Avoid signalling a recycled PID from an earlier observation.
            if procfs::is_same_process(*member) {
                // SAFETY: kill has no pointer arguments.
                unsafe {
                    libc::kill(member.pid, signal);
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.armed {
            self.signal(libc::SIGKILL);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub(super) fn run(
    command: Command,
    limits: Limits,
    report_usage: bool,
    received: &AtomicUsize,
    sampler: &mut impl Sampler,
) -> Result<u8, String> {
    let mut session = Session::spawn(command)?;
    let started = Instant::now();
    let mut peak = 0;
    let mut status = None;
    let reason = loop {
        status = status.or(session
            .child
            .try_wait()
            .map_err(|error| format!("cannot wait for Blobray: {error}"))?);
        let sample = match sampler.sample(session.id) {
            Ok(sample) => sample,
            Err(error) => {
                break Some(format!(
                    "could not inspect process-session resource usage: {error}"
                ));
            }
        };
        session.members = sample.members;
        peak = peak.max(sample.rss_bytes);
        if let Some(reason) = cancellation(received) {
            break Some(reason.to_owned());
        }
        if session.members.is_empty() && status.is_some() {
            break None;
        }
        if sample.rss_bytes > limits.memory_bytes {
            break Some(format!(
                "process-tree RSS exceeded {} bytes (observed {} KiB)",
                limits.memory_bytes,
                sample.rss_bytes / 1024
            ));
        }
        if started.elapsed() >= limits.deadline {
            break Some(format!(
                "runtime exceeded {} seconds",
                limits.deadline.as_secs()
            ));
        }
        std::thread::sleep(limits.poll);
    };
    if let Some(reason) = reason {
        session.signal(libc::SIGTERM);
        let stopping = Instant::now();
        while stopping.elapsed() < limits.grace {
            // Even after the leader exits, workers in the same session remain owned.
            let _ = session.child.try_wait();
            match sampler.sample(session.id) {
                Ok(sample) => {
                    session.members = sample.members;
                    if session.members.is_empty() {
                        break;
                    }
                }
                Err(_) => break,
            }
            std::thread::sleep(limits.poll);
        }
        session.signal(libc::SIGKILL);
        session
            .child
            .wait()
            .map_err(|error| format!("cannot reap stopped Blobray: {error}"))?;
        eprintln!("error: blobray resource watchdog stopped the command: {reason}");
        eprintln!("peak RSS: {} KiB", peak / 1024);
        // Drop repeats best-effort cleanup in case descendants appeared during shutdown.
        return Ok(137);
    }
    session.armed = false;
    if report_usage {
        eprintln!(
            "blobray usage: elapsed={}s peak_session_rss={} KiB",
            started.elapsed().as_secs(),
            peak / 1024
        );
    }
    let status = status.expect("a completed session has a reaped leader");
    Ok(status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)) as u8)
}
