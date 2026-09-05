//! A Unix process group owns the command and its ordinary descendants.
//!
//! This is lifecycle cleanup, not a resource limit or a sandbox for processes
//! that deliberately move to another process group or session.

use crate::Result;
use std::{
    process::{ChildStderr, ChildStdout, Command, ExitStatus},
    time::{Duration, Instant},
};

pub struct Child {
    child: std::process::Child,
    group: i32,
    finished: bool,
    shutdown_grace: Duration,
}
impl Child {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        Self::spawn_with_shutdown_grace(command, Duration::from_secs(1))
    }
    pub fn spawn_with_shutdown_grace(
        command: &mut Command,
        shutdown_grace: Duration,
    ) -> Result<Self> {
        if super::cancellation_requested() {
            return Err("command cancelled before start".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            let child = command
                .spawn()
                .map_err(|error| format!("cannot start {command:?}: {error}"))?;
            let group = i32::try_from(child.id()).expect("Unix PID fits pid_t");
            Ok(Self {
                child,
                group,
                finished: false,
                shutdown_grace,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = command;
            Err("xtask process-group ownership is unsupported on this host".into())
        }
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }
    pub fn wait(&mut self) -> Result<ExitStatus> {
        loop {
            if super::cancellation_requested() {
                self.cleanup();
                return Err("command cancelled by signal".into());
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.cleanup();
                    return Ok(status);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    self.cleanup();
                    return Err(error.into());
                }
            }
        }
    }
    #[cfg(unix)]
    fn signal(&self, signal: i32) {
        // SAFETY: the negative PID names only the process group created for
        // this owned child; no caller supplies an arbitrary process identifier.
        unsafe {
            libc::kill(-self.group, signal);
        }
    }
    #[cfg(unix)]
    fn group_exists(&self) -> bool {
        // SAFETY: signal zero only observes the group created during spawn.
        unsafe { libc::kill(-self.group, 0) == 0 }
    }
    fn cleanup(&mut self) {
        if self.finished {
            return;
        }
        #[cfg(unix)]
        {
            self.signal(libc::SIGTERM);
            let deadline = Instant::now() + self.shutdown_grace;
            while self.group_exists() && Instant::now() < deadline {
                let _ = self.child.try_wait();
                std::thread::sleep(Duration::from_millis(10));
            }
            self.signal(libc::SIGKILL);
        }
        let _ = self.child.wait();
        self.finished = true;
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        self.cleanup();
    }
}
