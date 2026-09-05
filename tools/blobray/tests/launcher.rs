//! Exercise the public launcher with Rust subprocess fixtures, without shell tools.
#![cfg(target_os = "linux")]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "blobray cli пробел {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        // A bare filename must resolve against cwd, even with an empty PATH.
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), root.join("command")).unwrap();
        Self(root)
    }
    fn run(&self, backend: &str, usage: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_blobray-run"))
            .current_dir(&self.0)
            .env("PATH", "")
            .env("BLOBRAY_BINARY", "command")
            .env("BLOBRAY_LIMIT_BACKEND", backend)
            .env("BLOBRAY_REPORT_USAGE", usage)
            .env("BLOBRAY_LAUNCHER_FIXTURE", "1")
            .args(["--ignored", "--exact", "selected_command", "--nocapture"])
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "Rust subprocess fixture, launched by CLI tests"]
fn selected_command() {
    if std::env::var_os("BLOBRAY_LAUNCHER_FIXTURE").is_some()
        || std::env::current_dir()
            .unwrap()
            .join("systemd-fixture")
            .exists()
    {
        std::process::exit(23);
    }
}

#[test]
fn selected_local_binary_exit_and_usage_survive_empty_path_and_unicode_cwd() {
    let output = Fixture::new().run("watchdog", "1");
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("blobray usage:"));
}

#[test]
fn auto_uses_watchdog_when_systemd_is_unavailable() {
    let output = Fixture::new().run("auto", "0");
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn explicit_systemd_fails_when_unavailable() {
    let output = Fixture::new().run("systemd", "0");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requested user systemd resource scope is unavailable")
    );
}

#[test]
fn invalid_environment_settings_fail_closed() {
    for (backend, usage) in [("typo", "0"), ("watchdog", "yes")] {
        assert_eq!(Fixture::new().run(backend, usage).status.code(), Some(2));
    }
}

#[test]
#[ignore = "Rust process fixture for the explicit native systemd test"]
fn systemd_owned_worker() {
    let root = std::env::current_dir().unwrap();
    if !root.join("systemd-fixture").exists() {
        return;
    }
    // SAFETY: this isolated worker intentionally ignores TERM to test escalation.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    fs::write(root.join("pending"), std::process::id().to_string()).unwrap();
    fs::rename(root.join("pending"), root.join("ready")).unwrap();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires a live user systemd manager with memory-controller support; run explicitly"]
fn native_systemd_applies_limits_and_cancels_the_owned_service() {
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    fs::write(fixture.0.join("systemd-fixture"), "").unwrap();
    let completed = Command::new(env!("CARGO_BIN_EXE_blobray-run"))
        .current_dir(&fixture.0)
        .env("BLOBRAY_BINARY", "command")
        .env("BLOBRAY_LIMIT_BACKEND", "systemd")
        .env("BLOBRAY_REPORT_USAGE", "0")
        .args([
            "--ignored",
            "--exact",
            "selected_command",
            "--logfile=$HOME",
        ])
        .output()
        .unwrap();
    assert_eq!(
        completed.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );
    assert!(
        fixture.0.join("$HOME").exists(),
        "systemd must preserve literal argv"
    );
    struct Launcher(std::process::Child);
    impl Drop for Launcher {
        fn drop(&mut self) {
            // On assertion failure still ask the wrapper to clean up its service.
            if self.0.try_wait().ok().flatten().is_none() {
                // SAFETY: the live child handle belongs to this guard.
                unsafe {
                    libc::kill(self.0.id() as i32, libc::SIGTERM);
                }
                let _ = self.0.wait();
            }
        }
    }
    let mut launcher = Launcher(
        Command::new(env!("CARGO_BIN_EXE_blobray-run"))
            .current_dir(&fixture.0)
            .env("BLOBRAY_BINARY", "command")
            .env("BLOBRAY_LIMIT_BACKEND", "systemd")
            .env("BLOBRAY_REPORT_USAGE", "0")
            .args([
                "--ignored",
                "--exact",
                "systemd_owned_worker",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    let ready = fixture.0.join("ready");
    while !ready.exists() {
        assert!(
            launcher.0.try_wait().unwrap().is_none(),
            "required systemd backend unavailable or launch failed"
        );
        assert!(started.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = fs::read_to_string(ready).unwrap().parse().unwrap();
    let cgroup = fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
    let unit = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .and_then(|path| path.rsplit('/').next())
        .unwrap();
    assert!(unit.starts_with("blobray-"));
    let output = Command::new("systemctl")
        .args([
            "--user",
            "show",
            "--property=MemoryMax",
            "--property=MemorySwapMax",
            "--property=RuntimeMaxUSec",
            "--property=TimeoutStopUSec",
            "--property=KillMode",
            unit,
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let properties = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "MemoryMax=1073741824",
        "MemorySwapMax=0",
        "RuntimeMaxUSec=15min",
        "TimeoutStopUSec=10s",
        "KillMode=control-group",
    ] {
        assert!(
            properties.lines().any(|line| line == expected),
            "missing {expected}: {properties}"
        );
    }
    // SAFETY: launcher is still live and owned by this test.
    assert_eq!(
        unsafe { libc::kill(launcher.0.id() as i32, libc::SIGTERM) },
        0
    );
    let stopping = Instant::now();
    let status = loop {
        if let Some(status) = launcher.0.try_wait().unwrap() {
            break status;
        }
        assert!(stopping.elapsed() < Duration::from_secs(25));
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(137));
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "service worker was abandoned"
    );
}

#[test]
#[ignore = "Rust subprocess fixture for exact argument forwarding"]
fn selected_arguments() {
    let cwd = std::env::current_dir().unwrap();
    if cwd.join("argument-fixture").exists() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        fs::write(cwd.join("argv.json"), serde_json::to_vec(&args).unwrap()).unwrap();
    }
}

#[test]
fn absolute_dot_and_bare_paths_preserve_exact_argument_boundaries() {
    let fixture = Fixture::new();
    let host = fixture.0.join("selected host");
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &host).unwrap();
    fs::write(fixture.0.join("argument-fixture"), "").unwrap();
    let arguments = [
        "--ignored",
        "--exact",
        "selected_arguments",
        "--skip",
        "a path with spaces",
        "--skip",
        "$HOME",
        "--skip",
        "кириллица",
        "--nocapture",
    ];
    for selected in [
        host.as_path(),
        std::path::Path::new("./selected host"),
        std::path::Path::new("selected host"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_blobray-run"))
            .current_dir(&fixture.0)
            .env("PATH", "")
            .env("BLOBRAY_BINARY", selected)
            .env("BLOBRAY_LIMIT_BACKEND", "watchdog")
            .env("BLOBRAY_REPORT_USAGE", "0")
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let observed: Vec<String> =
            serde_json::from_slice(&fs::read(fixture.0.join("argv.json")).unwrap()).unwrap();
        assert_eq!(observed, arguments);
    }
}
