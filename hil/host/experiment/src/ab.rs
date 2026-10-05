//! A/B experiments: two variants of the firmware on the same scenarios,
//! with enough repetitions to tell an effect from noise.
//!
//! A variant is a repository revision plus local checkouts that replace its
//! pinned dependencies (the `ESP_HAL_ROOT`, `EMBASSY_ROOT` and
//! `OPEN_RADIO_XARXA_ROOT` roots). Each arm's revision is checked out in the
//! experiment's worktree below `target/hil/ab/<id>/` and captured with its
//! overrides into a source snapshot, which the experiment's runner builds:
//! both arms therefore need the checkout's HIL protocol version.
//!
//! For every layout seed, the first round runs A, then B, each building its
//! images. The remaining rounds replay those exact images; each round, A then
//! B of one seed, holds a whole-stand lease of its own, so drift of the air
//! and the calibrations falls on both arms alike, and other owners' shorter
//! work goes between rounds. Every run is launched through
//! [`crate::launch::launch_run`] as a job of its own, records its arm and
//! variant in its manifest ([`oer_hil_run_bundle::experiment`]) and no
//! evidence. The report compares, per scenario and measurement, the arms'
//! run means ([`oer_hil_analysis::arms`]) and is written to `ab-report.json`.

use std::{
    collections::BTreeMap,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_analysis::{
    Run,
    arms::{self, MeasurementComparison},
};
use oer_hil_image_class::FeatureDelta;
use oer_hil_run_bundle::{
    RunId, RunStore,
    experiment::{Arm, DependencyOverride, Experiment, Variant},
};
use oer_hil_schema::run::Outcome;
use oer_hil_source_snapshot::Dependency;
use oer_process::git::Worktree;
use serde::Serialize;

use crate::{
    Result,
    job::Running,
    launch::{Launch, Runner, launch_run},
};

const REPORT_SCHEMA: u16 = 1;

/// A variant as written on the command line, before its revision and
/// checkouts are resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSpec {
    pub revision: String,
    pub overrides: Vec<(Dependency, PathBuf)>,
    /// Runtime features added to or removed from each image class's own.
    pub features: FeatureDelta,
}

impl std::str::FromStr for VariantSpec {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        let mut revision = None;
        let mut overrides = Vec::new();
        let mut features = None;
        for part in text
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            if let Some(value) = part.strip_prefix("rev=") {
                if revision.replace(value.to_owned()).is_some() {
                    return Err(format!("`{text}` names two revisions"));
                }
            } else if let Some(value) = part.strip_prefix("override:") {
                let (dependency, path) = value
                    .split_once('=')
                    .ok_or_else(|| format!("`{part}` is not override:<dependency>=<path>"))?;
                let dependency: Dependency = dependency.parse()?;
                if overrides.iter().any(|(known, _)| *known == dependency) {
                    return Err(format!("`{text}` overrides {} twice", dependency.id()));
                }
                overrides.push((dependency, PathBuf::from(path)));
            } else if let Some(value) = part.strip_prefix("features=") {
                if features.replace(value.parse::<FeatureDelta>()?).is_some() {
                    return Err(format!("`{text}` names features twice"));
                }
            } else {
                return Err(format!(
                    "`{part}` is neither rev=<revision>, override:<dependency>=<path> nor \
                     features=+f,-g"
                ));
            }
        }
        Ok(Self {
            revision: revision.unwrap_or_else(|| String::from("HEAD")),
            overrides,
            features: features.unwrap_or_default(),
        })
    }
}

/// What one experiment compares.
#[derive(Clone, Debug)]
pub struct Spec {
    pub a: VariantSpec,
    pub b: VariantSpec,
    pub scenarios: Vec<String>,
    /// Runs of each arm per layout seed.
    pub repetitions: u32,
    /// Layout seeds 1..=K, each built and run for both arms.
    pub layout_seeds: u32,
    /// The runner arguments naming the boards every run uses.
    pub boards: Vec<String>,
}

/// One run of an arm.
#[derive(Clone, Debug, Serialize)]
pub struct ArmRun {
    pub arm: Arm,
    pub seed: NonZeroU32,
    pub run: String,
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub schema: u16,
    pub id: String,
    pub a: Variant,
    pub b: Variant,
    pub scenarios: Vec<String>,
    pub repetitions: u32,
    pub layout_seeds: u32,
    pub runs: Vec<ArmRun>,
    pub comparisons: Vec<MeasurementComparison>,
}

/// A finished experiment: its report, where it is written, and every run
/// it created with its outcome.
pub struct Finished {
    pub report: Report,
    pub path: PathBuf,
    pub runs: Vec<(RunId, Option<Outcome>)>,
}

/// Run the experiment `spec` in the checkout at `checkout` with `runner`
/// for `owner`.
pub fn run(checkout: &Path, owner: &str, runner: &Runner, spec: &Spec) -> Result<Finished> {
    if spec.repetitions == 0 || spec.layout_seeds == 0 {
        return Err("--repetitions and --layout-seeds must be at least 1".into());
    }
    let id = oer_durable::unix_millis().to_string();
    let directory = checkout.join("target/hil/ab").join(&id);
    std::fs::create_dir_all(&directory)?;
    let current =
        crate::bisect::messages_lock(checkout).ok_or("this checkout names no HIL wire")?;
    let arms = [(Arm::A, &spec.a), (Arm::B, &spec.b)]
        .into_iter()
        .map(|(arm, variant)| prepare(checkout, &directory, arm, variant, &current))
        .collect::<Result<Vec<_>>>()?;
    let mut report = Report {
        schema: REPORT_SCHEMA,
        id: id.clone(),
        a: arms[0].variant.clone(),
        b: arms[1].variant.clone(),
        scenarios: spec.scenarios.clone(),
        repetitions: spec.repetitions,
        layout_seeds: spec.layout_seeds,
        runs: Vec::new(),
        comparisons: Vec::new(),
    };
    let path = directory.join("ab-report.json");
    let seeds = (1..=spec.layout_seeds)
        .filter_map(NonZeroU32::new)
        .collect::<Vec<_>>();
    let session = Session {
        checkout,
        owner,
        id: &id,
        directory: &directory,
        runner,
        boards: &spec.boards,
        store: RunStore::shared()?,
    };
    let mut loaded = Vec::new();
    // The first round of each seed builds each arm's images.
    let mut first = BTreeMap::new();
    for seed in &seeds {
        for arm in &arms {
            let run = arm.run(&session, &spec.scenarios, &Firmware::Build(*seed), None)?;
            first.insert((arm.arm, *seed), run.clone());
            record(
                &session.store,
                &mut report,
                &mut loaded,
                arm.arm,
                *seed,
                &run,
            )?;
            write_report(&path, &report)?;
        }
    }
    // The other rounds replay those images. One round, A then B of one
    // seed, holds the stand, so drift falls on both arms alike; between
    // rounds the lease is released and shorter work of owners with a higher
    // balance goes first. Every round leases the same work, so the arbiter
    // estimates the next round from the rounds before it.
    if spec.repetitions > 1 {
        let arbiter = oer_hil_arbiter::Arbiter::open()?;
        let work = round_work(&spec.scenarios);
        for _ in 1..spec.repetitions {
            for seed in &seeds {
                let grant = arbiter.acquire(&oer_hil_arbiter::Request {
                    owner: owner.to_owned(),
                    work: work.clone(),
                    scenarios: spec.scenarios.clone(),
                    claims: vec![oer_hil_arbiter::Claim::stand()],
                })?;
                for arm in &arms {
                    let built = &first[&(arm.arm, *seed)];
                    for (_, scenarios) in classes_of(&loaded, built) {
                        let run = arm.run(
                            &session,
                            &scenarios,
                            &Firmware::Replay(built.clone()),
                            Some(&grant),
                        )?;
                        record(
                            &session.store,
                            &mut report,
                            &mut loaded,
                            arm.arm,
                            *seed,
                            &run,
                        )?;
                        write_report(&path, &report)?;
                    }
                }
                drop(grant);
            }
        }
    }
    report.comparisons = arms::compare(
        &loaded
            .iter()
            .map(|(arm, run)| (*arm, run))
            .collect::<Vec<_>>(),
    );
    write_report(&path, &report)?;
    for arm in arms {
        if let Err(error) = arm.worktree.remove() {
            eprintln!("hil: the arm's worktree was not removed: {error}");
        }
    }
    Ok(Finished {
        runs: loaded
            .iter()
            .map(|(_, run)| (RunId::new(run.id()), run.outcome()))
            .collect(),
        report,
        path,
    })
}

/// The arbiter work of one replay round: the same for every round of an
/// experiment on these scenarios, so each round's estimate is the rounds'
/// own duration.
fn round_work(scenarios: &[String]) -> String {
    format!("ab round {}", scenarios.join(" "))
}

/// Add the run `id` of `arm` to the report and to the loaded runs.
fn record(
    store: &RunStore,
    report: &mut Report,
    loaded: &mut Vec<(Arm, Run)>,
    arm: Arm,
    seed: NonZeroU32,
    id: &RunId,
) -> Result<()> {
    let run = Run::open(store, id.as_str())?;
    report.runs.push(ArmRun {
        arm,
        seed,
        run: id.to_string(),
        outcome: run.outcome().map(|outcome| outcome.id().to_owned()),
    });
    loaded.push((arm, run));
    Ok(())
}

/// The scenarios of the run `id`, grouped by the image class each ran on:
/// a replay flashes one class.
fn classes_of(loaded: &[(Arm, Run)], id: &RunId) -> BTreeMap<String, Vec<String>> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some((_, run)) = loaded.iter().find(|(_, run)| run.id() == id.as_str()) {
        for scenario in run.scenarios() {
            classes
                .entry(scenario.image.id().to_owned())
                .or_default()
                .push(scenario.scenario.clone());
        }
    }
    classes
}

/// How a run gets its firmware.
#[derive(Clone, Debug)]
enum Firmware {
    /// Build from the arm's snapshot with this layout seed.
    Build(NonZeroU32),
    /// Replay the images an earlier run of the arm archived.
    Replay(RunId),
}

/// What every run of one experiment shares.
struct Session<'a> {
    checkout: &'a Path,
    owner: &'a str,
    id: &'a str,
    directory: &'a Path,
    /// The runner every round runs, fixed when the experiment started.
    runner: &'a Runner,
    boards: &'a [String],
    store: RunStore,
}

struct PreparedArm {
    arm: Arm,
    variant: Variant,
    worktree: Worktree,
    snapshot: PathBuf,
}

/// Check the arm's revision out, resolve its overrides and capture its
/// sources.
fn prepare(
    checkout: &Path,
    directory: &Path,
    arm: Arm,
    spec: &VariantSpec,
    current: &str,
) -> Result<PreparedArm> {
    let commit = oer_process::git::commit(checkout, &spec.revision)?;
    let worktree = Worktree::detached(checkout, &directory.join(format!("arm-{arm}")), &commit)?;
    if crate::bisect::messages_lock(worktree.path()).as_deref() != Some(current) {
        return Err(format!(
            "arm {arm} ({}) speaks another HIL wire than this checkout (its \
             hil/protocol/messages.lock differs); an A/B comparison runs both arms on this \
             checkout's runner",
            spec.revision
        )
        .into());
    }
    let mut overrides = Vec::new();
    for (dependency, path) in &spec.overrides {
        let path = path.canonicalize().map_err(|error| {
            format!(
                "override {} at {}: {error}",
                dependency.id(),
                path.display()
            )
        })?;
        overrides.push(DependencyOverride {
            dependency: *dependency,
            commit: oer_process::git::text(&path, ["rev-parse", "HEAD"])?,
            dirty: !oer_process::git::text(&path, ["status", "--porcelain"])?.is_empty(),
            path,
        });
    }
    let snapshot = oer_hil_source_snapshot::capture_with_overrides(
        worktree.path(),
        &[],
        false,
        &overrides
            .iter()
            .map(|entry| (entry.dependency, entry.path.clone()))
            .collect::<Vec<_>>(),
        &oer_hil_image::source_snapshot_store()?,
    )?;
    Ok(PreparedArm {
        arm,
        variant: Variant {
            commit,
            overrides,
            features: spec.features.clone(),
        },
        worktree,
        snapshot: snapshot.directory().to_owned(),
    })
}

impl PreparedArm {
    /// One run of this arm, a job of its own, under `grant` when the
    /// experiment holds the stand; the created run's id.
    fn run(
        &self,
        session: &Session<'_>,
        scenarios: &[String],
        firmware: &Firmware,
        grant: Option<&oer_hil_arbiter::Grant>,
    ) -> Result<RunId> {
        let mut arguments = vec![String::from("run")];
        arguments.extend(scenarios.iter().cloned());
        arguments.extend(session.boards.iter().cloned());
        match firmware {
            Firmware::Build(seed) => {
                arguments.push(String::from("--source-snapshot"));
                arguments.push(self.snapshot.to_string_lossy().into_owned());
                arguments.extend([String::from("--layout-seed"), seed.to_string()]);
                if !self.variant.features.is_empty() {
                    arguments.extend([
                        String::from("--features"),
                        self.variant.features.to_string(),
                    ]);
                }
            }
            Firmware::Replay(run) => {
                arguments.extend([String::from("--firmware-from"), run.to_string()]);
            }
        }
        let experiment = Experiment {
            id: session.id.to_owned(),
            arm: self.arm,
            variant: self.variant.clone(),
        };
        let arguments = arguments
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut job = Running::new(session.checkout, session.owner, &arguments)?;
        let mut launch = Launch::new(session.checkout, session.runner)
            .args(arguments)
            .env(oer_hil_arbiter::OWNER_ENV, session.owner)
            .env(
                oer_hil_run_bundle::experiment::EXPERIMENT_ENV,
                serde_json::to_string(&experiment)?,
            )
            .env(oer_hil_arbiter::jobs::JOB_ENV, job.id())
            .log(session.directory.join(format!(
                "run-{}-{}.log",
                self.arm,
                oer_durable::unix_millis()
            )));
        if let Some(grant) = grant {
            for (name, value) in grant.environment() {
                launch = launch.env(name, value);
            }
        }
        let launched = launch_run(&launch)?;
        job.finish_with(&session.store, &launched.runs)?;
        launched.run()
    }
}

pub fn summary(report: &Report) -> String {
    let mut text = format!(
        "ab {}: A {} vs B {}, {} runs\n",
        report.id,
        describe(&report.a),
        describe(&report.b),
        report.runs.len()
    );
    for entry in &report.comparisons {
        let c = &entry.comparison;
        text.push_str(&format!(
            "  {} {}: A {:.3} ±{:.3} (n={}), B {:.3} ±{:.3} (n={}) {}; B−A {:+.3} ±{:.3}: {}{}\n",
            entry.scenario,
            entry.measurement,
            c.a.mean,
            c.a.deviation,
            c.a.count,
            c.b.mean,
            c.b.deviation,
            c.b.count,
            entry.unit,
            c.difference,
            c.interval,
            c.verdict,
            if entry.better.is_none() {
                " (ungated)"
            } else {
                ""
            }
        ));
    }
    text
}

fn describe(variant: &Variant) -> String {
    let mut text = variant.commit[..12.min(variant.commit.len())].to_owned();
    for entry in &variant.overrides {
        text.push_str(&format!(
            " +{}@{}{}",
            entry.dependency.id(),
            &entry.commit[..12.min(entry.commit.len())],
            if entry.dirty { "+" } else { "" }
        ));
    }
    text
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    oer_durable::atomic_json(path, report)
}

#[cfg(test)]
mod tests;
