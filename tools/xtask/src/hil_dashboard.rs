//! `cargo hil dashboard`: a live page of the stand on this host.
//!
//! A single-threaded HTTP server bound to the loopback interface serves one
//! page and `status.json`: the arbiter's holders, queue, boards and recent
//! leases, plus the newest runs of the shared run store. The page polls the
//! JSON, so it reflects the stand without reloading. The server only reads
//! the arbiter's state and the run manifests.
//!
//! One dashboard serves the host. It names itself in `dashboard.json` of the
//! arbiter directory: a second start of the same build prints the running
//! one's address, and a different build stops it and takes over. A running
//! dashboard exits once another replaced it, or once the stand's state has a
//! schema newer than it reads, instead of serving errors.
use std::{
    io::{BufRead as _, BufReader, Write as _},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
};

use serde_json::{Value, json};

use crate::{Result, hil_runs};

/// The port the page is bookmarked at; `--port` overrides it.
const DEFAULT_PORT: u16 = 8765;
const RECENT_LEASES: usize = 30;
const RECENT_RUNS: usize = 25;
const PAGE: &str = include_str!("hil_dashboard.html");
/// How often the host fixtures are probed again.
const FIXTURE_REFRESH: std::time::Duration = std::time::Duration::from_secs(60);

/// The host fixtures as last probed, refreshed by a background thread.
static FIXTURES: std::sync::Mutex<Vec<crate::hil_fixtures::Fixture>> =
    std::sync::Mutex::new(Vec::new());

/// Probe the host fixtures now and every [`FIXTURE_REFRESH`] after.
fn watch_fixtures() {
    let Ok(lab) = oer_hil_runner_core::lab::config::LabConfig::default_path() else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            let fixtures = crate::hil_fixtures::probe(&lab);
            *FIXTURES
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = fixtures;
            std::thread::sleep(FIXTURE_REFRESH);
        }
    });
}

pub fn serve(runs: &Path, args: &[std::ffi::OsString]) -> Result<std::process::ExitCode> {
    let port = match args {
        [] => DEFAULT_PORT,
        [flag, port] if flag == "--port" => port
            .to_str()
            .and_then(|port| port.parse().ok())
            .ok_or("--port takes a port number")?,
        _ => return Err("usage: cargo hil dashboard [--port PORT]".into()),
    };
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let record = arbiter.directory().join("dashboard.json");
    let me = Instance::current(port)?;
    if let Some(running) = Instance::read(&record).filter(Instance::alive) {
        if running.build == me.build {
            eprintln!(
                "hil dashboard: already served at http://127.0.0.1:{}",
                running.port
            );
            return Ok(std::process::ExitCode::SUCCESS);
        }
        eprintln!(
            "hil dashboard: replacing the dashboard of another build (pid {})",
            running.pid
        );
        running.stop();
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|error| format!("cannot listen on 127.0.0.1:{port}: {error}"))?;
    me.write(&record)?;
    watch_fixtures();
    eprintln!(
        "hil dashboard: http://{} (Ctrl+C stops it)",
        listener.local_addr()?
    );
    listener.set_nonblocking(true)?;
    let mut checked = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false)?;
                // A client that disconnects mid-request only loses its response.
                let _ = handle(stream, runs);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => {}
        }
        if oer_process::cancellation_requested() {
            me.withdraw(&record);
            eprintln!("hil dashboard: stopped");
            return Ok(std::process::ExitCode::SUCCESS);
        }
        if checked.elapsed() >= std::time::Duration::from_secs(2) {
            checked = std::time::Instant::now();
            if Instance::read(&record).is_none_or(|current| current.pid != me.pid) {
                eprintln!("hil dashboard: another dashboard took over; exiting");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            if arbiter.outdated() {
                eprintln!(
                    "hil dashboard: the stand's state is newer than this build reads; \
                     exiting. Start the dashboard of the current build."
                );
                let _ = std::fs::remove_file(&record);
                return Ok(std::process::ExitCode::SUCCESS);
            }
        }
    }
}

/// A running dashboard, as `dashboard.json` names it.
#[derive(Clone, Debug, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
struct Instance {
    pid: u32,
    /// When the process started, telling it from a later process with a
    /// reused PID.
    started_unix_millis: u64,
    port: u16,
    /// The executable and its modification time: another build differs.
    build: String,
}

impl Instance {
    fn current(port: u16) -> Result<Self> {
        let exe = std::env::current_exe()?;
        let modified = std::fs::metadata(&exe)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        Ok(Self {
            pid: std::process::id(),
            started_unix_millis: oer_hil_arbiter::process_started_unix_millis(std::process::id())
                .ok_or("cannot read this process's start time")?,
            port,
            build: format!("{}@{modified}", exe.display()),
        })
    }

    fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    fn write(&self, path: &Path) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("no parent")?)?;
        serde_json::to_writer(&mut file, self)?;
        file.persist(path)?;
        Ok(())
    }

    fn alive(&self) -> bool {
        oer_hil_arbiter::process_started_unix_millis(self.pid) == Some(self.started_unix_millis)
    }

    /// Remove `record` when it still names this dashboard, so a stopped
    /// dashboard leaves no record behind while another's stays.
    fn withdraw(&self, record: &Path) {
        if Instance::read(record).is_some_and(|current| current == *self) {
            let _ = std::fs::remove_file(record);
        }
    }

    /// Stop this dashboard and wait up to five seconds for its port.
    fn stop(&self) {
        if let Some(pid) = rustix::process::Pid::from_raw(self.pid as i32) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
        for _ in 0..50 {
            if !self.alive() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

fn handle(mut stream: TcpStream, runs: &Path) -> Result<()> {
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut reader = BufReader::new(&stream);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // Headers are not needed; drain them so the client sees a clean close.
    let mut header = String::new();
    while reader.read_line(&mut header)? > 2 {
        header.clear();
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let (status, content_type, body) = respond(path, runs);
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body.as_bytes())?;
    Ok(())
}

fn respond(path: &str, runs: &Path) -> (&'static str, &'static str, String) {
    match path.split('?').next().unwrap_or(path) {
        "/" => ("200 OK", "text/html; charset=utf-8", PAGE.to_owned()),
        path if path.starts_with("/runs/") => match run_report(runs, &path["/runs/".len()..]) {
            Some(report) => ("200 OK", "text/html; charset=utf-8", report),
            None => (
                "404 Not Found",
                "text/plain",
                String::from("no such run report"),
            ),
        },
        path if path.starts_with("/jobs/") => match job_log(&path["/jobs/".len()..]) {
            Some(log) => ("200 OK", "text/plain; charset=utf-8", log),
            None => (
                "404 Not Found",
                "text/plain",
                String::from("no such job log"),
            ),
        },
        "/status.json" => match snapshot(runs) {
            Ok(value) => (
                "200 OK",
                "application/json",
                serde_json::to_string(&value).unwrap_or_default(),
            ),
            Err(error) => (
                "500 Internal Server Error",
                "application/json",
                json!({"error": error.to_string()}).to_string(),
            ),
        },
        _ => ("404 Not Found", "text/plain", String::from("not found")),
    }
}

/// Whether `id` can only name an entry of its directory: digits, letters
/// and dashes, never a path.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `/runs/<id>/report.html`: the run's HTML report from the store.
fn run_report(runs: &Path, rest: &str) -> Option<String> {
    let id = rest.strip_suffix("/report.html")?;
    plain_id(id)
        .then(|| std::fs::read_to_string(runs.join(id).join("report.html")).ok())
        .flatten()
}

/// Bytes of a job log the page shows: its end, where a failure is.
const JOB_LOG_TAIL: usize = 256 * 1024;

/// `/jobs/<id>.log`: the end of an enqueued job's log.
fn job_log(rest: &str) -> Option<String> {
    let id = rest.strip_suffix(".log")?;
    if !plain_id(id) {
        return None;
    }
    let log = crate::hil_jobs::Jobs::open().ok()?.read(id).ok()?.log?;
    let bytes = std::fs::read(log).ok()?;
    let start = bytes.len().saturating_sub(JOB_LOG_TAIL);
    Some(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

/// The stand and its newest runs.
fn snapshot(runs: &Path) -> Result<Value> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let status = arbiter.status()?;
    let jobs = crate::hil_jobs::Jobs::open()?.unfinished();
    let mut history = arbiter.history()?;
    history.reverse();
    history.truncate(RECENT_LEASES);
    Ok(json!({
        "generated_unix": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs(),
        "holders": status.holders,
        "queue": status.queue,
        "balances": status.balances,
        "devices": status.devices,
        "fixtures": *FIXTURES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        "maintenance": status.maintenance,
        "jobs": crate::hil_jobs::views(&jobs, &status),
        "leases": history,
        "runs": newest_runs(runs, RECENT_RUNS),
    }))
}

/// The `count` newest runs, newest first. Run directories start with their
/// start time, so only those are read.
fn newest_runs(runs: &Path, count: usize) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(runs) else {
        return Vec::new();
    };
    let mut names = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(|character: char| character.is_ascii_digit()))
        .collect::<Vec<_>>();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
        .into_iter()
        .filter_map(|name| hil_runs::load(&runs.join(name)))
        .take(count)
        .map(|run| {
            json!({
                "id": run.id,
                "started_millis": run.started_millis,
                "state": run.state,
                "outcome": run.outcome,
                "commit": run.commit.as_deref().map(|commit| &commit[..commit.len().min(12)]),
                "dirty": run.dirty,
                "checkout": run.checkout,
                "scenarios": run.scenarios.iter().map(|scenario| json!({
                    "id": scenario.id,
                    "outcome": scenario.outcome,
                })).collect::<Vec<_>>(),
                "progress": (run.state == hil_runs::State::Running)
                    .then(|| progress(&run.directory))
                    .flatten(),
                // A run still marked running whose runner is gone ended
                // without sealing its bundle.
                "abandoned": run.state == hil_runs::State::Running
                    && !hil_runs::runner_alive(&run.directory, run.started_millis),
                "report": run.directory.join("report.html").is_file(),
                "experiment_arm": std::fs::read(run.directory.join("manifest.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                    .and_then(|manifest| manifest["experiment"]["arm"].as_str().map(str::to_owned)),
            })
        })
        .collect()
}

/// Where a running run is: its latest step, the scenario it is in, and how
/// many of its planned scenarios finished. `None` when its events or plan
/// cannot be read or use a vocabulary this build does not know.
fn progress(run: &Path) -> Option<Value> {
    use oer_hil_evidence::run::{PlanDisposition, RunPlan};
    use oer_hil_schema::run::{RunEvent, RunEventKind};
    let events = std::fs::read_to_string(run.join("events.jsonl"))
        .ok()?
        .lines()
        .map(serde_json::from_str::<RunEvent>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .ok()?;
    let plan: RunPlan = serde_json::from_slice(&std::fs::read(run.join("plan.json")).ok()?).ok()?;
    let planned = plan
        .entries
        .iter()
        .filter(|entry| entry.disposition == PlanDisposition::Selected)
        .count();
    let finished = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                RunEventKind::ScenarioFinished | RunEventKind::ScenarioBlocked
            )
        })
        .count();
    let latest = events.last()?;
    let current = events
        .iter()
        .rev()
        .find(|event| event.kind == RunEventKind::ScenarioStarted)
        .filter(|started| {
            !events.iter().any(|event| {
                event.kind == RunEventKind::ScenarioFinished
                    && event.scenario == started.scenario
                    && event.timestamp_unix_millis >= started.timestamp_unix_millis
            })
        })
        .and_then(|started| started.scenario.clone());
    Some(json!({
        "step": latest.kind.id(),
        "step_since_millis": latest.timestamp_unix_millis,
        "scenario": current,
        "finished": finished,
        "planned": planned,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(runs: &Path, id: &str, outcome: &str) {
        let directory = runs.join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("manifest.json"),
            json!({"state": "completed", "outcome": outcome, "started_at_unix_ms": 1}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn a_report_link_reaches_only_a_report_inside_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let runs = directory.path().join("runs");
        run(&runs, "1790000000000-1f2e", "passed");
        std::fs::write(runs.join("1790000000000-1f2e/report.html"), "<p>report</p>").unwrap();
        std::fs::write(directory.path().join("report.html"), "outside").unwrap();
        assert_eq!(
            run_report(&runs, "1790000000000-1f2e/report.html").as_deref(),
            Some("<p>report</p>")
        );
        assert_eq!(run_report(&runs, "../report.html"), None);
        assert_eq!(run_report(&runs, "1790000000000-1f2e/manifest.json"), None);
        assert_eq!(job_log("../../etc/passwd.log"), None);
    }

    #[test]
    fn a_dashboard_record_names_a_live_process_of_one_build() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("dashboard.json");
        let me = Instance::current(8765).unwrap();
        me.write(&record).unwrap();
        let read = Instance::read(&record).unwrap();
        assert_eq!(read, me);
        assert!(read.alive());
        // A reused PID started at another time is not the dashboard.
        let recycled = Instance {
            started_unix_millis: me.started_unix_millis + 1_000,
            ..me.clone()
        };
        assert!(!recycled.alive());
        assert_eq!(Instance::current(9000).unwrap().build, me.build);
    }

    #[test]
    fn a_stopped_dashboard_withdraws_only_its_own_record() {
        let directory = tempfile::tempdir().unwrap();
        let record = directory.path().join("dashboard.json");
        let me = Instance::current(8765).unwrap();
        let other = Instance {
            pid: me.pid + 1,
            ..me.clone()
        };
        other.write(&record).unwrap();
        me.withdraw(&record);
        assert_eq!(Instance::read(&record), Some(other));
        me.write(&record).unwrap();
        me.withdraw(&record);
        assert!(!record.exists());
    }

    #[test]
    fn a_running_run_reports_its_step_scenario_and_finished_count() {
        let run = tempfile::tempdir().unwrap();
        let event = |millis: u64, kind: &str, scenario: Option<&str>| {
            json!({"timestamp_unix_millis": millis, "kind": kind, "scenario": scenario,
                   "image": null, "outcome": null})
            .to_string()
        };
        std::fs::write(
            run.path().join("events.jsonl"),
            [
                event(1, "run-started", None),
                event(2, "scenario-started", Some("a")),
                event(3, "scenario-finished", Some("a")),
                event(4, "scenario-started", Some("b")),
            ]
            .join("\n"),
        )
        .unwrap();
        let entry = |scenario: &str, disposition: &str| {
            json!({"scenario": scenario, "image": "correctness", "repetitions": 1,
                   "disposition": disposition, "reason": null})
        };
        std::fs::write(
            run.path().join("plan.json"),
            json!({"schema": 2, "run_id": "r", "selection": "s", "entries": [
                entry("a", "selected"), entry("b", "selected"), entry("c", "selected"),
                entry("d", "filtered"),
            ]})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            progress(run.path()).unwrap(),
            json!({"step": "scenario-started", "step_since_millis": 4, "scenario": "b",
                   "finished": 1, "planned": 3})
        );
        // An event this build does not know is not guessed at.
        let mut events = std::fs::read_to_string(run.path().join("events.jsonl")).unwrap();
        events.push_str(&format!("\n{}", event(5, "future-step", None)));
        std::fs::write(run.path().join("events.jsonl"), events).unwrap();
        assert_eq!(progress(run.path()), None);
    }

    #[test]
    fn the_page_and_its_newest_runs_are_served() {
        let store = tempfile::tempdir().unwrap();
        run(store.path(), "1000-a", "failed");
        run(store.path(), "2000-b", "passed");
        run(store.path(), "3000-c", "passed");
        std::fs::create_dir_all(store.path().join("runs.before-shared-store")).unwrap();
        let newest = newest_runs(store.path(), 2);
        assert_eq!(
            newest
                .iter()
                .map(|run| run["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["3000-c", "2000-b"]
        );
        let (status, content_type, body) = respond("/", store.path());
        assert_eq!(status, "200 OK");
        assert!(content_type.starts_with("text/html"));
        assert!(body.contains("status.json"));
        assert_eq!(respond("/missing", store.path()).0, "404 Not Found");
    }
}
