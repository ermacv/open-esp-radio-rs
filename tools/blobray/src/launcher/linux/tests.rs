use super::procfs::{Procfs, Sample, Sampler};
use super::{Signals, session};
use crate::launcher::{Limits, selected_binary};
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "blobray launcher тест {} {}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn command(&self, name: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                &format!("launcher::linux::tests::{name}"),
                "--nocapture",
            ])
            .env("BLOBRAY_TEST_ROOT", &self.root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    fn ready(&self) -> PathBuf {
        self.root.join("ready")
    }

    fn wait_ready(&self, child: &mut Child) {
        let started = Instant::now();
        while !self.ready().exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "fixture exited before readiness"
            );
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "fixture readiness timeout"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_stopped(&self) {
        let Some(pid) = fs::read_to_string(self.ready())
            .ok()
            .and_then(|text| text.parse::<i32>().ok())
        else {
            return;
        };
        let started = Instant::now();
        while let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
            // Orphan zombies await PID 1; they cannot execute or retain RSS.
            if stat
                .rsplit_once(')')
                .unwrap()
                .1
                .trim_start()
                .starts_with('Z')
            {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "owned process {pid} still alive"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(pid) = fs::read_to_string(self.ready())
            .ok()
            .and_then(|text| text.parse::<i32>().ok())
        {
            // SAFETY: this PID was published by this test's private child.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn small_limits() -> Limits {
    Limits {
        deadline: Duration::from_secs(2),
        grace: Duration::from_millis(100),
        ..Limits::default()
    }
}

fn root() -> Option<PathBuf> {
    std::env::var_os("BLOBRAY_TEST_ROOT").map(PathBuf::from)
}

#[test]
#[ignore = "Rust subprocess fixture, launched only by process tests"]
fn fixture_exit() {
    if root().is_some() {
        std::process::exit(23);
    }
}

#[test]
#[ignore = "Rust subprocess fixture, launched only by process tests"]
fn fixture_hold() {
    let Some(root) = root() else {
        return;
    };
    // SAFETY: this isolated fixture intentionally ignores TERM to test escalation.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
    let memory = if std::env::var_os("BLOBRAY_TEST_ALLOCATE").is_some() {
        vec![0x5au8; 32 * 1024 * 1024]
    } else {
        Vec::new()
    };
    fs::write(root.join("pending"), std::process::id().to_string()).unwrap();
    fs::rename(root.join("pending"), root.join("ready")).unwrap();
    loop {
        std::hint::black_box(&memory);
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "Rust subprocess fixture, launched only by process tests"]
fn fixture_parent_exits() {
    let Some(root) = root() else {
        return;
    };
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "launcher::linux::tests::fixture_hold",
            "--nocapture",
        ])
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !root.join("ready").exists() {
        assert!(child.try_wait().unwrap().is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    // The worker is deliberately reparented and uses a distinct process group.
    std::process::exit(23);
}

#[test]
#[ignore = "Rust subprocess fixture, launched only by process tests"]
fn fixture_launcher() {
    let Some(root) = root() else {
        return;
    };
    let signals = Signals::install().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "launcher::linux::tests::fixture_hold",
            "--nocapture",
        ])
        .env("BLOBRAY_TEST_ROOT", root)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = session::run(
        command,
        Limits::default(),
        true,
        &signals.received,
        &mut Procfs::default(),
    )
    .unwrap();
    std::process::exit(i32::from(status));
}

#[test]
fn command_exit_status_is_preserved() {
    let fixture = Fixture::new();
    let status = session::run(
        fixture.command("fixture_exit"),
        small_limits(),
        true,
        &AtomicUsize::new(0),
        &mut Procfs::default(),
    )
    .unwrap();
    assert_eq!(status, 23);
}

#[derive(Clone, Copy)]
enum Fault {
    Enumeration,
    Rss,
    Malformed,
    Empty,
}

struct FaultAfterReady {
    fixture: PathBuf,
    procfs: Procfs,
    fault: Fault,
}

impl Sampler for FaultAfterReady {
    fn sample(&mut self, session: i32) -> io::Result<Sample> {
        if !self.fixture.join("ready").exists() {
            return self.procfs.sample(session);
        }
        let fake = self.fixture.join("proc");
        match self.fault {
            Fault::Enumeration => self.procfs.root = fake.join("does-not-exist"),
            Fault::Empty => {
                fs::create_dir_all(&fake)?;
                self.procfs.root = fake;
            }
            Fault::Rss | Fault::Malformed => {
                let own = fake.join(std::process::id().to_string());
                fs::create_dir_all(&own)?;
                let record = if matches!(self.fault, Fault::Rss) {
                    let stat = fs::read_to_string("/proc/self/stat")?;
                    let (prefix, fields) = stat.rsplit_once(')').unwrap();
                    let mut fields: Vec<_> = fields.split_whitespace().collect();
                    fields[21] = "missing-rss";
                    format!("{prefix}) {}", fields.join(" "))
                } else {
                    "not a process record".to_owned()
                };
                fs::write(own.join("stat"), record)?;
                self.procfs.root = fake;
            }
        }
        self.procfs.sample(session)
    }
}

fn monitoring_failure(fault: Fault) {
    let fixture = Fixture::new();
    let mut sampler = FaultAfterReady {
        fixture: fixture.root.clone(),
        procfs: Procfs::default(),
        fault,
    };
    let status = session::run(
        fixture.command("fixture_hold"),
        small_limits(),
        false,
        &AtomicUsize::new(0),
        &mut sampler,
    )
    .unwrap();
    assert_eq!(status, 137);
    assert!(fixture.ready().exists());
    fixture.assert_stopped();
}

#[test]
fn process_enumeration_failure_stops_the_command() {
    monitoring_failure(Fault::Enumeration);
}
#[test]
fn rss_query_failure_cannot_be_treated_as_zero_usage() {
    monitoring_failure(Fault::Rss);
}
#[test]
fn malformed_process_output_stops_the_command() {
    monitoring_failure(Fault::Malformed);
}
#[test]
fn empty_process_table_cannot_hide_the_live_command() {
    monitoring_failure(Fault::Empty);
}

#[test]
fn term_escalates_when_the_command_ignores_it() {
    let fixture = Fixture::new();
    let mut launcher = ChildGuard(fixture.command("fixture_launcher").spawn().unwrap());
    fixture.wait_ready(&mut launcher.0);
    let started = Instant::now();
    // SAFETY: the handle is still owned and was just checked alive.
    assert_eq!(
        unsafe { libc::kill(launcher.0.id() as i32, libc::SIGTERM) },
        0
    );
    let status = loop {
        if let Some(status) = launcher.0.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(25));
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(137));
    assert!(started.elapsed() >= Limits::default().grace);
    fixture.assert_stopped();
}

#[test]
fn actual_memory_allocation_exceeds_the_test_budget() {
    let fixture = Fixture::new();
    let mut command = fixture.command("fixture_hold");
    command.env("BLOBRAY_TEST_ALLOCATE", "1");
    let limits = Limits {
        memory_bytes: 8 * 1024 * 1024,
        deadline: Duration::from_secs(10),
        ..small_limits()
    };
    let started = Instant::now();
    assert_eq!(
        session::run(
            command,
            limits,
            false,
            &AtomicUsize::new(0),
            &mut Procfs::default()
        )
        .unwrap(),
        137
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "memory limit must precede deadline"
    );
    fixture.assert_stopped();
}

#[test]
fn runtime_limit_terminates_reparented_worker_in_another_process_group() {
    let fixture = Fixture::new();
    let limits = small_limits();
    let started = Instant::now();
    assert_eq!(
        session::run(
            fixture.command("fixture_parent_exits"),
            limits,
            false,
            &AtomicUsize::new(0),
            &mut Procfs::default()
        )
        .unwrap(),
        137
    );
    assert!(started.elapsed() >= limits.deadline);
    fixture.assert_stopped();
}

#[test]
fn missing_binary_is_an_error() {
    let fixture = Fixture::new();
    assert!(selected_binary(&fixture.root.join("missing")).is_err());
}

#[test]
fn selected_binary_preserves_unicode_and_spaces() {
    let fixture = Fixture::new();
    let binary = fixture.root.join("выбранный command");
    std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &binary).unwrap();
    assert_eq!(
        selected_binary(&binary).unwrap(),
        std::env::current_exe().unwrap().canonicalize().unwrap()
    );
}
