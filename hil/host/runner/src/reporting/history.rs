//! Rebuildable history and stability views derived from immutable run bundles.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::run::{
    Measurement, MeasurementUnit, MeasurementVerdict, Outcome, RUN_SCHEMA, RunManifest, RunState,
    SuiteCounts, SuiteResult, Threshold, aggregate_outcome, atomic_json, atomic_write,
};
use crate::Result;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HistoryCounts {
    pub(crate) runs: usize,
    pub(crate) running: usize,
    pub(crate) completed: usize,
    pub(crate) interrupted: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RunHistoryEntry {
    pub(crate) run_id: String,
    pub(crate) state: RunState,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) started_unix_millis: u64,
    pub(crate) finished_unix_millis: Option<u64>,
    pub(crate) duration_millis: Option<u64>,
    pub(crate) commit: String,
    pub(crate) dirty: bool,
    pub(crate) workspace_sha256: String,
    #[serde(default)]
    pub(crate) replayed_from_runs: Vec<String>,
    pub(crate) cell_id: String,
    pub(crate) device_id: String,
    pub(crate) suite_counts: Option<SuiteCounts>,
    pub(crate) run_directory: PathBuf,
    pub(crate) suite_report: Option<PathBuf>,
    pub(crate) html_report: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ScenarioTrend {
    pub(crate) scenario: String,
    pub(crate) observations: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) broken: usize,
    pub(crate) skipped: usize,
    pub(crate) blocked: usize,
    pub(crate) interrupted: usize,
    pub(crate) pass_rate_basis_points: u16,
    pub(crate) flaky: bool,
    pub(crate) consecutive_non_passed: usize,
    pub(crate) last_outcome: Outcome,
    pub(crate) last_run_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MeasurementTrend {
    pub(crate) scenario: String,
    pub(crate) measurement: String,
    pub(crate) unit: MeasurementUnit,
    pub(crate) threshold: Option<Threshold>,
    pub(crate) observations: usize,
    pub(crate) minimum: u64,
    pub(crate) maximum: u64,
    pub(crate) latest: u64,
    pub(crate) failed_verdicts: usize,
    pub(crate) last_run_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HistoryReport {
    pub(crate) schema: u16,
    pub(crate) target: String,
    pub(crate) source_watermark_unix_millis: u64,
    pub(crate) counts: HistoryCounts,
    pub(crate) runs: Vec<RunHistoryEntry>,
    pub(crate) scenarios: Vec<ScenarioTrend>,
    pub(crate) measurements: Vec<MeasurementTrend>,
}

#[derive(Debug, Serialize)]
pub(crate) struct HistoryCompletion {
    pub(crate) schema: u16,
    pub(crate) target: String,
    pub(crate) runs: usize,
    pub(crate) scenarios: usize,
    pub(crate) measurements: usize,
    pub(crate) history_report: PathBuf,
    pub(crate) html_report: PathBuf,
}

#[derive(Clone, Debug)]
struct ScenarioObservation {
    started_unix_millis: u64,
    run_id: String,
    scenario: String,
    outcome: Outcome,
}

#[derive(Clone, Debug)]
struct MeasurementObservation {
    started_unix_millis: u64,
    run_id: String,
    scenario: String,
    repetition: u8,
    measurement: Measurement,
}

#[derive(Default)]
struct TrendAccumulator {
    observations: usize,
    passed: usize,
    failed: usize,
    broken: usize,
    skipped: usize,
    blocked: usize,
    interrupted: usize,
    consecutive_non_passed: usize,
    last_outcome: Option<Outcome>,
    last_run_id: String,
}

struct MeasurementAccumulator {
    observations: usize,
    minimum: u64,
    maximum: u64,
    latest: u64,
    failed_verdicts: usize,
    last_run_id: String,
}

pub(crate) fn rebuild(root: &Path, target: &str) -> Result<HistoryCompletion> {
    rebuild_at(&root.join("target/hil").join(target), target)
}

pub(super) fn rebuild_at(target_directory: &Path, target: &str) -> Result<HistoryCompletion> {
    let runs_directory = target_directory.join("runs");
    fs::create_dir_all(&runs_directory)?;
    let mut entries = fs::read_dir(&runs_directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let mut runs = Vec::new();
    let mut observations = Vec::new();
    let mut measurement_observations = Vec::new();
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            return Err(format!(
                "HIL runs directory contains a non-directory entry: {}",
                entry.path().display()
            )
            .into());
        }
        let run_directory = entry.path();
        let manifest_path = run_directory.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(format!(
                "HIL run bundle has no manifest: {}",
                run_directory.display()
            )
            .into());
        }
        let manifest: RunManifest = read_json(&manifest_path)?;
        validate_manifest(&manifest, target, &run_directory)?;
        let relative_directory = PathBuf::from("runs").join(&manifest.run_id);
        let suite = if manifest.state == RunState::Completed {
            let suite_path = run_directory.join("suite.json");
            if !suite_path.is_file() {
                return Err(format!(
                    "completed HIL run has no suite report: {}",
                    run_directory.display()
                )
                .into());
            }
            let suite: SuiteResult = read_json(&suite_path)?;
            validate_suite(&suite, &manifest)?;
            for scenario in &suite.scenarios {
                observations.push(ScenarioObservation {
                    started_unix_millis: suite.started_unix_millis,
                    run_id: suite.run_id.clone(),
                    scenario: scenario.scenario.clone(),
                    outcome: scenario.outcome,
                });
                for repetition in &scenario.repetitions {
                    for measurement in &repetition.measurements {
                        measurement_observations.push(MeasurementObservation {
                            started_unix_millis: suite.started_unix_millis,
                            run_id: suite.run_id.clone(),
                            scenario: scenario.scenario.clone(),
                            repetition: repetition.repetition,
                            measurement: measurement.clone(),
                        });
                    }
                }
            }
            Some(suite)
        } else {
            None
        };
        let replayed_from_runs = manifest
            .firmware
            .iter()
            .filter_map(|artifact| {
                artifact
                    .replayed_from
                    .as_ref()
                    .map(|origin| origin.source_run_id.clone())
            })
            .collect();
        runs.push(RunHistoryEntry {
            run_id: manifest.run_id,
            state: manifest.state,
            outcome: suite.as_ref().map(|suite| suite.outcome),
            started_unix_millis: manifest.started_unix_millis,
            finished_unix_millis: manifest.finished_unix_millis,
            duration_millis: manifest.duration_millis,
            commit: manifest.repository.commit,
            dirty: manifest.repository.dirty,
            workspace_sha256: manifest.repository.workspace_sha256,
            replayed_from_runs,
            cell_id: manifest.cell.cell_id,
            device_id: manifest.cell.device_id,
            suite_counts: suite.map(|suite| suite.counts),
            suite_report: (manifest.state == RunState::Completed)
                .then(|| relative_directory.join("suite.json")),
            html_report: (manifest.state == RunState::Completed)
                .then(|| relative_directory.join("report.html")),
            run_directory: relative_directory,
        });
    }

    runs.sort_by(|left, right| {
        right
            .started_unix_millis
            .cmp(&left.started_unix_millis)
            .then_with(|| right.run_id.cmp(&left.run_id))
    });
    observations.sort_by(|left, right| {
        left.started_unix_millis
            .cmp(&right.started_unix_millis)
            .then_with(|| left.run_id.cmp(&right.run_id))
            .then_with(|| left.scenario.cmp(&right.scenario))
    });
    measurement_observations.sort_by(|left, right| {
        left.started_unix_millis
            .cmp(&right.started_unix_millis)
            .then_with(|| left.run_id.cmp(&right.run_id))
            .then_with(|| left.scenario.cmp(&right.scenario))
            .then_with(|| left.repetition.cmp(&right.repetition))
            .then_with(|| left.measurement.name.cmp(&right.measurement.name))
    });
    let source_watermark_unix_millis = runs
        .iter()
        .map(|run| run.finished_unix_millis.unwrap_or(run.started_unix_millis))
        .max()
        .unwrap_or(0);
    let report = HistoryReport {
        schema: RUN_SCHEMA,
        target: target.to_owned(),
        source_watermark_unix_millis,
        counts: history_counts(&runs),
        scenarios: scenario_trends(observations),
        measurements: measurement_trends(measurement_observations),
        runs,
    };
    let history_report = target_directory.join("history.json");
    let html_report = target_directory.join("history.html");
    atomic_json(&history_report, &report)?;
    atomic_write(&html_report, render_html(&report).as_bytes())?;
    Ok(HistoryCompletion {
        schema: RUN_SCHEMA,
        target: target.to_owned(),
        runs: report.runs.len(),
        scenarios: report.scenarios.len(),
        measurements: report.measurements.len(),
        history_report,
        html_report,
    })
}

pub(super) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| format!("invalid HIL report `{}`: {error}", path.display()).into())
}

pub(super) fn validate_manifest(
    manifest: &RunManifest,
    target: &str,
    directory: &Path,
) -> Result<()> {
    if manifest.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL manifest `{}` has schema {}, expected {RUN_SCHEMA}",
            directory.display(),
            manifest.schema
        )
        .into());
    }
    if manifest.target != target {
        return Err(format!(
            "HIL manifest `{}` targets `{}`, expected `{target}`",
            directory.display(),
            manifest.target
        )
        .into());
    }
    let timestamps_are_consistent = match manifest.state {
        RunState::Running => {
            manifest.finished_unix_millis.is_none() && manifest.duration_millis.is_none()
        }
        RunState::Completed | RunState::Interrupted => {
            manifest.finished_unix_millis.is_some()
                && manifest.duration_millis.is_some()
                && manifest.finished_unix_millis >= Some(manifest.started_unix_millis)
        }
    };
    if !timestamps_are_consistent {
        return Err(format!(
            "HIL manifest `{}` has timestamps inconsistent with state {:?}",
            directory.display(),
            manifest.state
        )
        .into());
    }
    let directory_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("HIL run directory is not UTF-8: {}", directory.display()))?;
    if manifest.run_id != directory_id {
        return Err(format!(
            "HIL manifest run ID `{}` does not match directory `{directory_id}`",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

pub(super) fn validate_suite(suite: &SuiteResult, manifest: &RunManifest) -> Result<()> {
    if suite.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL suite `{}` has schema {}, expected {RUN_SCHEMA}",
            suite.run_id, suite.schema
        )
        .into());
    }
    if suite.run_id != manifest.run_id
        || suite.target != manifest.target
        || suite.started_unix_millis != manifest.started_unix_millis
        || Some(suite.finished_unix_millis) != manifest.finished_unix_millis
        || Some(suite.duration_millis) != manifest.duration_millis
    {
        return Err(format!(
            "HIL suite `{}` does not match its manifest",
            manifest.run_id
        )
        .into());
    }
    if suite.counts != SuiteCounts::from_results(&suite.scenarios) {
        return Err(format!("HIL suite `{}` has inconsistent counts", suite.run_id).into());
    }
    let expected_outcome = if suite
        .scenarios
        .iter()
        .all(|scenario| scenario.outcome.is_passed())
    {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    if suite.outcome != expected_outcome {
        return Err(format!(
            "HIL suite `{}` has an outcome inconsistent with its scenarios",
            suite.run_id
        )
        .into());
    }
    for scenario in &suite.scenarios {
        if scenario.schema != RUN_SCHEMA {
            return Err(format!(
                "HIL scenario `{}` has schema {}, expected {RUN_SCHEMA}",
                scenario.scenario, scenario.schema
            )
            .into());
        }
        if scenario.repetitions.is_empty() {
            if scenario.outcome != Outcome::Blocked || scenario.failure.is_none() {
                return Err(format!(
                    "HIL scenario `{}` has no repetitions without a blocking failure",
                    scenario.scenario
                )
                .into());
            }
            continue;
        }
        if scenario.repetitions.len() != usize::from(scenario.required_repetitions)
            || scenario.outcome
                != aggregate_outcome(scenario.repetitions.iter().map(|entry| entry.outcome))
        {
            return Err(format!(
                "HIL scenario `{}` has inconsistent repetition results",
                scenario.scenario
            )
            .into());
        }
        for (index, repetition) in scenario.repetitions.iter().enumerate() {
            if repetition.schema != RUN_SCHEMA {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has schema {}, expected {RUN_SCHEMA}",
                    scenario.scenario, repetition.repetition, repetition.schema
                )
                .into());
            }
            if usize::from(repetition.repetition) != index + 1 {
                return Err(format!(
                    "HIL scenario `{}` has a non-canonical repetition sequence",
                    scenario.scenario
                )
                .into());
            }
            if (repetition.outcome == Outcome::Passed && repetition.failure.is_some())
                || (matches!(
                    repetition.outcome,
                    Outcome::Failed | Outcome::Broken | Outcome::Blocked | Outcome::Interrupted
                ) && repetition.failure.is_none())
            {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has an inconsistent failure record",
                    scenario.scenario, repetition.repetition
                )
                .into());
            }
            let mut names = BTreeSet::new();
            for measurement in &repetition.measurements {
                if measurement.name.is_empty()
                    || measurement.name.len() > 128
                    || !measurement.name.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'.' | b'-')
                    })
                    || !names.insert(&measurement.name)
                    || !measurement.is_consistent()
                {
                    return Err(format!(
                        "HIL scenario `{}` repetition {} has invalid measurements",
                        scenario.scenario, repetition.repetition
                    )
                    .into());
                }
            }
            if repetition
                .measurements
                .iter()
                .any(|measurement| measurement.verdict == Some(MeasurementVerdict::Failed))
                && repetition.outcome != Outcome::Failed
            {
                return Err(format!(
                    "HIL scenario `{}` repetition {} passed with a failed measurement verdict",
                    scenario.scenario, repetition.repetition
                )
                .into());
            }
        }
    }
    Ok(())
}

fn history_counts(runs: &[RunHistoryEntry]) -> HistoryCounts {
    let mut counts = HistoryCounts {
        runs: runs.len(),
        ..HistoryCounts::default()
    };
    for run in runs {
        match run.state {
            RunState::Running => counts.running += 1,
            RunState::Completed => counts.completed += 1,
            RunState::Interrupted => counts.interrupted += 1,
        }
        match run.outcome {
            Some(Outcome::Passed) => counts.passed += 1,
            Some(_) => counts.failed += 1,
            None => {}
        }
    }
    counts
}

fn scenario_trends(observations: Vec<ScenarioObservation>) -> Vec<ScenarioTrend> {
    let mut accumulated = BTreeMap::<String, TrendAccumulator>::new();
    for observation in observations {
        let trend = accumulated.entry(observation.scenario).or_default();
        trend.observations += 1;
        match observation.outcome {
            Outcome::Passed => {
                trend.passed += 1;
                trend.consecutive_non_passed = 0;
            }
            Outcome::Failed => {
                trend.failed += 1;
                trend.consecutive_non_passed += 1;
            }
            Outcome::Broken => {
                trend.broken += 1;
                trend.consecutive_non_passed += 1;
            }
            Outcome::Skipped => {
                trend.skipped += 1;
                trend.consecutive_non_passed += 1;
            }
            Outcome::Blocked => {
                trend.blocked += 1;
                trend.consecutive_non_passed += 1;
            }
            Outcome::Interrupted => {
                trend.interrupted += 1;
                trend.consecutive_non_passed += 1;
            }
        }
        trend.last_outcome = Some(observation.outcome);
        trend.last_run_id = observation.run_id;
    }
    accumulated
        .into_iter()
        .map(|(scenario, trend)| {
            let non_passed = trend.observations - trend.passed;
            let basis_points = trend.passed.saturating_mul(10_000) / trend.observations;
            ScenarioTrend {
                scenario,
                observations: trend.observations,
                passed: trend.passed,
                failed: trend.failed,
                broken: trend.broken,
                skipped: trend.skipped,
                blocked: trend.blocked,
                interrupted: trend.interrupted,
                pass_rate_basis_points: u16::try_from(basis_points).unwrap_or(10_000),
                flaky: trend.passed > 0 && non_passed > 0,
                consecutive_non_passed: trend.consecutive_non_passed,
                last_outcome: trend
                    .last_outcome
                    .expect("a scenario trend has at least one observation"),
                last_run_id: trend.last_run_id,
            }
        })
        .collect()
}

fn measurement_trends(observations: Vec<MeasurementObservation>) -> Vec<MeasurementTrend> {
    let mut accumulated = BTreeMap::<
        (String, String, MeasurementUnit, Option<Threshold>),
        MeasurementAccumulator,
    >::new();
    for observation in observations {
        let key = (
            observation.scenario,
            observation.measurement.name,
            observation.measurement.unit,
            observation.measurement.threshold,
        );
        let value = observation.measurement.value;
        let trend = accumulated.entry(key).or_insert(MeasurementAccumulator {
            observations: 0,
            minimum: value,
            maximum: value,
            latest: value,
            failed_verdicts: 0,
            last_run_id: String::new(),
        });
        trend.observations += 1;
        trend.minimum = trend.minimum.min(value);
        trend.maximum = trend.maximum.max(value);
        trend.latest = value;
        trend.failed_verdicts +=
            usize::from(observation.measurement.verdict == Some(MeasurementVerdict::Failed));
        trend.last_run_id = observation.run_id;
    }
    accumulated
        .into_iter()
        .map(
            |((scenario, measurement, unit, threshold), trend)| MeasurementTrend {
                scenario,
                measurement,
                unit,
                threshold,
                observations: trend.observations,
                minimum: trend.minimum,
                maximum: trend.maximum,
                latest: trend.latest,
                failed_verdicts: trend.failed_verdicts,
                last_run_id: trend.last_run_id,
            },
        )
        .collect()
}

fn render_html(report: &HistoryReport) -> String {
    let mut run_rows = String::new();
    for run in &report.runs {
        let outcome = run.outcome.map_or_else(
            || format!("{:?}", run.state),
            |outcome| format!("{outcome:?}"),
        );
        let counts = run.suite_counts.as_ref().map_or_else(
            || String::from("&mdash;"),
            |counts| format!("{} / {}", counts.passed, counts.scenarios),
        );
        let report_link = run.html_report.as_ref().map_or_else(
            || String::from("&mdash;"),
            |path| {
                let path = html_escape(&path.display().to_string());
                format!("<a href=\"{path}\">report</a>")
            },
        );
        let commit = run.commit.get(..12).unwrap_or(&run.commit);
        let firmware = if run.replayed_from_runs.is_empty() {
            String::from("current build")
        } else {
            format!("replay: {}", run.replayed_from_runs.join(", "))
        };
        let _ = writeln!(
            run_rows,
            "<tr><td><code>{}</code></td><td data-unix-ms=\"{}\">{}</td><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td><code>{}</code>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&run.run_id),
            run.started_unix_millis,
            run.started_unix_millis,
            html_escape(&format!("{} / {}", run.cell_id, run.device_id)),
            if run.outcome == Some(Outcome::Passed) {
                "pass"
            } else {
                "fail"
            },
            html_escape(&outcome),
            counts,
            html_escape(commit),
            if run.dirty { "*" } else { "" },
            html_escape(&firmware),
            report_link,
        );
    }

    let mut scenario_rows = String::new();
    for trend in &report.scenarios {
        let percent = f64::from(trend.pass_rate_basis_points) / 100.0;
        let _ = writeln!(
            scenario_rows,
            "<tr><td>{}</td><td>{}</td><td><div class=\"bar\"><span style=\"width:{percent:.2}%\"></span></div>{percent:.2}%</td><td class=\"{}\">{}</td><td>{}</td><td>{:?}</td></tr>",
            html_escape(&trend.scenario),
            trend.observations,
            if trend.flaky { "warn" } else { "" },
            trend.flaky,
            trend.consecutive_non_passed,
            trend.last_outcome,
        );
    }

    let mut measurement_rows = String::new();
    for trend in &report.measurements {
        let threshold = trend.threshold.map_or_else(
            || String::from("&mdash;"),
            |threshold| {
                format!(
                    "{} {} {}",
                    threshold.comparison.symbol(),
                    threshold.value,
                    trend.unit.id()
                )
            },
        );
        let _ = writeln!(
            measurement_rows,
            "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{} / {} / {} {}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&trend.scenario),
            html_escape(&trend.measurement),
            trend.observations,
            trend.minimum,
            trend.latest,
            trend.maximum,
            trend.unit.id(),
            threshold,
            trend.failed_verdicts,
        );
    }

    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>HIL history · {target}</title>\
         <style>body{{font:14px system-ui,sans-serif;margin:2rem;max-width:1400px}}\
         table{{border-collapse:collapse;width:100%;margin-bottom:2rem}}th,td{{border:1px solid #bbb;padding:.45rem;text-align:left}}\
         .pass{{color:#087830;font-weight:700}}.fail{{color:#b42318;font-weight:700}}.warn{{color:#a15c00;font-weight:700}}\
         code{{background:#eee;padding:.1rem .25rem}}.bar{{display:inline-block;width:8rem;height:.6rem;background:#eee;margin-right:.5rem}}\
         .bar span{{display:block;height:100%;background:#14804a}}</style></head><body>\
         <h1>HIL history · {target}</h1><p>{passed}/{completed} completed runs passed · {interrupted} interrupted · {running} running</p>\
         <h2>Runs</h2><table><thead><tr><th>Run</th><th>Started</th><th>Cell / DUT</th><th>Outcome</th><th>Passed scenarios</th><th>Commit</th><th>Firmware</th><th>Report</th></tr></thead>\
         <tbody>{run_rows}</tbody></table><h2>Scenario stability</h2>\
         <table><thead><tr><th>Scenario</th><th>Runs</th><th>Pass rate</th><th>Flaky</th><th>Consecutive non-passed</th><th>Last outcome</th></tr></thead>\
         <tbody>{scenario_rows}</tbody></table><h2>Measurement trends</h2>\
         <table><thead><tr><th>Scenario</th><th>Measurement</th><th>Observations</th><th>Min / latest / max</th><th>Threshold</th><th>Failed verdicts</th></tr></thead>\
         <tbody>{measurement_rows}</tbody></table><script>for(const e of document.querySelectorAll('[data-unix-ms]')){{e.textContent=new Date(Number(e.dataset.unixMs)).toLocaleString()}}</script></body></html>\n",
        target = html_escape(&report.target),
        passed = report.counts.passed,
        completed = report.counts.completed,
        interrupted = report.counts.interrupted,
        running = report.counts.running,
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        qualification::scenario::ImageClass,
        reporting::run::{
            Comparison, Failure, FailureKind, Measurement, MeasurementUnit, RepetitionResult,
            ScenarioResult,
        },
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_target(label: &str) -> PathBuf {
        let target = std::env::temp_dir().join(format!(
            "open-radio-hil-history-{label}-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(target.join("runs")).unwrap();
        target
    }

    fn write_completed_run(
        target: &Path,
        run_id: &str,
        started_unix_millis: u64,
        outcome: Outcome,
    ) {
        let directory = target.join("runs").join(run_id);
        fs::create_dir_all(&directory).unwrap();
        atomic_json(
            &directory.join("manifest.json"),
            &serde_json::json!({
                "schema": RUN_SCHEMA,
                "run_id": run_id,
                "target": "esp32s31",
                "state": "completed",
                "started_unix_millis": started_unix_millis,
                "finished_unix_millis": started_unix_millis + 100,
                "duration_millis": 100,
                "invocation": ["cargo", "hil"],
                "repository": {
                    "commit": "0123456789abcdef",
                    "dirty": false,
                    "workspace_sha256": "00"
                },
                "runner": {
                    "package": "runner",
                    "version": "1",
                    "protocol_version": 1,
                    "host_os": "linux",
                    "host_arch": "x86_64",
                    "tools": []
                },
                "cell": {
                    "cell_id": "cell-1",
                    "device_id": "dut-1",
                    "serial_device": "/dev/ttyACM0"
                },
                "firmware": []
            }),
        )
        .unwrap();
        let scenarios = vec![ScenarioResult::from_repetitions(
            String::from("udp-rx"),
            ImageClass::Correctness,
            1,
            vec![RepetitionResult {
                schema: RUN_SCHEMA,
                repetition: 1,
                outcome,
                started_unix_millis,
                duration_millis: 100,
                artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
                attachments: Vec::new(),
                measurements: vec![
                    Measurement::observed(
                        "icmp.rtt.p95",
                        if outcome == Outcome::Passed {
                            900
                        } else {
                            1_100
                        },
                        MeasurementUnit::Microseconds,
                    )
                    .evaluated(Comparison::AtMost, 1_000),
                ],
                failure: (outcome != Outcome::Passed)
                    .then(|| Failure::new(FailureKind::Scenario, "measurement failed")),
            }],
        )];
        let counts = SuiteCounts {
            scenarios: 1,
            passed: usize::from(outcome == Outcome::Passed),
            failed: usize::from(outcome == Outcome::Failed),
            broken: usize::from(outcome == Outcome::Broken),
            skipped: usize::from(outcome == Outcome::Skipped),
            blocked: usize::from(outcome == Outcome::Blocked),
            interrupted: usize::from(outcome == Outcome::Interrupted),
        };
        atomic_json(
            &directory.join("suite.json"),
            &SuiteResult {
                schema: RUN_SCHEMA,
                run_id: run_id.to_owned(),
                target: String::from("esp32s31"),
                outcome: if outcome == Outcome::Passed {
                    Outcome::Passed
                } else {
                    Outcome::Failed
                },
                started_unix_millis,
                finished_unix_millis: started_unix_millis + 100,
                duration_millis: 100,
                counts,
                scenarios,
            },
        )
        .unwrap();
    }

    #[test]
    fn history_is_rebuilt_from_runs_and_exposes_flakiness() {
        let target = temporary_target("trends");
        write_completed_run(&target, "run-a", 1, Outcome::Passed);
        write_completed_run(&target, "run-b", 2, Outcome::Failed);

        let completion = rebuild_at(&target, "esp32s31").unwrap();
        let first_json = fs::read(&completion.history_report).unwrap();
        let first_html = fs::read(&completion.html_report).unwrap();
        let report: HistoryReport = read_json(&completion.history_report).unwrap();
        assert_eq!(report.counts.runs, 2);
        assert_eq!(report.counts.passed, 1);
        assert_eq!(report.counts.failed, 1);
        assert_eq!(report.runs[0].run_id, "run-b");
        assert_eq!(report.scenarios.len(), 1);
        assert_eq!(report.scenarios[0].pass_rate_basis_points, 5_000);
        assert!(report.scenarios[0].flaky);
        assert_eq!(report.scenarios[0].consecutive_non_passed, 1);
        assert_eq!(report.scenarios[0].last_outcome, Outcome::Failed);
        assert_eq!(report.measurements.len(), 1);
        assert_eq!(report.measurements[0].minimum, 900);
        assert_eq!(report.measurements[0].latest, 1_100);
        assert_eq!(report.measurements[0].maximum, 1_100);
        assert_eq!(report.measurements[0].failed_verdicts, 1);
        let html = fs::read_to_string(completion.html_report).unwrap();
        assert!(html.contains("Scenario stability"));
        assert!(html.contains("Measurement trends"));
        assert!(html.contains("runs/run-b/report.html"));
        let rebuilt = rebuild_at(&target, "esp32s31").unwrap();
        assert_eq!(fs::read(rebuilt.history_report).unwrap(), first_json);
        assert_eq!(fs::read(rebuilt.html_report).unwrap(), first_html);
        fs::remove_dir_all(target).unwrap();
    }

    #[test]
    fn malformed_run_bundle_fails_closed() {
        let target = temporary_target("invalid");
        fs::create_dir(target.join("runs/run-without-manifest")).unwrap();
        let error = rebuild_at(&target, "esp32s31").unwrap_err();
        assert!(error.to_string().contains("has no manifest"));
        fs::remove_dir_all(target).unwrap();
    }
}
