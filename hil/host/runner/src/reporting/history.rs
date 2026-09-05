//! Rebuildable history and stability views derived from immutable run bundles.

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::evidence::run::{
    Measurement, MeasurementUnit, MeasurementVerdict, Outcome, RUN_SCHEMA, RunManifest, RunState,
    SuiteCounts, SuiteResult, Threshold, atomic_json, atomic_write,
};

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

pub(crate) fn rebuild_at(target_directory: &Path, target: &str) -> Result<HistoryCompletion> {
    let runs_directory = target_directory.join("runs");
    fs::create_dir_all(&runs_directory)?;
    let _publication = crate::evidence::run::IndexGuard::acquire(target_directory)?;
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
         <label>Filter measurements <input id=\"measurement-filter\" type=\"search\" placeholder=\"scenario or measurement name\"></label>\
         <table id=\"measurement-trends\"><thead><tr><th>Scenario</th><th>Measurement</th><th>Observations</th><th>Min / latest / max</th><th>Threshold</th><th>Failed verdicts</th></tr></thead>\
         <tbody>{measurement_rows}</tbody></table><script>for(const e of document.querySelectorAll('[data-unix-ms]')){{e.textContent=new Date(Number(e.dataset.unixMs)).toLocaleString()}};document.getElementById('measurement-filter').addEventListener('input',event=>{{const query=event.target.value.toLowerCase();for(const row of document.querySelectorAll('#measurement-trends tbody tr')){{row.hidden=!row.textContent.toLowerCase().includes(query)}}}})</script></body></html>\n",
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
mod tests;

use crate::evidence::run::validation::{read_json, validate_manifest, validate_suite};
