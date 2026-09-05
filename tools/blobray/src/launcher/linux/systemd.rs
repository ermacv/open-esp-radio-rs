use super::cancellation;
use crate::launcher::Config;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_UNIT: AtomicU64 = AtomicU64::new(0);

fn unit_name() -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "blobray-{}-{nonce}-{}.service",
        std::process::id(),
        NEXT_UNIT.fetch_add(1, Ordering::Relaxed)
    )
}

// Control commands are also bounded; a hung user manager must not hang cleanup.
fn control(args: &[&str], timeout: Duration) -> Option<ExitStatus> {
    let mut child = Command::new("systemctl")
        .args(["--user"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

struct Unit {
    name: String,
    client: Child,
}

impl Drop for Unit {
    fn drop(&mut self) {
        // The service is a sibling, not a descendant of systemd-run. Killing the
        // client alone would abandon the actual analysis and its cgroup.
        control(
            &["kill", "--signal=KILL", &self.name],
            Duration::from_secs(2),
        );
        control(&["stop", "--no-block", &self.name], Duration::from_secs(2));
        let _ = self.client.kill();
        let _ = self.client.wait();
    }
}

fn command(name: &str, runtime: &str) -> Command {
    let mut command = Command::new("systemd-run");
    command
        .args([
            "--user",
            "--pipe",
            "--wait",
            "--collect",
            "--quiet",
            "--expand-environment=no",
        ])
        .arg(format!("--unit={name}"))
        .args([
            "--property=MemoryMax=1G",
            "--property=MemorySwapMax=0",
            "--property=TimeoutStopSec=10s",
            "--property=KillMode=control-group",
        ])
        .arg(format!("--property=RuntimeMaxSec={runtime}"));
    command
}

pub(super) fn available(received: &AtomicUsize) -> Result<bool, String> {
    if !control(&["show-environment"], Duration::from_secs(3))
        .is_some_and(|status| status.success())
    {
        return Ok(false);
    }
    let name = unit_name();
    let Ok(client) = command(&name, "10s")
        .args(["--", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(false);
    };
    let mut unit = Unit { name, client };
    let started = Instant::now();
    loop {
        if let Some(status) = unit.client.try_wait().map_err(|error| error.to_string())? {
            return Ok(status.success());
        }
        if cancellation(received).is_some() || started.elapsed() > Duration::from_secs(12) {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) fn run(config: &Config, received: &AtomicUsize) -> Result<u8, String> {
    if cancellation(received).is_some() {
        return Ok(137);
    }
    let name = unit_name();
    let mut command = command(&name, "15min");
    let mut directory = std::ffi::OsString::from("--working-directory=");
    directory.push(std::env::current_dir().map_err(|error| error.to_string())?);
    command.arg(directory);
    command.arg("--").arg(&config.binary).args(&config.args);
    let client = command
        .spawn()
        .map_err(|error| format!("cannot launch systemd resource scope: {error}"))?;
    let mut unit = Unit { name, client };
    let mut stopping = None;
    loop {
        if let Some(reason) = cancellation(received).filter(|_| stopping.is_none()) {
            eprintln!("error: blobray resource scope stopped the command: {reason}");
            control(&["stop", "--no-block", &unit.name], Duration::from_secs(2));
            stopping = Some(Instant::now());
        }
        if let Some(status) = unit.client.try_wait().map_err(|error| error.to_string())? {
            return Ok(if stopping.is_some() {
                137
            } else {
                status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)) as u8
            });
        }
        if stopping.is_some_and(|started| started.elapsed() >= Duration::from_secs(10)) {
            return Ok(137); // Unit drop kills the cgroup and reaps systemd-run.
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
