//! Finding, explaining, comparing and pruning the shared store's HIL runs.
//!
//! Bundles are read as JSON documents, but their vocabulary (run state,
//! outcomes, failure kinds, measurement units, thresholds and verdicts) is
//! parsed into the shared `oer_hil_schema::run` types: a value outside it
//! makes the bundle unreadable here instead of silently matching nothing.
//! These commands never decide qualification, which remains the independent
//! evaluator's. Pins and the prune rule keep
//! what agents cite, replay and compare.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use oer_hil_schema::run::{
    FailureKind, MeasurementUnit, MeasurementVerdict, Outcome, RunState, Threshold,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::Result;

/// A run's state as recorded, or `Abandoned` when it is recorded as running
/// but its runner is gone.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Running,
    Completed,
    Interrupted,
    Abandoned,
}

impl State {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Abandoned => "abandoned",
        }
    }

    /// A completed or interrupted bundle can no longer change.
    pub const fn is_sealed(self) -> bool {
        matches!(self, Self::Completed | Self::Interrupted)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.id())
    }
}

/// One sealed or interrupted run, summarized.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    pub directory: PathBuf,
    pub started_millis: u64,
    pub state: State,
    /// The suite outcome; `None` when the run has no suite result.
    pub outcome: Option<Outcome>,
    pub commit: Option<String>,
    pub dirty: bool,
    /// Checkout the runner ran from, from its observer path.
    pub checkout: Option<String>,
    /// `(image class, application SHA-256)` of each flashed image.
    pub images: Vec<(String, String)>,
    /// Runs whose archived firmware this run replayed.
    pub replayed: Vec<String>,
    pub scenarios: Vec<ScenarioRun>,
    /// Digest of the observer build the run names in the store, when it
    /// refers to one instead of embedding it.
    pub observer: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ScenarioRun {
    pub id: String,
    pub image: String,
    pub outcome: Outcome,
    /// Why the scenario as a whole did not run or finish, such as a failed
    /// image build that blocked it before any repetition.
    pub failure: Option<(FailureKind, String)>,
    pub repetitions: Vec<Repetition>,
}

#[derive(Clone, Debug)]
pub struct Repetition {
    pub number: u64,
    pub outcome: Outcome,
    pub failure: Option<(FailureKind, String)>,
    pub measurements: Vec<Measurement>,
    pub directory: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Measurement {
    pub name: String,
    pub value: Option<f64>,
    pub unit: MeasurementUnit,
    pub threshold: Option<Threshold>,
    pub verdict: Option<MeasurementVerdict>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// `value` as a schema type; `None` when it is absent or not of that type.
fn typed<T: DeserializeOwned>(value: &Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

/// The run's status for display and selection: its suite outcome, or its
/// state when it has none.
pub fn status(run: &Run) -> &'static str {
    run.outcome.map_or(run.state.id(), Outcome::id)
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Summarize the bundle in `directory`; `None` when it has no manifest.
pub fn load(directory: &Path) -> Option<Run> {
    let manifest = read(&directory.join("manifest.json"))?;
    let suite = read(&directory.join("suite.json"));
    let plan = read(&directory.join("plan.json"));
    let checkout = manifest["invocation"][0].as_str().and_then(|runner| {
        let path = Path::new(runner);
        path.ancestors()
            .find(|ancestor| ancestor.ends_with("target"))
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    });
    let images = manifest["firmware"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|firmware| {
            Some((
                text(&firmware["image"])?,
                text(&firmware["application_sha256"]).unwrap_or_default(),
            ))
        })
        .collect();
    let replayed = plan
        .as_ref()
        .map(|plan| collect_strings(&plan["firmware"], "source_run_id"))
        .unwrap_or_default();
    let scenarios = suite
        .as_ref()
        .and_then(|suite| suite["scenarios"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|scenario| {
            Some(ScenarioRun {
                id: text(&scenario["scenario"])?,
                image: text(&scenario["image"]).unwrap_or_default(),
                outcome: typed(&scenario["outcome"])?,
                failure: match &scenario["failure"] {
                    Value::Null => None,
                    failure => Some((
                        typed(&failure["kind"])?,
                        failure.get("message").and_then(text).unwrap_or_default(),
                    )),
                },
                repetitions: scenario["repetitions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|repetition| {
                        Some(Repetition {
                            number: repetition["repetition"].as_u64().unwrap_or_default(),
                            outcome: typed(&repetition["outcome"])?,
                            failure: match &repetition["failure"] {
                                Value::Null => None,
                                failure => Some((
                                    typed(&failure["kind"])?,
                                    failure.get("message").and_then(text).unwrap_or_default(),
                                )),
                            },
                            measurements: repetition["measurements"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|measurement| {
                                    Some(Measurement {
                                        name: text(&measurement["name"])?,
                                        value: measurement["value"].as_f64(),
                                        unit: typed(&measurement["unit"])?,
                                        threshold: match &measurement["threshold"] {
                                            Value::Null => None,
                                            threshold => Some(typed(threshold)?),
                                        },
                                        verdict: match &measurement["verdict"] {
                                            Value::Null => None,
                                            verdict => Some(typed(verdict)?),
                                        },
                                    })
                                })
                                .collect::<Option<_>>()?,
                            directory: text(&repetition["artifact_directory"]),
                        })
                    })
                    .collect::<Option<_>>()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let recorded: RunState = typed(&manifest["state"])?;
    let state = match recorded {
        RunState::Running if !runner_alive(directory, started_millis(&manifest)) => {
            State::Abandoned
        }
        RunState::Running => State::Running,
        RunState::Completed => State::Completed,
        RunState::Interrupted => State::Interrupted,
    };
    Some(Run {
        id: text(&manifest["run_id"])
            .or_else(|| Some(directory.file_name()?.to_string_lossy().into_owned()))?,
        directory: directory.to_owned(),
        started_millis: started_millis(&manifest),
        state,
        outcome: match suite.as_ref().map(|suite| &suite["outcome"]) {
            None | Some(Value::Null) => None,
            Some(outcome) => Some(typed(outcome)?),
        },
        commit: text(&manifest["repository"]["commit"]),
        dirty: manifest["repository"]["dirty"]
            .as_bool()
            .unwrap_or_default(),
        checkout,
        images,
        replayed,
        scenarios,
        observer: Some(&manifest["runner"]["observer"])
            .filter(|record| record["schema"] == oer_hil_schema::observer_store::REFERENCED)
            .and_then(|record| text(&record["build_sha256"])),
    })
}

fn started_millis(manifest: &Value) -> u64 {
    manifest["started_unix_millis"].as_u64().unwrap_or_default()
}

/// Whether the runner that started the run in `directory` still runs. The run
/// ID ends with the runner's PID in hexadecimal; a live process with that PID
/// that started after the run is another process reusing it.
pub(crate) fn runner_alive(directory: &Path, started_millis: u64) -> bool {
    let Some(pid) = directory
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('-').nth(1))
        .and_then(|pid| u32::from_str_radix(pid, 16).ok())
    else {
        // An unknown naming scheme is not evidence of a dead runner.
        return true;
    };
    oer_hil_arbiter::process_started_unix_millis(pid)
        // Boot time has a resolution of one second.
        .is_some_and(|process| process <= started_millis + 2000)
}

fn collect_strings(value: &Value, key: &str) -> Vec<String> {
    let mut found = Vec::new();
    match value {
        Value::Object(map) => {
            for (name, value) in map {
                if name == key
                    && let Some(text) = value.as_str()
                {
                    found.push(text.to_owned());
                }
                found.extend(collect_strings(value, key));
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|item| found.extend(collect_strings(item, key))),
        _ => {}
    }
    found
}

/// Every run below `runs`, oldest first.
/// The newest run of `checkout` in `runs`, reading run directories newest
/// first until one matches.
pub fn newest_of(runs: &Path, checkout: Option<&str>) -> Option<Run> {
    let mut names = fs::read_dir(runs)
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(|character: char| character.is_ascii_digit()))
        .collect::<Vec<_>>();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
        .into_iter()
        .filter_map(|name| load(&runs.join(name)))
        .find(|run| run.checkout.as_deref() == checkout)
}

/// Follow the run in `directory`, printing each step its `events.jsonl`
/// records, until it ends; the exit status for its outcome: 0 passed, 1
/// failed, broken, blocked or skipped, 2 interrupted, abandoned or on a
/// quarantined board.
pub fn wait(directory: &Path) -> Result<u8> {
    use oer_hil_schema::run::{RunEvent, RunEventKind};
    use std::io::{BufRead as _, Seek as _};
    let path = directory.join("events.jsonl");
    let mut offset = 0;
    let mut pending = String::new();
    loop {
        if let Ok(mut file) = fs::File::open(&path) {
            file.seek(std::io::SeekFrom::Start(offset))?;
            let mut reader = std::io::BufReader::new(file);
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                offset += line.len() as u64;
                pending.push_str(&line);
                line.clear();
                // A line is complete once its newline arrived.
                if !pending.ends_with('\n') {
                    continue;
                }
                let event: RunEvent = serde_json::from_str(pending.trim_end())
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                pending.clear();
                println!(
                    "{} {}{}{}",
                    event.timestamp_unix_millis / 1000,
                    event.kind.id(),
                    event
                        .scenario
                        .as_deref()
                        .map(|scenario| format!(" {scenario}"))
                        .unwrap_or_default(),
                    event
                        .outcome
                        .map(|outcome| format!(" {}", outcome.id()))
                        .unwrap_or_default()
                );
                if matches!(
                    event.kind,
                    RunEventKind::RunFinished | RunEventKind::RunInterrupted
                ) {
                    return Ok(exit_status(load(directory).and_then(|run| run.outcome)));
                }
            }
        }
        match load(directory) {
            // A runner that died leaves its run running forever.
            Some(run) if run.state == State::Abandoned => {
                println!("abandoned: the run's runner is gone");
                return Ok(2);
            }
            // A sealed run ends waiting even when its events stop short.
            Some(run) if run.state.is_sealed() => {
                println!(
                    "{}: {}",
                    run.state,
                    run.outcome.map_or("no outcome", Outcome::id)
                );
                return Ok(exit_status(run.outcome));
            }
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// The exit status [`wait`] reports for a run's `outcome`.
fn exit_status(outcome: Option<Outcome>) -> u8 {
    match outcome {
        Some(Outcome::Passed) => 0,
        Some(Outcome::Failed | Outcome::Broken | Outcome::Blocked | Outcome::Skipped) => 1,
        Some(Outcome::Interrupted | Outcome::BoardQuarantined) | None => 2,
    }
}

/// The runs of every store in `stores`, oldest first: each chip keeps its
/// runs in its own store.
pub fn all_in(stores: &[PathBuf]) -> Result<Vec<Run>> {
    let mut found = Vec::new();
    for store in stores {
        found.extend(all(store)?);
    }
    found.sort_by_key(|run| (run.started_millis, run.id.clone()));
    Ok(found)
}

/// The run `id` from whichever store in `stores` holds it.
pub fn find_in(stores: &[PathBuf], id: &str) -> Option<Run> {
    stores.iter().find_map(|store| load(&store.join(id)))
}

pub fn all(runs: &Path) -> Result<Vec<Run>> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(runs) else {
        return Ok(found);
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(run) = load(&entry.path())
        {
            found.push(run);
        }
    }
    found.sort_by_key(|run| (run.started_millis, run.id.clone()));
    Ok(found)
}

/// How long a stored observer build no run names yet is kept: a starting
/// run stores its build before its manifest names it.
const OBSERVER_GRACE: std::time::Duration = std::time::Duration::from_secs(3600);

/// Remove the observer builds beside `runs` that none of its runs names;
/// returns how many were removed.
pub fn collect_observers(runs: &Path) -> Result<usize> {
    let named = all(runs)?
        .into_iter()
        .filter_map(|run| run.observer)
        .collect();
    let canonical = fs::canonicalize(runs)?;
    let store = canonical.parent().ok_or("the run store has no parent")?;
    Ok(
        oer_hil_schema::observer_store::collect_garbage(store, &named, OBSERVER_GRACE)
            .map_err(|error| error.to_string())?
            .len(),
    )
}

/// Selection of `runs list`.
#[derive(Debug, Default)]
pub struct Filter {
    pub scenario: Option<String>,
    pub outcome: Option<String>,
    pub image: Option<String>,
    pub since_millis: Option<u64>,
}

impl Filter {
    pub fn matches(&self, run: &Run) -> bool {
        let scenario = |run: &Run| {
            self.scenario
                .as_ref()
                .is_none_or(|wanted| run.scenarios.iter().any(|s| &s.id == wanted))
        };
        let outcome = |run: &Run| {
            self.outcome
                .as_ref()
                .is_none_or(|wanted| match &self.scenario {
                    Some(scenario) => run
                        .scenarios
                        .iter()
                        .any(|s| &s.id == scenario && s.outcome.id() == wanted),
                    None => status(run) == wanted,
                })
        };
        scenario(run)
            && outcome(run)
            && self.image.as_ref().is_none_or(|wanted| {
                run.images
                    .iter()
                    .any(|(class, sha)| class == wanted || sha.starts_with(wanted.as_str()))
            })
            && self
                .since_millis
                .is_none_or(|since| run.started_millis >= since)
    }
}

fn date(millis: u64) -> String {
    let seconds = millis / 1000;
    let output = std::process::Command::new("date")
        .args(["-d", &format!("@{seconds}"), "+%Y-%m-%d %H:%M"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| seconds.to_string())
}

fn short(commit: &Option<String>, dirty: bool) -> String {
    let commit = commit
        .as_deref()
        .map_or("unknown", |commit| &commit[..commit.len().min(9)]);
    if dirty {
        format!("{commit}+")
    } else {
        commit.to_owned()
    }
}

/// One line per run.
pub fn list_line(run: &Run) -> String {
    let scenarios = run
        .scenarios
        .iter()
        .map(|scenario| format!("{}={}", scenario.id, scenario.outcome))
        .collect::<Vec<_>>()
        .join(" ");
    let images = run
        .images
        .iter()
        .map(|(class, sha)| format!("{class}:{}", &sha[..sha.len().min(8)]))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{} {} {:<10} {:<22} {:<10} [{}] {}",
        run.id,
        date(run.started_millis),
        status(run),
        run.checkout.as_deref().unwrap_or("?"),
        short(&run.commit, run.dirty),
        images,
        scenarios
    )
}

/// Why a run did not pass: per scenario and repetition, the failure, the
/// measurements that missed their criteria, cleanup failures, and where to
/// look next.
pub fn why(run: &Run, tail_lines: usize) -> String {
    let mut text = format!(
        "run {} ({}) from {} at {}: {}\n",
        run.id,
        date(run.started_millis),
        run.checkout.as_deref().unwrap_or("?"),
        short(&run.commit, run.dirty),
        status(run)
    );
    if run.scenarios.is_empty() {
        text.push_str(&format!(
            "  no suite result; the run ended in state `{}`; see {}\n",
            run.state,
            run.directory.join("events.jsonl").display()
        ));
    }
    for scenario in &run.scenarios {
        if scenario.outcome.is_passed() {
            text.push_str(&format!("  {} [{}]: passed\n", scenario.id, scenario.image));
            continue;
        }
        text.push_str(&format!(
            "  {} [{}]: {}\n",
            scenario.id, scenario.image, scenario.outcome
        ));
        if let Some((kind, message)) = &scenario.failure {
            text.push_str(&format!("    failure ({kind}):\n"));
            for line in message.lines() {
                text.push_str(&format!("      {}\n", printable(line)));
            }
        }
        for repetition in &scenario.repetitions {
            if repetition.outcome.is_passed() {
                continue;
            }
            let missed = repetition
                .measurements
                .iter()
                .filter(|m| m.verdict.is_some_and(|verdict| !verdict.is_passed()))
                .collect::<Vec<_>>();
            let cause = match (&repetition.failure, missed.is_empty()) {
                (_, false) => "criterion: measured values missed their acceptance criteria",
                (Some((FailureKind::Infrastructure, _)), true) => "infrastructure fault",
                (Some((FailureKind::Precondition, _)), true) => "precondition not met",
                (Some((FailureKind::ImageBuild | FailureKind::ImageFlash, _)), true) => {
                    "image build or flash"
                }
                (Some((FailureKind::Scenario, _)), true) => "scenario failure",
                (Some((FailureKind::Hang, _)), true) => "the target hung; its watchdog reset it",
                (Some((FailureKind::UnexpectedReset, _)), true) => {
                    "the target reset for a reason the runner did not cause"
                }
                (None, true) => "no recorded failure",
            };
            text.push_str(&format!(
                "    repetition {}: {} — {cause}\n",
                repetition.number, repetition.outcome
            ));
            if let Some((kind, message)) = &repetition.failure {
                text.push_str(&format!("      failure ({kind}): {message}\n"));
            }
            for measurement in missed {
                text.push_str(&format!(
                    "      missed {}: {} {} (criterion {}, verdict {})\n",
                    measurement.name,
                    measurement
                        .value
                        .map_or_else(|| String::from("—"), |value| value.to_string()),
                    measurement.unit,
                    measurement
                        .threshold
                        .as_ref()
                        .map_or_else(|| String::from("—"), ToString::to_string),
                    measurement
                        .verdict
                        .map_or("—", |verdict| if verdict.is_passed() {
                            "passed"
                        } else {
                            "failed"
                        })
                ));
            }
            let Some(directory) = &repetition.directory else {
                continue;
            };
            let directory = run.directory.join(directory);
            if let Some(cleanup) = read(&directory.join("cleanup.json")) {
                for failure in collect_strings(&cleanup, "failure") {
                    text.push_str(&format!("      cleanup failed: {failure}\n"));
                }
            }
            // Whether the host saw a board's USB bridge drop off the bus.
            match oer_hil_stand::usb_events::recorded(&directory) {
                Ok(events) => {
                    for event in events {
                        text.push_str(&format!(
                            "      host {event} at +{:.3} s of the run\n",
                            (event.realtime_micros / 1000).saturating_sub(run.started_millis)
                                as f64
                                / 1000.0
                        ));
                    }
                }
                Err(error) => {
                    text.push_str(&format!("      host USB events unreadable: {error}\n"));
                }
            }
            text.push_str(&format!("      artifacts: {}\n", directory.display()));
            let log = directory.join("uart.log");
            if tail_lines > 0
                && let Ok(content) = fs::read_to_string(&log)
            {
                let lines = content.lines().collect::<Vec<_>>();
                text.push_str(&format!(
                    "      last {} lines of uart.log:\n",
                    tail_lines.min(lines.len())
                ));
                for line in &lines[lines.len().saturating_sub(tail_lines)..] {
                    text.push_str(&format!("        | {}\n", printable(line)));
                }
            }
            // The target's own trace of what led there, when it had one.
            if let Ok(trace) = fs::read_to_string(directory.join("post-mortem/trace.txt")) {
                let mut lines = trace.lines();
                let heading = lines.next().unwrap_or_default().trim_start_matches("# ");
                let events = lines.collect::<Vec<_>>();
                let shown = TRACE_EVENTS_SHOWN.min(events.len());
                text.push_str(&format!("      trace ({heading}), last {shown} events:\n"));
                for line in &events[events.len() - shown..] {
                    text.push_str(&format!("        | {line}\n"));
                }
            }
        }
    }
    text
}

/// Where a run's artifacts live: the bundle directory in the shared store,
/// its reports, and per scenario and repetition the outcome and the
/// directory with the files it holds.
pub fn show(run: &Run) -> String {
    let directory = fs::canonicalize(&run.directory).unwrap_or_else(|_| run.directory.clone());
    let mut text = format!(
        "run {} ({}) from {} at {}: {}\n  directory: {}\n",
        run.id,
        date(run.started_millis),
        run.checkout.as_deref().unwrap_or("?"),
        short(&run.commit, run.dirty),
        status(run),
        directory.display()
    );
    for report in ["manifest.json", "suite.json", "events.jsonl", "report.html"] {
        if directory.join(report).exists() {
            text.push_str(&format!(
                "  {report}: {}\n",
                directory.join(report).display()
            ));
        }
    }
    for scenario in &run.scenarios {
        text.push_str(&format!(
            "  {} [{}]: {}\n",
            scenario.id, scenario.image, scenario.outcome
        ));
        for repetition in &scenario.repetitions {
            let Some(relative) = &repetition.directory else {
                text.push_str(&format!(
                    "    repetition {}: {} (no artifacts)\n",
                    repetition.number, repetition.outcome
                ));
                continue;
            };
            let path = directory.join(relative);
            text.push_str(&format!(
                "    repetition {}: {} {}\n",
                repetition.number,
                repetition.outcome,
                path.display()
            ));
            let mut entries = fs::read_dir(&path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_dir() {
                        name + "/"
                    } else {
                        name
                    }
                })
                .collect::<Vec<_>>();
            entries.sort();
            if !entries.is_empty() {
                text.push_str(&format!("      {}\n", entries.join("  ")));
            }
        }
    }
    text
}

/// Trace events `why` shows per failed repetition.
const TRACE_EVENTS_SHOWN: usize = 64;

/// A log line with control and replacement characters of raw binary output
/// shown as `·`.
fn printable(line: &str) -> String {
    line.chars()
        .map(|c| {
            if c == '\u{fffd}' || (c.is_control() && c != '\t') {
                '·'
            } else {
                c
            }
        })
        .collect()
}

/// Mean value of each numeric measurement of `scenario` over repetitions.
fn means(run: &Run, scenario: &str) -> BTreeMap<String, (f64, String)> {
    let mut sums: BTreeMap<String, (f64, usize, String)> = BTreeMap::new();
    for measurement in run
        .scenarios
        .iter()
        .filter(|s| s.id == scenario)
        .flat_map(|s| &s.repetitions)
        .flat_map(|r| &r.measurements)
    {
        if let Some(value) = measurement.value {
            let entry = sums.entry(measurement.name.clone()).or_insert((
                0.0,
                0,
                measurement.unit.to_string(),
            ));
            entry.0 += value;
            entry.1 += 1;
        }
    }
    sums.into_iter()
        .map(|(name, (sum, count, unit))| (name, (sum / count as f64, unit)))
        .collect()
}

/// Measurement means of two runs side by side, for their common scenarios.
pub fn compare(a: &Run, b: &Run, filter: Option<&str>) -> String {
    let mut text = format!(
        "A {} {} {}\nB {} {} {}\n",
        a.id,
        short(&a.commit, a.dirty),
        status(a),
        b.id,
        short(&b.commit, b.dirty),
        status(b)
    );
    let scenarios = a
        .scenarios
        .iter()
        .map(|s| s.id.clone())
        .filter(|id| b.scenarios.iter().any(|s| &s.id == id))
        .collect::<BTreeSet<_>>();
    if scenarios.is_empty() {
        text.push_str("no common scenario\n");
    }
    for scenario in scenarios {
        let outcome = |run: &Run| {
            run.scenarios
                .iter()
                .find(|s| s.id == scenario)
                .map_or("—", |s| s.outcome.id())
        };
        text.push_str(&format!(
            "{scenario}: A {} ({} repetitions), B {} ({} repetitions)\n",
            outcome(a),
            repetitions(a, &scenario),
            outcome(b),
            repetitions(b, &scenario)
        ));
        let (left, right) = (means(a, &scenario), means(b, &scenario));
        for name in left.keys().chain(right.keys()).collect::<BTreeSet<_>>() {
            if filter.is_some_and(|filter| !name.contains(filter)) {
                continue;
            }
            let value = |side: &BTreeMap<String, (f64, String)>| side.get(name).map(|(v, _)| *v);
            let unit = left.get(name).or(right.get(name)).map_or("", |(_, u)| u);
            let (x, y) = (value(&left), value(&right));
            let delta = match (x, y) {
                (Some(x), Some(y)) if x != 0.0 => format!("{:+.1}%", (y - x) / x.abs() * 100.0),
                (Some(x), Some(y)) => format!("{:+}", y - x),
                _ => String::from("—"),
            };
            let show = |v: Option<f64>| v.map_or_else(|| String::from("—"), |v| format!("{v:.3}"));
            text.push_str(&format!(
                "  {name:<60} {:>14} {:>14} {unit:<8} {delta}\n",
                show(x),
                show(y)
            ));
        }
    }
    text
}

fn repetitions(run: &Run, scenario: &str) -> usize {
    run.scenarios
        .iter()
        .find(|s| s.id == scenario)
        .map_or(0, |s| s.repetitions.len())
}

/// Outcome and measurement means of `scenario` across runs, oldest first.
pub fn history(runs: &[Run], scenario: &str, filter: Option<&str>) -> String {
    let mut text = String::new();
    let mut names = BTreeSet::new();
    let rows = runs
        .iter()
        .filter_map(|run| {
            let result = run.scenarios.iter().find(|s| s.id == scenario)?;
            let values = means(run, scenario)
                .into_iter()
                .filter(|(name, _)| filter.is_some_and(|filter| name.contains(filter)))
                .collect::<BTreeMap<_, _>>();
            names.extend(values.keys().cloned());
            Some((run, result, values))
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return format!("no run contains {scenario}\n");
    }
    for name in &names {
        text.push_str(&format!("  column {name}\n"));
    }
    for (run, result, values) in rows {
        let columns = names
            .iter()
            .map(|name| {
                values
                    .get(name)
                    .map_or_else(|| String::from("—"), |(value, _)| format!("{value:.3}"))
            })
            .collect::<Vec<_>>()
            .join("  ");
        let failure = result
            .repetitions
            .iter()
            .find_map(|r| r.failure.as_ref().map(|(_, message)| message.clone()))
            .map(|message| format!("  {}", message.chars().take(80).collect::<String>()))
            .unwrap_or_default();
        text.push_str(&format!(
            "{} {} {:<22} {:<10} {:<8} {columns}{failure}\n",
            run.id,
            date(run.started_millis),
            run.checkout.as_deref().unwrap_or("?"),
            short(&run.commit, run.dirty),
            result.outcome,
        ));
    }
    text
}

/// How stable `scenario` is over `runs`: its pass rate and how many of its
/// newest runs in a row did not pass.
pub fn stability(runs: &[Run], scenario: &str) -> String {
    let outcomes = runs
        .iter()
        .filter_map(|run| run.scenarios.iter().find(|s| s.id == scenario))
        .map(|result| result.outcome)
        .collect::<Vec<_>>();
    let passed = outcomes
        .iter()
        .filter(|outcome| outcome.is_passed())
        .count();
    let streak = outcomes
        .iter()
        .rev()
        .take_while(|outcome| !outcome.is_passed())
        .count();
    format!(
        "{scenario}: {passed} of {} runs passed ({:.0}%), {streak} newest in a row did not pass\n",
        outcomes.len(),
        100.0 * passed as f64 / outcomes.len().max(1) as f64
    )
}

/// Runs pinned by owners, with their reasons.
pub fn pins(store: &Path) -> BTreeMap<String, Value> {
    read(&store.join("pins.json"))
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn set_pin(store: &Path, run: &str, pin: Option<Value>) -> Result<()> {
    let mut pins = pins(store);
    match pin {
        Some(pin) => {
            pins.insert(run.to_owned(), pin);
        }
        None => {
            pins.remove(run);
        }
    }
    fs::create_dir_all(store)?;
    let temporary = store.join(format!("pins.json.{}", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(&pins)?)?;
    fs::rename(temporary, store.join("pins.json"))?;
    Ok(())
}

/// How one scenario's repetitions ended across runs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stability {
    pub scenario: String,
    pub passed: usize,
    /// Failures of the code under test.
    pub failed: usize,
    /// Failures of the stand: broken or blocked repetitions.
    pub stand: usize,
    /// The newest failure's message.
    pub last_failure: Option<String>,
}

impl Stability {
    pub fn repetitions(&self) -> usize {
        self.passed + self.failed + self.stand
    }

    /// Whether the scenario both passed and did not: its result depends on
    /// more than the code under test.
    pub fn is_flaky(&self) -> bool {
        self.passed > 0 && self.failed + self.stand > 0
    }
}

/// Each scenario's repetitions across `runs`, oldest first, that ended as
/// passed, failed, broken or blocked.
pub fn stabilities(runs: &[Run]) -> Vec<Stability> {
    let mut by_scenario: BTreeMap<String, Stability> = BTreeMap::new();
    for run in runs {
        for scenario in &run.scenarios {
            let entry = by_scenario
                .entry(scenario.id.clone())
                .or_insert_with(|| Stability {
                    scenario: scenario.id.clone(),
                    ..Stability::default()
                });
            for repetition in &scenario.repetitions {
                match repetition.outcome {
                    Outcome::Passed => entry.passed += 1,
                    Outcome::Failed => entry.failed += 1,
                    Outcome::Broken | Outcome::Blocked => entry.stand += 1,
                    Outcome::Skipped | Outcome::Interrupted | Outcome::BoardQuarantined => {
                        continue;
                    }
                }
                if let Some((_, message)) = &repetition.failure {
                    entry.last_failure = Some(message.clone());
                }
            }
        }
    }
    by_scenario.into_values().collect()
}

/// The scenarios of `stabilities` with at least `minimum` repetitions that
/// did not always pass, least passing first, with `quarantined` marked.
pub fn flaky_report(
    stabilities: &[Stability],
    minimum: usize,
    quarantined: &BTreeMap<String, Value>,
) -> String {
    let mut listed = stabilities
        .iter()
        .filter(|entry| entry.repetitions() >= minimum && entry.failed + entry.stand > 0)
        .collect::<Vec<_>>();
    listed.sort_by(|a, b| {
        (a.passed * b.repetitions())
            .cmp(&(b.passed * a.repetitions()))
            .then_with(|| a.scenario.cmp(&b.scenario))
    });
    let mut report = String::new();
    for entry in listed {
        let rate = 100 * entry.passed / entry.repetitions().max(1);
        report.push_str(&format!(
            "{:>3}% {} ({} of {} passed; {} failed, {} stand){}{}\n",
            rate,
            entry.scenario,
            entry.passed,
            entry.repetitions(),
            entry.failed,
            entry.stand,
            if entry.is_flaky() { ", flaky" } else { "" },
            if quarantined.contains_key(&entry.scenario) {
                ", quarantined"
            } else {
                ""
            },
        ));
        if let Some(message) = &entry.last_failure {
            let message = message.lines().next().unwrap_or_default();
            report.push_str(&format!(
                "      last: {}\n",
                message.chars().take(120).collect::<String>()
            ));
        }
    }
    if report.is_empty() {
        report.push_str("every scenario with enough repetitions passed each time\n");
    }
    report
}

/// Scenarios quarantined by owners, with their reasons: a run of every
/// scenario leaves them out.
pub fn quarantined(store: &Path) -> BTreeMap<String, Value> {
    read(&store.join("quarantine.json"))
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn set_quarantine(store: &Path, scenario: &str, entry: Option<Value>) -> Result<()> {
    let mut quarantined = quarantined(store);
    match entry {
        Some(entry) => {
            quarantined.insert(scenario.to_owned(), entry);
        }
        None => {
            quarantined.remove(scenario);
        }
    }
    fs::create_dir_all(store)?;
    let temporary = store.join(format!("quarantine.json.{}", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(&quarantined)?)?;
    fs::rename(temporary, store.join("quarantine.json"))?;
    Ok(())
}

/// The prune rule.
#[derive(Debug)]
pub struct Retention {
    /// Every run younger than this is kept.
    pub keep_days: u64,
    /// The newest failed runs kept per scenario and image class.
    pub keep_failed: usize,
}

/// Why each run is kept; runs without a reason may be deleted.
pub fn retained(
    runs: &[Run],
    rule: &Retention,
    now_millis: u64,
    pinned: &BTreeSet<String>,
    cited: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut keep = BTreeMap::new();
    let recent = now_millis.saturating_sub(rule.keep_days * 24 * 3600 * 1000);
    for run in runs {
        if pinned.contains(&run.id) {
            keep.insert(run.id.clone(), String::from("pinned"));
        } else if cited.contains(&run.id) {
            keep.insert(run.id.clone(), String::from("cited by a committed shard"));
        } else if run.started_millis >= recent {
            keep.insert(
                run.id.clone(),
                format!("younger than {} days", rule.keep_days),
            );
        } else if run.state == State::Running {
            keep.insert(run.id.clone(), String::from("in progress"));
        } else if run.outcome.is_none() && run.state != State::Abandoned {
            keep.insert(run.id.clone(), String::from("incomplete"));
        }
        for source in &run.replayed {
            keep.entry(source.clone())
                .or_insert_with(|| format!("firmware replayed by {}", run.id));
        }
    }
    let mut passed = BTreeMap::new();
    let mut failed: BTreeMap<(String, String), Vec<&Run>> = BTreeMap::new();
    for run in runs.iter().rev() {
        for scenario in &run.scenarios {
            let key = (scenario.id.clone(), scenario.image.clone());
            if scenario.outcome.is_passed() {
                passed.entry(key).or_insert(run.id.clone());
            } else {
                failed.entry(key).or_default().push(run);
            }
        }
    }
    for ((scenario, image), run) in passed {
        keep.entry(run)
            .or_insert_with(|| format!("latest pass of {scenario} [{image}]"));
    }
    for ((scenario, image), runs) in failed {
        for run in runs.into_iter().take(rule.keep_failed) {
            keep.entry(run.id.clone())
                .or_insert_with(|| format!("recent failure of {scenario} [{image}]"));
        }
    }
    keep
}

/// The runs to delete, oldest first, so that a store of these `runs`
/// (oldest first) with these exclusive `sizes` fits `budget` bytes. Only a
/// run that no rule but its age keeps is deleted: `kept` holds the runs the
/// other rules keep.
pub fn over_budget<'a>(
    runs: &'a [Run],
    kept: &BTreeMap<String, String>,
    sizes: &BTreeMap<String, u64>,
    budget: u64,
) -> Vec<&'a Run> {
    let size = |run: &Run| sizes.get(&run.id).copied().unwrap_or(0);
    let mut total: u64 = runs.iter().map(size).sum();
    let mut deleted = Vec::new();
    for run in runs {
        if total <= budget {
            break;
        }
        if kept.contains_key(&run.id) {
            continue;
        }
        total = total.saturating_sub(size(run));
        deleted.push(run);
    }
    deleted
}

/// Bytes only this directory holds: files without other hard links.
pub fn exclusive_bytes(directory: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    let mut total = 0;
    let mut pending = vec![directory.to_owned()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.nlink() == 1 {
                total += metadata.len();
            }
        }
    }
    total
}

/// Run IDs the committed HIL evidence shards cite.
pub fn cited_by_shards(root: &Path) -> BTreeSet<String> {
    let mut cited = BTreeSet::new();
    let Ok(targets) = fs::read_dir(root.join("hil/evidence")) else {
        return cited;
    };
    for target in targets.flatten() {
        let Ok(shards) = fs::read_dir(target.path()) else {
            continue;
        };
        for shard in shards.flatten() {
            if let Some(value) = read(&shard.path()) {
                cited.extend(collect_strings(&value, "run-id"));
                cited.extend(collect_strings(&value, "run_id"));
            }
        }
    }
    cited
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scenario_that_passed_and_failed_is_flaky_and_its_stand_failures_count_apart() {
        let repetition = |outcome, failure: Option<&str>| Repetition {
            number: 1,
            outcome,
            failure: failure.map(|message| (FailureKind::Infrastructure, message.to_owned())),
            measurements: Vec::new(),
            directory: None,
        };
        let run = |repetitions: Vec<Repetition>| Run {
            id: String::from("1-a"),
            directory: PathBuf::new(),
            started_millis: 1,
            state: State::Completed,
            outcome: Some(Outcome::Failed),
            commit: None,
            dirty: false,
            checkout: None,
            images: Vec::new(),
            replayed: Vec::new(),
            scenarios: vec![ScenarioRun {
                id: String::from("system-watchdog"),
                image: String::from("system-watchdog"),
                outcome: Outcome::Failed,
                failure: None,
                repetitions,
            }],
            observer: None,
        };
        let runs = [
            run(vec![
                repetition(Outcome::Passed, None),
                repetition(
                    Outcome::Blocked,
                    Some("device did not publish a HIL protocol hello"),
                ),
            ]),
            run(vec![
                repetition(Outcome::Passed, None),
                repetition(Outcome::Failed, Some("unexpected reset")),
                repetition(Outcome::Interrupted, None),
            ]),
        ];
        let stabilities = stabilities(&runs);
        assert_eq!(stabilities.len(), 1);
        let watchdog = &stabilities[0];
        assert_eq!(
            (watchdog.passed, watchdog.failed, watchdog.stand),
            (2, 1, 1)
        );
        assert!(watchdog.is_flaky());
        assert_eq!(watchdog.last_failure.as_deref(), Some("unexpected reset"));
        let quarantined = BTreeMap::from([(
            String::from("system-watchdog"),
            serde_json::json!({"reason": "lost hello"}),
        )]);
        let report = flaky_report(&stabilities, 3, &quarantined);
        assert!(report.starts_with(
            " 50% system-watchdog (2 of 4 passed; 1 failed, 1 stand), flaky, quarantined"
        ));
        // Too few repetitions to judge.
        assert!(flaky_report(&stabilities, 5, &quarantined).contains("passed each time"));
    }

    #[test]
    fn over_its_budget_a_store_loses_its_oldest_runs_only_age_kept() {
        let store = tempfile::tempdir().unwrap();
        for (id, started) in [("1-a", 1), ("2-b", 2), ("3-c", 3), ("4-d", 4)] {
            let directory = store.path().join(id);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("manifest.json"),
                serde_json::json!({"state": "completed", "started_unix_millis": started})
                    .to_string(),
            )
            .unwrap();
        }
        let runs = all(store.path()).unwrap();
        let sizes = [("1-a", 10), ("2-b", 20), ("3-c", 30), ("4-d", 40)]
            .map(|(id, size)| (String::from(id), size))
            .into();
        let kept = BTreeMap::from([(String::from("2-b"), String::from("pinned"))]);
        let deleted = over_budget(&runs, &kept, &sizes, 65)
            .into_iter()
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>();
        // 100 bytes: the oldest goes (90), the pinned one stays, the next
        // goes (60) and the store fits.
        assert_eq!(deleted, ["1-a", "3-c"]);
        assert!(over_budget(&runs, &kept, &sizes, 100).is_empty());
    }

    #[test]
    fn every_chips_store_answers_for_its_own_runs() {
        let esp32s31 = tempfile::tempdir().unwrap();
        let esp32c5 = tempfile::tempdir().unwrap();
        let sealed = |store: &Path, id: &str, started: u64| {
            let directory = store.join(id);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join("manifest.json"),
                serde_json::json!({"state": "completed", "started_unix_millis": started})
                    .to_string(),
            )
            .unwrap();
        };
        sealed(esp32s31.path(), "20-00000001", 20);
        sealed(esp32c5.path(), "10-00000002", 10);
        let stores = [esp32s31.path().to_owned(), esp32c5.path().to_owned()];
        let found = find_in(&stores, "10-00000002").unwrap();
        assert!(found.directory.starts_with(esp32c5.path()));
        assert!(find_in(&stores, "30-00000003").is_none());
        let ids = all_in(&stores)
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["10-00000002", "20-00000001"]);
    }

    #[test]
    fn waiting_ends_at_the_run_end_with_its_outcome() {
        let run = tempfile::tempdir().unwrap();
        let event = |kind: &str, outcome: Option<&str>| {
            serde_json::json!({"timestamp_unix_millis": 1, "kind": kind, "scenario": null,
                               "image": null, "outcome": outcome})
            .to_string()
        };
        fs::write(
            run.path().join("events.jsonl"),
            format!(
                "{}\n{}\n",
                event("run-started", None),
                event("run-finished", Some("failed"))
            ),
        )
        .unwrap();
        // Without a readable suite the outcome is unknown: not a pass.
        assert_eq!(wait(run.path()).unwrap(), 2);
        // A sealed run whose events stop short still ends the wait.
        let sealed = tempfile::tempdir().unwrap();
        fs::write(
            sealed.path().join("events.jsonl"),
            format!("{}\n", event("run-started", None)),
        )
        .unwrap();
        fs::write(
            sealed.path().join("manifest.json"),
            serde_json::json!({"state": "interrupted", "run_id": "1-a", "started_unix_millis": 1})
                .to_string(),
        )
        .unwrap();
        assert_eq!(wait(sealed.path()).unwrap(), 2);
        assert_eq!(exit_status(Some(Outcome::Passed)), 0);
        assert_eq!(exit_status(Some(Outcome::Broken)), 1);
        assert_eq!(exit_status(Some(Outcome::BoardQuarantined)), 2);
    }

    #[test]
    fn observer_builds_no_run_names_are_collected_after_a_grace() {
        use oer_hil_schema::observer_store;
        use serde_json::json;
        let store = tempfile::tempdir().unwrap();
        let runs = store.path().join("runs");
        let named = observer_store::store(store.path(), &json!({"named": true})).unwrap();
        let orphan = observer_store::store(store.path(), &json!({"named": false})).unwrap();
        let directory = runs.join("1000-a");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("manifest.json"),
            json!({"state": "completed", "started_at_unix_ms": 1,
                "runner": {"observer": {"schema": 2, "build_sha256": named}}})
            .to_string(),
        )
        .unwrap();
        assert_eq!(collect_observers(&runs).unwrap(), 0, "stored just now");
        let old = std::time::SystemTime::now() - 2 * OBSERVER_GRACE;
        for sha256 in [&named, &orphan] {
            fs::File::options()
                .append(true)
                .open(observer_store::path(store.path(), sha256).unwrap())
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        assert_eq!(collect_observers(&runs).unwrap(), 1);
        assert!(observer_store::load(store.path(), &named).is_ok());
        assert!(observer_store::load(store.path(), &orphan).is_err());
    }

    #[test]
    fn a_running_run_without_its_runner_is_abandoned() {
        let runs = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let running = |id: &str| {
            let directory = runs.path().join(id);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("manifest.json"),
                serde_json::json!({"state": "running", "started_unix_millis": now}).to_string(),
            )
            .unwrap();
            load(&directory).unwrap().state
        };
        // This test process started before the run.
        assert_eq!(
            running(&format!("{now}-{:08x}", std::process::id())),
            State::Running
        );
        // No process has PID 0xfffffffe.
        assert_eq!(running(&format!("{now}-fffffffe")), State::Abandoned);
    }

    fn bundle(runs: &Path, id: &str, started: u64, outcome: &str, replay: Option<&str>) {
        let directory = runs.join(id);
        let repetition = directory.join("scenarios/s/repetition-001");
        fs::create_dir_all(&repetition).unwrap();
        fs::write(
            directory.join("manifest.json"),
            serde_json::json!({
                "run_id": id, "state": "completed", "started_unix_millis": started,
                "invocation": ["/home/u/dev/checkout-a/target/hil/observers/x/runner", "run", "s"],
                "repository": {"commit": "0123456789abcdef", "dirty": false},
                "firmware": [{"image": "correctness", "application_sha256": "aabbccddeeff"}]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            directory.join("plan.json"),
            serde_json::json!({"firmware": replay.map(|run| serde_json::json!({"replay": {"source_run_id": run}}))})
                .to_string(),
        )
        .unwrap();
        let failed = outcome != "passed";
        fs::write(
            directory.join("suite.json"),
            serde_json::json!({"outcome": outcome, "scenarios": [{
                "scenario": "s", "image": "correctness", "outcome": outcome,
                "repetitions": [{
                    "repetition": 1, "outcome": outcome,
                    "artifact_directory": "scenarios/s/repetition-001",
                    "failure": failed.then(|| serde_json::json!({"kind": "scenario", "message": "burst missing"})),
                    "measurements": [
                        {"name": "rx.mbps", "value": if failed { 10_000_000 } else { 20_000_000 },
                         "unit": "bits-per-second",
                         "threshold": {"comparison": "at-least", "value": 15_000_000},
                         "verdict": if failed { "failed" } else { "passed" }}
                    ]
                }]
            }]})
            .to_string(),
        )
        .unwrap();
        fs::write(
            repetition.join("uart.log"),
            "boot\nready\npanic: x\u{1}\u{fffd}\n",
        )
        .unwrap();
        if failed {
            fs::write(
                repetition.join("usb-events.json"),
                serde_json::json!([{
                    "realtime_micros": (started + 12_345) * 1000, "device": "3-8",
                    "event": "disconnected", "device_number": 19
                }])
                .to_string(),
            )
            .unwrap();
            fs::create_dir_all(repetition.join("post-mortem")).unwrap();
            let events = (0..70)
                .map(|index| format!("{index:>12} us  event-{index}\n"))
                .collect::<String>();
            fs::write(
                repetition.join("post-mortem/trace.txt"),
                format!("# 70 entries; of the current boot\n{events}"),
            )
            .unwrap();
        }
    }

    #[test]
    fn runs_are_found_explained_and_compared() {
        let store = tempfile::tempdir().unwrap();
        bundle(store.path(), "r1", 1000, "passed", None);
        bundle(store.path(), "r2", 2000, "failed", None);
        let runs = all(store.path()).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].checkout.as_deref(), Some("checkout-a"));
        let filter = Filter {
            scenario: Some("s".into()),
            outcome: Some("failed".into()),
            ..Filter::default()
        };
        assert_eq!(runs.iter().filter(|run| filter.matches(run)).count(), 1);
        assert!(
            !Filter {
                image: Some("performance".into()),
                ..Filter::default()
            }
            .matches(&runs[0])
        );
        let why = why(&runs[1], 2);
        assert!(why.contains("criterion"), "{why}");
        assert!(
            why.contains("missed rx.mbps: 10000000 bit/s (criterion >= 15000000"),
            "{why}"
        );
        assert!(why.contains("| panic: x··"), "{why}");
        assert!(
            why.contains("host usb 3-8 disconnected (device number 19) at +12.345 s of the run"),
            "{why}"
        );
        assert!(
            why.contains("trace (70 entries; of the current boot), last 64 events"),
            "{why}"
        );
        assert!(
            why.contains("event-69") && why.contains("event-6\n"),
            "{why}"
        );
        assert!(!why.contains("event-5\n"), "{why}");
        assert!(!why.contains("| boot"), "{why}");
        let shown = show(&runs[1]);
        let directory = fs::canonicalize(store.path().join("r2")).unwrap();
        assert!(
            shown.contains(&format!("  directory: {}\n", directory.display())),
            "{shown}"
        );
        assert!(shown.contains("  s [correctness]: failed\n"), "{shown}");
        assert!(
            shown.contains(&format!(
                "    repetition 1: failed {}\n",
                directory.join("scenarios/s/repetition-001").display()
            )),
            "{shown}"
        );
        assert!(
            shown.contains("post-mortem/") && shown.contains("uart.log"),
            "{shown}"
        );
        let compared = compare(&runs[0], &runs[1], Some("rx"));
        assert!(compared.contains("-50.0%"), "{compared}");
        let history = history(&runs, "s", Some("rx"));
        assert_eq!(history.lines().filter(|l| l.starts_with('r')).count(), 2);
        assert_eq!(
            stability(&runs, "s"),
            "s: 1 of 2 runs passed (50%), 1 newest in a row did not pass\n"
        );
    }

    #[test]
    fn pruning_keeps_what_is_cited_replayed_recent_and_latest() {
        let store = tempfile::tempdir().unwrap();
        let day = 24 * 3600 * 1000;
        bundle(store.path(), "old-pass", day, "passed", None);
        bundle(store.path(), "new-pass", 2 * day, "passed", None);
        for index in 0..4 {
            bundle(
                store.path(),
                &format!("fail-{index}"),
                3 * day + index,
                "failed",
                None,
            );
        }
        bundle(store.path(), "baseline", 4 * day, "failed", None);
        bundle(
            store.path(),
            "replayer",
            5 * day,
            "passed",
            Some("old-pass"),
        );
        bundle(store.path(), "recent", 100 * day, "failed", None);
        let runs = all(store.path()).unwrap();
        let keep = retained(
            &runs,
            &Retention {
                keep_days: 30,
                keep_failed: 2,
            },
            100 * day,
            &BTreeSet::from([String::from("baseline")]),
            &BTreeSet::from([String::from("fail-0")]),
        );
        assert_eq!(keep["baseline"], "pinned");
        assert_eq!(keep["fail-0"], "cited by a committed shard");
        assert!(keep["old-pass"].starts_with("firmware replayed"));
        assert!(keep["replayer"].starts_with("latest pass"));
        assert!(keep["recent"].starts_with("younger"));
        let deleted = runs
            .iter()
            .filter(|run| !keep.contains_key(&run.id))
            .map(|run| run.id.as_str())
            .collect::<Vec<_>>();
        // The two newest failures are `recent` and `baseline`.
        assert_eq!(deleted, ["new-pass", "fail-1", "fail-2", "fail-3"]);
    }

    #[test]
    fn why_shows_the_failure_that_blocked_a_scenario_before_any_repetition() {
        let run = Run {
            id: "r".into(),
            directory: PathBuf::from("/nonexistent"),
            started_millis: 0,
            state: State::Completed,
            outcome: Some(Outcome::Blocked),
            commit: None,
            dirty: false,
            checkout: None,
            images: Vec::new(),
            replayed: Vec::new(),
            scenarios: vec![ScenarioRun {
                id: "coex".into(),
                image: "wifi-ble-coex".into(),
                outcome: Outcome::Blocked,
                failure: Some((
                    FailureKind::ImageBuild,
                    "stack audit failed\nframe grew to 9248 bytes".into(),
                )),
                repetitions: Vec::new(),
            }],
            observer: None,
        };
        let text = why(&run, 0);
        assert!(text.contains("failure (image-build):"), "{text}");
        assert!(text.contains("      frame grew to 9248 bytes"), "{text}");
    }
}
