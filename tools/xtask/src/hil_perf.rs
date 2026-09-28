//! Performance of gated HIL measurements across commits.
//!
//! A scenario's gated measurements (those with an `at-least` or `at-most`
//! threshold, such as UDP rates) are its performance figures; the threshold
//! also fixes which direction is better. This module summarizes them per
//! commit from the shared run store, keeps one reviewed baseline per scenario
//! and reports a run whose figures moved the wrong way by more than the
//! baseline's noise. It reads sealed bundles only and never decides
//! qualification, which remains the evaluator's.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use oer_hil_schema::run::{Comparison, MeasurementUnit, Threshold};

use crate::{
    Result,
    hil_runs::{self, Run, State},
};

/// Which way a gated measurement improves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Better {
    Higher,
    Lower,
}

impl Better {
    /// The direction an `at-least` or `at-most` gate prefers; an `exactly`
    /// gate is a correctness check, not a performance figure.
    fn from_threshold(threshold: &Threshold) -> Option<(Self, f64)> {
        let value = threshold.value as f64;
        match threshold.comparison {
            Comparison::AtLeast => Some((Self::Higher, value)),
            Comparison::AtMost => Some((Self::Lower, value)),
            Comparison::Exactly => None,
        }
    }
}

/// Repetition values of one gated measurement of one scenario in one run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Sample {
    pub scenario: String,
    pub measurement: String,
    pub unit: MeasurementUnit,
    pub better: Better,
    pub threshold: f64,
    pub values: Vec<f64>,
}

/// Mean and sample standard deviation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Spread {
    pub mean: f64,
    pub deviation: f64,
    pub count: usize,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let deviation = if count > 1 {
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64).sqrt()
        } else {
            0.0
        };
        Some(Self {
            mean,
            deviation,
            count,
        })
    }
}

/// Gated measurements of every scenario of `run`, one sample per
/// `(scenario, measurement)` with a value in each repetition that has one.
pub fn samples(run: &Run) -> Vec<Sample> {
    let mut samples: BTreeMap<(String, String), Sample> = BTreeMap::new();
    for scenario in &run.scenarios {
        for measurement in scenario.repetitions.iter().flat_map(|r| &r.measurements) {
            let (Some(value), Some(threshold)) = (measurement.value, &measurement.threshold) else {
                continue;
            };
            let Some((better, limit)) = Better::from_threshold(threshold) else {
                continue;
            };
            samples
                .entry((scenario.id.clone(), measurement.name.clone()))
                .or_insert_with(|| Sample {
                    scenario: scenario.id.clone(),
                    measurement: measurement.name.clone(),
                    unit: measurement.unit,
                    better,
                    threshold: limit,
                    values: Vec::new(),
                })
                .values
                .push(value);
        }
    }
    samples.into_values().collect()
}

/// What performance reporting needs of one run, cached per sealed run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunSummary {
    pub version: u32,
    pub id: String,
    pub started_millis: u64,
    pub state: State,
    pub commit: Option<String>,
    pub dirty: bool,
    pub networks: Vec<String>,
    /// Layout seeds of the run's images; empty when every image kept the
    /// linker's natural order.
    #[serde(default)]
    pub layout_seeds: Vec<u32>,
    pub samples: Vec<Sample>,
}

/// Bumped whenever [`RunSummary`] or [`samples`] changes meaning, so cached
/// summaries are recomputed.
const SUMMARY_VERSION: u32 = 3;

pub fn summary(run: &Run) -> RunSummary {
    RunSummary {
        version: SUMMARY_VERSION,
        id: run.id.clone(),
        started_millis: run.started_millis,
        state: run.state,
        commit: run.commit.clone(),
        dirty: run.dirty,
        networks: networks(run),
        layout_seeds: layout_seeds(run),
        samples: samples(run),
    }
}

fn manifest_firmware(run: &Run) -> Vec<Value> {
    fs::read(run.directory.join("manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|manifest| manifest["firmware"].as_array().cloned())
        .unwrap_or_default()
}

/// The layout seeds the run's images were linked with, in ascending order.
pub fn layout_seeds(run: &Run) -> Vec<u32> {
    let mut seeds = manifest_firmware(run)
        .iter()
        .filter_map(|firmware| firmware["layout_seed"].as_u64())
        .filter_map(|seed| u32::try_from(seed).ok())
        .collect::<Vec<_>>();
    seeds.sort_unstable();
    seeds.dedup();
    seeds
}

/// Which code placement a run measured: the linker's natural order or the
/// seeds its images were shuffled with.
fn layout(seeds: &[u32]) -> String {
    match seeds {
        [] => "natural".to_owned(),
        seeds => format!(
            "seed {}",
            seeds
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// The network implementations recorded in the run's build provenance.
pub fn networks(run: &Run) -> Vec<String> {
    let mut networks = manifest_firmware(run)
        .iter()
        .filter_map(|firmware| firmware["build_provenance_path"].as_str())
        .filter_map(|path| fs::read(run.directory.join(path)).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter_map(|provenance| {
            provenance["parameters"]["network"]
                .as_str()
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    networks.sort();
    networks.dedup();
    networks
}

/// A reviewed reference for one scenario's gated measurements.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Baseline {
    pub run: String,
    pub commit: Option<String>,
    pub reason: String,
    pub by: String,
    pub unix_millis: u64,
    pub measurements: BTreeMap<String, BaselineMeasurement>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BaselineMeasurement {
    pub unit: MeasurementUnit,
    pub better: Better,
    pub spread: Spread,
}

/// Baselines by scenario, kept beside the run store so every checkout shares
/// them.
pub fn baselines_path(store: &Path) -> PathBuf {
    store.join("perf-baselines.json")
}

pub fn load_baselines(store: &Path) -> Result<BTreeMap<String, Baseline>> {
    match fs::read(baselines_path(store)) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

fn save_baselines(store: &Path, baselines: &BTreeMap<String, Baseline>) -> Result<()> {
    let path = baselines_path(store);
    let mut file = tempfile::NamedTempFile::new_in(store)?;
    serde_json::to_writer_pretty(&mut file, baselines)?;
    file.persist(path)?;
    Ok(())
}

/// Make `run` the baseline of each named scenario (all its gated scenarios
/// when none is named). Only a completed run of a clean commit can be one.
pub fn set_baseline(
    store: &Path,
    run: &RunSummary,
    scenarios: &[String],
    reason: &str,
    by: &str,
    unix_millis: u64,
) -> Result<Vec<String>> {
    if run.dirty || run.commit.is_none() {
        return Err(format!("run {} was not built from a clean commit", run.id).into());
    }
    if run.state != State::Completed {
        return Err(format!("run {} is {}, not completed", run.id, run.state).into());
    }
    let mut by_scenario: BTreeMap<String, BTreeMap<String, BaselineMeasurement>> = BTreeMap::new();
    for sample in run.samples.iter().cloned() {
        if !scenarios.is_empty() && !scenarios.contains(&sample.scenario) {
            continue;
        }
        if let Some(spread) = Spread::of(&sample.values) {
            by_scenario.entry(sample.scenario).or_default().insert(
                sample.measurement,
                BaselineMeasurement {
                    unit: sample.unit,
                    better: sample.better,
                    spread,
                },
            );
        }
    }
    for scenario in scenarios {
        if !by_scenario.contains_key(scenario) {
            return Err(format!("run {} has no gated measurement of {scenario}", run.id).into());
        }
    }
    let mut baselines = load_baselines(store)?;
    for (scenario, measurements) in &by_scenario {
        baselines.insert(
            scenario.clone(),
            Baseline {
                run: run.id.clone(),
                commit: run.commit.clone(),
                reason: reason.to_owned(),
                by: by.to_owned(),
                unix_millis,
                measurements: measurements.clone(),
            },
        );
    }
    save_baselines(store, &baselines)?;
    Ok(by_scenario.into_keys().collect())
}

/// How a measurement compares with its baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Improved,
    Within,
    Regressed,
}

/// Relative tolerance below which a change is noise even for a perfectly
/// repeatable baseline.
const MINIMUM_TOLERANCE: f64 = 0.02;

/// Compare `current` with `baseline`: a move in the worse direction by more
/// than twice the baseline's deviation and 2 % of its mean is a regression;
/// the same margin in the better direction is an improvement.
pub fn change(baseline: &BaselineMeasurement, current: &Spread) -> Change {
    let margin =
        (2.0 * baseline.spread.deviation).max(MINIMUM_TOLERANCE * baseline.spread.mean.abs());
    let delta = current.mean - baseline.spread.mean;
    let worse = match baseline.better {
        Better::Higher => -delta,
        Better::Lower => delta,
    };
    if worse > margin {
        Change::Regressed
    } else if -worse > margin {
        Change::Improved
    } else {
        Change::Within
    }
}

fn short(commit: &Option<String>) -> String {
    commit
        .as_deref()
        .map_or_else(|| "unknown".to_owned(), |c| c[..c.len().min(9)].to_owned())
}

/// Display factor and unit for a measured unit and magnitude.
fn scale(magnitude: f64, unit: MeasurementUnit) -> (f64, String) {
    match unit {
        MeasurementUnit::BitsPerSecond => (1e6, "Mbit/s".into()),
        MeasurementUnit::Microseconds if magnitude.abs() >= 1000.0 => (1000.0, "ms".into()),
        unit => (1.0, unit.to_string()),
    }
}

fn format_spread(spread: &Spread, unit: MeasurementUnit) -> String {
    let (factor, unit) = scale(spread.mean, unit);
    format!(
        "{:.2}±{:.2} {unit} (n={})",
        spread.mean / factor,
        spread.deviation / factor,
        spread.count
    )
}

/// The values of one gated measurement from every clean run of one commit
/// linked with one layout.
struct CommitRow {
    commit: String,
    layout: String,
    runs: String,
    values: Vec<f64>,
    sample: Sample,
}

/// Per-commit summary of the gated measurements of `scenarios` (every gated
/// scenario when empty) in clean runs, oldest first, with each commit
/// compared to the scenario's baseline.
pub fn report(
    runs: &[RunSummary],
    scenarios: &[String],
    measurement: Option<&str>,
    baselines: &BTreeMap<String, Baseline>,
) -> String {
    // (scenario, measurement) -> one row per commit, in first-run order.
    let mut rows: BTreeMap<(String, String), Vec<CommitRow>> = BTreeMap::new();
    for run in runs.iter().filter(|run| !run.dirty && run.commit.is_some()) {
        for sample in run.samples.iter().cloned() {
            if !scenarios.is_empty() && !scenarios.contains(&sample.scenario) {
                continue;
            }
            if measurement.is_some_and(|filter| !sample.measurement.contains(filter)) {
                continue;
            }
            let commit = short(&run.commit);
            let layout = layout(&run.layout_seeds);
            let entries = rows
                .entry((sample.scenario.clone(), sample.measurement.clone()))
                .or_default();
            match entries
                .iter_mut()
                .find(|entry| entry.commit == commit && entry.layout == layout)
            {
                Some(entry) => {
                    entry.values.extend(&sample.values);
                    entry.runs.push(',');
                    entry.runs.push_str(&run.id);
                }
                None => entries.push(CommitRow {
                    commit,
                    layout,
                    runs: run.id.clone(),
                    values: sample.values.clone(),
                    sample,
                }),
            }
        }
    }
    if rows.is_empty() {
        return "no clean run has a gated measurement of the selection\n".into();
    }
    let mut text = String::new();
    for ((scenario, name), entries) in rows {
        let first = &entries[0].sample;
        let direction = match first.better {
            Better::Higher => "higher is better",
            Better::Lower => "lower is better",
        };
        let (factor, unit) = scale(first.threshold, first.unit);
        text.push_str(&format!(
            "{scenario} {name} — gate {:.2} {unit}, {direction}\n",
            first.threshold / factor
        ));
        let baseline = baselines
            .get(&scenario)
            .and_then(|baseline| Some((baseline, baseline.measurements.get(&name)?)));
        if let Some((baseline, measurement)) = baseline {
            text.push_str(&format!(
                "  baseline {} {}: {} — {}\n",
                short(&baseline.commit),
                baseline.run,
                format_spread(&measurement.spread, measurement.unit),
                baseline.reason
            ));
        } else {
            text.push_str("  no baseline\n");
        }
        text.push_str(&layout_sensitivity(&entries));
        for CommitRow {
            commit,
            layout,
            runs: run_ids,
            values,
            sample,
        } in entries
        {
            let Some(spread) = Spread::of(&values) else {
                continue;
            };
            let verdict = match baseline.map(|(_, measurement)| change(measurement, &spread)) {
                Some(Change::Regressed) => "  REGRESSED",
                Some(Change::Improved) => "  improved",
                Some(Change::Within) => "  within noise",
                None => "",
            };
            let gate = match sample.better {
                Better::Higher if spread.mean < sample.threshold => "  below gate",
                Better::Lower if spread.mean > sample.threshold => "  above gate",
                _ => "",
            };
            text.push_str(&format!(
                "  {commit:<10} {layout:<10} {}{verdict}{gate}  [{run_ids}]\n",
                format_spread(&spread, sample.unit)
            ));
        }
    }
    text
}

/// For each commit measured with more than one layout, how far the layout
/// means spread compared with the repetitions within one layout: a spread
/// well above the repetition noise means placement, not code, moves the
/// figure, and a single-layout comparison of two commits can mislead.
fn layout_sensitivity(entries: &[CommitRow]) -> String {
    let mut commits: Vec<&str> = Vec::new();
    for entry in entries {
        if !commits.contains(&entry.commit.as_str()) {
            commits.push(&entry.commit);
        }
    }
    let mut text = String::new();
    for commit in commits {
        let layouts = entries
            .iter()
            .filter(|entry| entry.commit == commit)
            .filter_map(|entry| Spread::of(&entry.values).map(|spread| (entry, spread)))
            .collect::<Vec<_>>();
        if layouts.len() < 2 {
            continue;
        }
        let means = layouts
            .iter()
            .map(|(_, spread)| spread.mean)
            .collect::<Vec<_>>();
        let Some(between) = Spread::of(&means) else {
            continue;
        };
        let within = layouts
            .iter()
            .map(|(_, spread)| spread.deviation)
            .sum::<f64>()
            / layouts.len() as f64;
        let unit = layouts[0].0.sample.unit;
        let (factor, unit_name) = scale(between.mean, unit);
        text.push_str(&format!(
            "  {commit:<10} across {} layouts: means {:.2}±{:.2} {unit_name}, repetitions ±{:.2}\n",
            layouts.len(),
            between.mean / factor,
            between.deviation / factor,
            within / factor
        ));
    }
    text
}

/// Every gated measurement of `run` that regressed against its scenario's
/// baseline, as report lines; empty when none did.
pub fn regressions(run: &RunSummary, baselines: &BTreeMap<String, Baseline>) -> Vec<String> {
    run.samples
        .iter()
        .filter_map(|sample| {
            let baseline = baselines.get(&sample.scenario)?;
            let reference = baseline.measurements.get(&sample.measurement)?;
            let spread = Spread::of(&sample.values)?;
            (change(reference, &spread) == Change::Regressed).then(|| {
                format!(
                    "{} {}: {} against baseline {} {}",
                    sample.scenario,
                    sample.measurement,
                    format_spread(&spread, sample.unit),
                    format_spread(&reference.spread, reference.unit),
                    baseline.run
                )
            })
        })
        .collect()
}

/// Summaries of the runs started at or after `since_millis`, oldest first.
/// Run directories are named by their start time, so older runs are skipped
/// without reading their bundles; a sealed run's summary is cached in
/// `cache` and reused, since its bundle can no longer change.
pub fn summaries_since(
    directory: &Path,
    cache: &Path,
    since_millis: u64,
) -> Result<Vec<RunSummary>> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(found);
    };
    fs::create_dir_all(cache)?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let started = name
            .split('-')
            .next()
            .and_then(|millis| millis.parse::<u64>().ok());
        if started.is_some_and(|started| started < since_millis) || !entry.file_type()?.is_dir() {
            continue;
        }
        let cached = cache.join(format!("{name}.json"));
        let reused = fs::read(&cached)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RunSummary>(&bytes).ok())
            .filter(|summary| summary.version == SUMMARY_VERSION);
        let summary = match reused {
            Some(summary) => summary,
            None => {
                let Some(run) = hil_runs::load(&entry.path()) else {
                    continue;
                };
                let computed = summary(&run);
                if computed.state.is_sealed() {
                    let mut file = tempfile::NamedTempFile::new_in(cache)?;
                    serde_json::to_writer(&mut file, &computed)?;
                    file.persist(&cached)?;
                }
                computed
            }
        };
        found.push(summary);
    }
    found.sort_by_key(|run| (run.started_millis, run.id.clone()));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hil_runs::{Measurement, Repetition, ScenarioRun};
    use oer_hil_schema::run::{MeasurementVerdict, Outcome};

    fn run(id: &str, commit: &str, dirty: bool, values: &[f64]) -> Run {
        Run {
            id: id.into(),
            directory: PathBuf::from("/nonexistent"),
            started_millis: 1,
            state: State::Completed,
            outcome: Some(Outcome::Passed),
            commit: Some(commit.into()),
            dirty,
            checkout: None,
            images: Vec::new(),
            replayed: Vec::new(),
            scenarios: vec![ScenarioRun {
                id: "udp-tx".into(),
                image: "performance".into(),
                outcome: Outcome::Passed,
                failure: None,
                repetitions: values
                    .iter()
                    .enumerate()
                    .map(|(number, value)| Repetition {
                        number: number as u64 + 1,
                        outcome: Outcome::Passed,
                        failure: None,
                        measurements: vec![
                            Measurement {
                                name: "udp.tx.host-rate".into(),
                                value: Some(*value),
                                unit: MeasurementUnit::BitsPerSecond,
                                threshold: Some(Threshold {
                                    comparison: Comparison::AtLeast,
                                    value: 100_000_000,
                                }),
                                verdict: Some(MeasurementVerdict::Passed),
                            },
                            Measurement {
                                name: "ungated".into(),
                                value: Some(1.0),
                                unit: MeasurementUnit::Count,
                                threshold: None,
                                verdict: None,
                            },
                        ],
                        directory: None,
                    })
                    .collect(),
            }],
            observer: None,
        }
    }

    #[test]
    fn only_gated_measurements_are_samples_and_the_gate_sets_the_direction() {
        let samples = samples(&run("r", "c", false, &[115e6, 116e6]));
        assert_eq!(summary(&run("r", "c", false, &[1.0])).samples.len(), 1);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].better, Better::Higher);
        assert_eq!(samples[0].values, [115e6, 116e6]);
        let gate = |comparison| Threshold {
            comparison,
            value: 3,
        };
        assert_eq!(
            Better::from_threshold(&gate(Comparison::AtMost)),
            Some((Better::Lower, 3.0))
        );
        assert_eq!(Better::from_threshold(&gate(Comparison::Exactly)), None);
    }

    #[test]
    fn a_change_is_judged_against_the_baseline_noise_in_the_better_direction() {
        let baseline = BaselineMeasurement {
            unit: MeasurementUnit::BitsPerSecond,
            better: Better::Higher,
            spread: Spread::of(&[115e6, 116e6, 117e6]).unwrap(),
        };
        let within = Spread::of(&[114e6]).unwrap();
        let lower = Spread::of(&[105e6]).unwrap();
        let higher = Spread::of(&[125e6]).unwrap();
        assert_eq!(change(&baseline, &within), Change::Within);
        assert_eq!(change(&baseline, &lower), Change::Regressed);
        assert_eq!(change(&baseline, &higher), Change::Improved);
        let latency = BaselineMeasurement {
            better: Better::Lower,
            ..baseline
        };
        assert_eq!(change(&latency, &higher), Change::Regressed);
        assert_eq!(change(&latency, &lower), Change::Improved);
    }

    #[test]
    fn baselines_come_only_from_clean_sealed_runs_and_flag_regressions() {
        let store = tempfile::tempdir().unwrap();
        let run = |id, commit, dirty, values: &[f64]| summary(&run(id, commit, dirty, values));
        let dirty = run("d", "c", true, &[115e6]);
        assert!(set_baseline(store.path(), &dirty, &[], "r", "me", 0).is_err());
        let clean = run("b", "c", false, &[115e6, 116e6, 117e6]);
        assert_eq!(
            set_baseline(store.path(), &clean, &[], "owned-xarxa", "me", 0).unwrap(),
            ["udp-tx"]
        );
        assert!(set_baseline(store.path(), &clean, &["other".into()], "r", "me", 0).is_err());
        let baselines = load_baselines(store.path()).unwrap();
        assert!(regressions(&run("x", "d", false, &[116e6]), &baselines).is_empty());
        assert_eq!(
            regressions(&run("y", "e", false, &[100e6]), &baselines).len(),
            1
        );
        let text = report(
            &[clean.clone(), run("y", "e", false, &[100e6]), dirty],
            &[],
            None,
            &baselines,
        );
        assert!(text.contains("REGRESSED"), "{text}");
        assert!(!text.contains("[d]"), "dirty runs are not reported: {text}");
    }

    #[test]
    fn sealed_summaries_are_reused_and_older_runs_are_not_read() {
        let store = tempfile::tempdir().unwrap();
        let (runs, cache) = (store.path().join("runs"), store.path().join("cache"));
        for name in ["1000-old", "3000-new"] {
            fs::create_dir_all(runs.join(name)).unwrap();
        }
        // The new run's bundle is empty: only its cached summary can supply it.
        fs::create_dir_all(&cache).unwrap();
        let mut cached = summary(&run("3000-new", "c", false, &[115e6]));
        cached.started_millis = 3000;
        fs::write(
            cache.join("3000-new.json"),
            serde_json::to_vec(&cached).unwrap(),
        )
        .unwrap();
        let found = summaries_since(&runs, &cache, 2000).unwrap();
        assert_eq!(found, [cached.clone()]);
        let stale = RunSummary {
            version: 0,
            ..cached
        };
        fs::write(
            cache.join("3000-new.json"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert!(summaries_since(&runs, &cache, 2000).unwrap().is_empty());
    }

    #[test]
    fn the_layout_seed_comes_from_each_flashed_image() {
        let store = tempfile::tempdir().unwrap();
        let mut seeded = run("s", "c", false, &[1.0]);
        seeded.directory = store.path().to_owned();
        fs::write(
            store.path().join("manifest.json"),
            r#"{"firmware":[{"image":"performance","layout_seed":7},{"image":"peer"}]}"#,
        )
        .unwrap();
        assert_eq!(layout_seeds(&seeded), [7]);
        assert!(layout_seeds(&run("n", "c", false, &[1.0])).is_empty());
        assert_eq!(layout(&[]), "natural");
        assert_eq!(layout(&[7, 11]), "seed 7,11");
    }

    #[test]
    fn layouts_of_one_commit_are_reported_apart_with_their_spread() {
        let seeded = |id, seed, values: &[f64]| {
            let mut summary = summary(&run(id, "c", false, values));
            summary.layout_seeds = vec![seed];
            summary
        };
        let runs = [
            summary(&run("n", "c", false, &[100e6, 101e6])),
            seeded("a", 7, &[120e6, 121e6]),
            seeded("b", 11, &[90e6, 91e6]),
        ];
        let text = report(&runs, &[], None, &BTreeMap::new());
        assert!(text.contains("natural"), "{text}");
        assert!(text.contains("seed 7"), "{text}");
        assert!(text.contains("seed 11"), "{text}");
        assert!(text.contains("across 3 layouts"), "{text}");
        // One layout alone reports no layout sensitivity.
        let single = report(&runs[..1], &[], None, &BTreeMap::new());
        assert!(!single.contains("across"), "{single}");
    }
}
