//! Finding, explaining, comparing and pruning the shared store's HIL runs.
//!
//! These commands read sealed bundles as loosely typed JSON, so bundles of
//! older runner versions stay readable; they never decide qualification,
//! which remains the independent evaluator's. Pins and the prune rule keep
//! what agents cite, replay and compare.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::Result;

/// One sealed or interrupted run, summarized.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    pub directory: PathBuf,
    pub started_millis: u64,
    pub state: String,
    pub outcome: Option<String>,
    pub commit: Option<String>,
    pub dirty: bool,
    /// Checkout the runner ran from, from its observer path.
    pub checkout: Option<String>,
    /// `(image class, application SHA-256)` of each flashed image.
    pub images: Vec<(String, String)>,
    /// Runs whose archived firmware this run replayed.
    pub replayed: Vec<String>,
    pub scenarios: Vec<ScenarioRun>,
}

#[derive(Clone, Debug)]
pub struct ScenarioRun {
    pub id: String,
    pub image: String,
    pub outcome: String,
    pub repetitions: Vec<Repetition>,
}

#[derive(Clone, Debug)]
pub struct Repetition {
    pub number: u64,
    pub outcome: String,
    pub failure: Option<(String, String)>,
    pub measurements: Vec<Measurement>,
    pub directory: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Measurement {
    pub name: String,
    pub value: Option<f64>,
    pub unit: String,
    pub threshold: Option<Value>,
    pub verdict: Option<String>,
}

fn text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
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
        .map(|scenario| ScenarioRun {
            id: text(&scenario["scenario"]).unwrap_or_default(),
            image: text(&scenario["image"]).unwrap_or_default(),
            outcome: text(&scenario["outcome"]).unwrap_or_default(),
            repetitions: scenario["repetitions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|repetition| Repetition {
                    number: repetition["repetition"].as_u64().unwrap_or_default(),
                    outcome: text(&repetition["outcome"]).unwrap_or_default(),
                    failure: repetition["failure"].as_object().map(|failure| {
                        (
                            failure.get("kind").and_then(text).unwrap_or_default(),
                            failure.get("message").and_then(text).unwrap_or_default(),
                        )
                    }),
                    measurements: repetition["measurements"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|measurement| Measurement {
                            name: text(&measurement["name"]).unwrap_or_default(),
                            value: measurement["value"].as_f64(),
                            unit: text(&measurement["unit"]).unwrap_or_default(),
                            threshold: Some(measurement["threshold"].clone())
                                .filter(|threshold| !threshold.is_null()),
                            verdict: text(&measurement["verdict"]),
                        })
                        .collect(),
                    directory: text(&repetition["artifact_directory"]),
                })
                .collect(),
        })
        .collect();
    Some(Run {
        id: text(&manifest["run_id"])
            .or_else(|| Some(directory.file_name()?.to_string_lossy().into_owned()))?,
        directory: directory.to_owned(),
        started_millis: started_millis(&manifest),
        state: text(&manifest["state"])
            .map(|state| {
                if state == "running" && !runner_alive(directory, started_millis(&manifest)) {
                    String::from("abandoned")
                } else {
                    state
                }
            })
            .unwrap_or_default(),
        outcome: suite.as_ref().and_then(|suite| text(&suite["outcome"])),
        commit: text(&manifest["repository"]["commit"]),
        dirty: manifest["repository"]["dirty"]
            .as_bool()
            .unwrap_or_default(),
        checkout,
        images,
        replayed,
        scenarios,
    })
}

fn started_millis(manifest: &Value) -> u64 {
    manifest["started_unix_millis"].as_u64().unwrap_or_default()
}

/// Whether the runner that started the run in `directory` still runs. The run
/// ID ends with the runner's PID in hexadecimal; a live process with that PID
/// that started after the run is another process reusing it.
fn runner_alive(directory: &Path, started_millis: u64) -> bool {
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

/// Selection of `runs list`.
#[derive(Debug, Default)]
pub struct Filter {
    pub scenario: Option<String>,
    pub outcome: Option<String>,
    pub commit: Option<String>,
    pub image: Option<String>,
    pub checkout: Option<String>,
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
                        .any(|s| &s.id == scenario && &s.outcome == wanted),
                    None => run.outcome.as_deref().unwrap_or(&run.state) == wanted,
                })
        };
        scenario(run)
            && outcome(run)
            && self.commit.as_ref().is_none_or(|prefix| {
                run.commit
                    .as_ref()
                    .is_some_and(|commit| commit.starts_with(prefix.as_str()))
            })
            && self.image.as_ref().is_none_or(|wanted| {
                run.images
                    .iter()
                    .any(|(class, sha)| class == wanted || sha.starts_with(wanted.as_str()))
            })
            && self
                .checkout
                .as_ref()
                .is_none_or(|wanted| run.checkout.as_deref() == Some(wanted.as_str()))
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
        run.outcome.as_deref().unwrap_or(&run.state),
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
        run.outcome.as_deref().unwrap_or(&run.state)
    );
    if run.scenarios.is_empty() {
        text.push_str(&format!(
            "  no suite result; the run ended in state `{}`; see {}\n",
            run.state,
            run.directory.join("events.jsonl").display()
        ));
    }
    for scenario in &run.scenarios {
        if scenario.outcome == "passed" {
            text.push_str(&format!("  {} [{}]: passed\n", scenario.id, scenario.image));
            continue;
        }
        text.push_str(&format!(
            "  {} [{}]: {}\n",
            scenario.id, scenario.image, scenario.outcome
        ));
        for repetition in &scenario.repetitions {
            if repetition.outcome == "passed" {
                continue;
            }
            let missed = repetition
                .measurements
                .iter()
                .filter(|m| {
                    m.verdict
                        .as_deref()
                        .is_some_and(|v| v != "passed" && v != "pass")
                })
                .collect::<Vec<_>>();
            let cause = match (&repetition.failure, missed.is_empty()) {
                (_, false) => "criterion: measured values missed their acceptance criteria",
                (Some((kind, _)), true) if kind == "infrastructure" => "infrastructure fault",
                (Some((kind, _)), true) if kind == "precondition" => "precondition not met",
                (Some((kind, _)), true) if kind.starts_with("image") => "image build or flash",
                (Some(_), true) => "scenario failure",
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
                    measurement.verdict.as_deref().unwrap_or("—")
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
        }
    }
    text
}

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
            let entry =
                sums.entry(measurement.name.clone())
                    .or_insert((0.0, 0, measurement.unit.clone()));
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
        a.outcome.as_deref().unwrap_or(&a.state),
        b.id,
        short(&b.commit, b.dirty),
        b.outcome.as_deref().unwrap_or(&b.state)
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
                .map(|s| s.outcome.clone())
                .unwrap_or_default()
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
        } else if run.state == "running" {
            keep.insert(run.id.clone(), String::from("in progress"));
        } else if run.outcome.is_none() && run.state != "abandoned" {
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
            if scenario.outcome == "passed" {
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
            "running"
        );
        // No process has PID 0xfffffffe.
        assert_eq!(running(&format!("{now}-fffffffe")), "abandoned");
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
                        {"name": "rx.mbps", "value": if failed { 10.0 } else { 20.0 }, "unit": "Mbit/s",
                         "threshold": {"min": 15.0}, "verdict": if failed { "failed" } else { "passed" }}
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
            Filter {
                commit: Some("0123".into()),
                ..Filter::default()
            }
            .matches(&runs[0])
        );
        assert!(
            !Filter {
                image: Some("performance".into()),
                ..Filter::default()
            }
            .matches(&runs[0])
        );
        let why = why(&runs[1], 2);
        assert!(why.contains("criterion"), "{why}");
        assert!(why.contains("missed rx.mbps: 10 Mbit/s"), "{why}");
        assert!(why.contains("| panic: x··"), "{why}");
        assert!(!why.contains("| boot"), "{why}");
        let compared = compare(&runs[0], &runs[1], Some("rx"));
        assert!(compared.contains("-50.0%"), "{compared}");
        let history = history(&runs, "s", Some("rx"));
        assert_eq!(history.lines().filter(|l| l.starts_with('r')).count(), 2);
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
}
