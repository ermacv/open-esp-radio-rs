//! `cargo hil dashboard`: a live page of the stand on this host.
//!
//! A single-threaded HTTP server bound to the loopback interface serves one
//! page and `status.json`: the arbiter's holders, queue, boards and recent
//! leases, plus the newest runs of the shared run store. The page polls the
//! JSON, so it reflects the stand without reloading. Nothing is written; the
//! server only reads the arbiter's state and the run manifests.
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

pub fn serve(runs: &Path, args: &[std::ffi::OsString]) -> Result<std::process::ExitCode> {
    let port = match args {
        [] => DEFAULT_PORT,
        [flag, port] if flag == "--port" => port
            .to_str()
            .and_then(|port| port.parse().ok())
            .ok_or("--port takes a port number")?,
        _ => return Err("usage: cargo hil dashboard [--port PORT]".into()),
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|error| format!("cannot listen on 127.0.0.1:{port}: {error}"))?;
    eprintln!(
        "hil dashboard: http://{} (Ctrl+C stops it)",
        listener.local_addr()?
    );
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // A client that disconnects mid-request only loses its response.
        let _ = handle(stream, runs);
    }
    Ok(std::process::ExitCode::SUCCESS)
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

/// The stand and its newest runs.
fn snapshot(runs: &Path) -> Result<Value> {
    let arbiter = oer_hil_arbiter::Arbiter::open()?;
    let status = arbiter.status()?;
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
        "maintenance": status.maintenance,
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
            })
        })
        .collect()
}

/// Where a running run is: its latest step, the scenario it is in, and how
/// many of its planned scenarios finished. `None` when its events or plan
/// cannot be read or use a vocabulary this build does not know.
fn progress(run: &Path) -> Option<Value> {
    use oer_hil_runner_core::evidence::run::{PlanDisposition, RunPlan};
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
