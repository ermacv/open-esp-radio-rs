//! `cargo hil ab`: compare two variants of the firmware on the same
//! scenarios, with enough repetitions to tell an effect from noise.
//!
//! A variant is a repository revision plus local checkouts that replace its
//! pinned dependencies (the `ESP_HAL_ROOT`, `EMBASSY_ROOT` and
//! `OPEN_RADIO_XARXA_ROOT` roots). Each arm's revision is checked out in the
//! experiment's worktree below `target/hil/ab/<id>/` and captured with its
//! overrides into a source snapshot, which this checkout's runner builds:
//! both arms therefore need this checkout's HIL protocol version.
//!
//! For every layout seed, the first round runs A, then B, each building its
//! images. The remaining rounds replay those exact images, alternating A and
//! B under one whole-stand lease, so drift of the air and the calibrations
//! falls on both arms alike. Every run records its arm and variant in its
//! manifest ([`oer_hil_runner_core::experiment`]) and no evidence. The
//! report compares, per scenario and measurement, the arms' run means with
//! [`crate::hil_perf::compare`] and is written to `ab-report.json`.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_runner_core::experiment::{Arm, Dependency, DependencyOverride, Experiment, Variant};
use serde::Serialize;

use crate::{
    Context, Result,
    hil_perf::{self, AbComparison, Better},
    hil_runs,
};

const REPORT_SCHEMA: u16 = 1;

#[derive(clap::Parser)]
#[command(name = "cargo hil ab")]
pub(crate) struct AbCli {
    /// Variant A: `rev=<revision>`, then `;override:<esp-hal|embassy|xarxa>=<path>`
    /// for each replaced dependency; `rev=` defaults to HEAD.
    #[arg(long)]
    a: String,
    /// Variant B, in the same form.
    #[arg(long)]
    b: String,
    #[arg(long = "scenario", required = true)]
    scenarios: Vec<String>,
    /// Runs of each arm per layout seed.
    #[arg(long, default_value_t = 3)]
    repetitions: u32,
    /// Layout seeds 1..=K, each built and run for both arms.
    #[arg(long, default_value_t = 1)]
    layout_seeds: u32,
}

/// A variant as written on the command line, before its revision and
/// checkouts are resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VariantSpec {
    pub revision: String,
    pub overrides: Vec<(Dependency, PathBuf)>,
}

impl std::str::FromStr for VariantSpec {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        let mut revision = None;
        let mut overrides = Vec::new();
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
            } else {
                return Err(format!(
                    "`{part}` is neither rev=<revision> nor override:<dependency>=<path>"
                ));
            }
        }
        Ok(Self {
            revision: revision.unwrap_or_else(|| String::from("HEAD")),
            overrides,
        })
    }
}

/// One run of an arm.
#[derive(Clone, Debug, Serialize)]
pub struct ArmRun {
    pub arm: Arm,
    pub seed: NonZeroU32,
    pub run: String,
    pub outcome: Option<String>,
}

/// One compared measurement.
#[derive(Clone, Debug, Serialize)]
pub struct MeasurementComparison {
    pub scenario: String,
    pub measurement: String,
    pub unit: String,
    /// Which way the measurement's gate prefers; `None` for an ungated
    /// figure, whose significant differences are reported as changes.
    pub better: Option<Better>,
    pub comparison: AbComparison,
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

/// Per (scenario, measurement): its unit, its gate's direction and each
/// arm's values.
type Samples = BTreeMap<(String, String), (String, Option<Better>, Vec<f64>, Vec<f64>)>;

/// Each arm's run means of every scenario's measurements: one value per run,
/// the mean over the run's repetitions that measured it.
fn samples(runs: &[(Arm, hil_runs::Run)]) -> Samples {
    let mut samples = BTreeMap::new();
    for (arm, run) in runs {
        for scenario in &run.scenarios {
            let mut per_measurement: BTreeMap<String, (String, Option<Better>, Vec<f64>)> =
                BTreeMap::new();
            for repetition in &scenario.repetitions {
                for measurement in &repetition.measurements {
                    let Some(value) = measurement.value else {
                        continue;
                    };
                    let entry = per_measurement
                        .entry(measurement.name.clone())
                        .or_insert_with(|| {
                            (
                                measurement.unit.to_string(),
                                measurement
                                    .threshold
                                    .as_ref()
                                    .and_then(Better::from_threshold)
                                    .map(|(better, _)| better),
                                Vec::new(),
                            )
                        });
                    entry.2.push(value);
                }
            }
            for (name, (unit, better, values)) in per_measurement {
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                let entry = samples
                    .entry((scenario.id.clone(), name))
                    .or_insert_with(|| (unit, better, Vec::new(), Vec::new()));
                match arm {
                    Arm::A => entry.2.push(mean),
                    Arm::B => entry.3.push(mean),
                }
            }
        }
    }
    samples
}

/// The comparison of every measurement both arms measured.
pub fn compare(runs: &[(Arm, hil_runs::Run)]) -> Vec<MeasurementComparison> {
    samples(runs)
        .into_iter()
        .filter_map(|((scenario, measurement), (unit, better, a, b))| {
            let comparison = hil_perf::compare(better, &a, &b)?;
            Some(MeasurementComparison {
                scenario,
                measurement,
                unit,
                better,
                comparison,
            })
        })
        .collect()
}

pub(crate) fn run(ctx: &Context, owner: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = AbCli::try_parse_from(
        std::iter::once(OsString::from("cargo hil ab")).chain(args.iter().cloned()),
    )?;
    if cli.repetitions == 0 || cli.layout_seeds == 0 {
        return Err("--repetitions and --layout-seeds must be at least 1".into());
    }
    let id = unix_millis().to_string();
    let directory = ctx.root.join("target/hil/ab").join(&id);
    fs::create_dir_all(&directory)?;
    let current = crate::hil_bisect::protocol_version(&fs::read_to_string(
        ctx.root.join(crate::hil_bisect::PROTOCOL_SOURCE),
    )?)
    .ok_or("this checkout's HIL protocol version is unreadable")?;
    let arms = [
        (Arm::A, cli.a.parse::<VariantSpec>()?),
        (Arm::B, cli.b.parse::<VariantSpec>()?),
    ]
    .into_iter()
    .map(|(arm, spec)| prepare(ctx, &directory, arm, &spec, current))
    .collect::<Result<Vec<_>>>()?;
    let mut report = Report {
        schema: REPORT_SCHEMA,
        id: id.clone(),
        a: arms[0].variant.clone(),
        b: arms[1].variant.clone(),
        scenarios: cli.scenarios.clone(),
        repetitions: cli.repetitions,
        layout_seeds: cli.layout_seeds,
        runs: Vec::new(),
        comparisons: Vec::new(),
    };
    let path = directory.join("ab-report.json");
    let seeds = (1..=cli.layout_seeds)
        .filter_map(NonZeroU32::new)
        .collect::<Vec<_>>();
    let session = Session {
        ctx,
        owner,
        id: &id,
        directory: &directory,
    };
    let mut loaded = Vec::new();
    // The first round of each seed builds each arm's images.
    let mut first = BTreeMap::new();
    for seed in &seeds {
        for arm in &arms {
            let run = arm.run(&session, &cli.scenarios, Launch::Build(*seed))?;
            first.insert((arm.arm, *seed), run.clone());
            record(&mut report, &mut loaded, arm.arm, *seed, run)?;
            write_report(&path, &report)?;
        }
    }
    // The other rounds replay those images, alternating under one lease.
    if cli.repetitions > 1 {
        let arbiter = oer_hil_arbiter::Arbiter::open()?;
        let grant = arbiter.acquire(&oer_hil_arbiter::Request {
            owner: owner.to_owned(),
            work: format!(
                "ab {} rounds 2..={}",
                cli.scenarios.join(" "),
                cli.repetitions
            ),
            scenarios: cli.scenarios.clone(),
            claims: vec![oer_hil_arbiter::Claim::stand()],
        })?;
        for _ in 1..cli.repetitions {
            for seed in &seeds {
                for arm in &arms {
                    let built = &first[&(arm.arm, *seed)];
                    let classes = classes_of(&loaded, built);
                    for (_, scenarios) in classes {
                        let run = arm.run_leased(
                            &session,
                            &scenarios,
                            Launch::Replay(built.clone()),
                            &grant,
                        )?;
                        record(&mut report, &mut loaded, arm.arm, *seed, run)?;
                        write_report(&path, &report)?;
                    }
                }
            }
        }
    }
    report.comparisons = compare(
        &loaded
            .iter()
            .map(|(arm, run): &(Arm, hil_runs::Run)| (*arm, run.clone()))
            .collect::<Vec<_>>(),
    );
    write_report(&path, &report)?;
    for arm in &arms {
        let _ = git(
            &ctx.root,
            &[
                "worktree",
                "remove",
                "--force",
                &arm.worktree.to_string_lossy(),
            ],
        );
    }
    print!("{}", summary(&report));
    println!("report: {}", path.display());
    Ok(std::process::ExitCode::SUCCESS)
}

/// Add the run `id` of `arm` to the report and to the loaded runs.
fn record(
    report: &mut Report,
    loaded: &mut Vec<(Arm, hil_runs::Run)>,
    arm: Arm,
    seed: NonZeroU32,
    id: String,
) -> Result<()> {
    let runs = crate::hil_store::shared_runs(crate::hil::HIL_TARGET)?;
    let run = hil_runs::load(&runs.join(&id)).ok_or_else(|| format!("run {id} is unreadable"))?;
    report.runs.push(ArmRun {
        arm,
        seed,
        run: id,
        outcome: run.outcome.map(|outcome| outcome.id().to_owned()),
    });
    loaded.push((arm, run));
    Ok(())
}

/// The scenarios of the run `id`, grouped by the image class each ran on:
/// a replay flashes one class.
fn classes_of(loaded: &[(Arm, hil_runs::Run)], id: &str) -> BTreeMap<String, Vec<String>> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some((_, run)) = loaded.iter().find(|(_, run)| run.id == id) {
        for scenario in &run.scenarios {
            classes
                .entry(scenario.image.clone())
                .or_default()
                .push(scenario.id.clone());
        }
    }
    classes
}

/// How a run gets its firmware.
#[derive(Clone, Debug)]
enum Launch {
    /// Build from the arm's snapshot with this layout seed.
    Build(NonZeroU32),
    /// Replay the images an earlier run of the arm archived.
    Replay(String),
}

/// What every run of one experiment shares.
struct Session<'a> {
    ctx: &'a Context,
    owner: &'a str,
    id: &'a str,
    directory: &'a Path,
}

struct PreparedArm {
    arm: Arm,
    variant: Variant,
    worktree: PathBuf,
    snapshot: PathBuf,
}

/// Check the arm's revision out, resolve its overrides and capture its
/// sources.
fn prepare(
    ctx: &Context,
    directory: &Path,
    arm: Arm,
    spec: &VariantSpec,
    current: u16,
) -> Result<PreparedArm> {
    let commit = git_text(
        &ctx.root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{}^{{commit}}", spec.revision),
        ],
    )
    .map_err(|_| format!("{} names no commit", spec.revision))?;
    let worktree = directory.join(format!("arm-{arm}"));
    git(
        &ctx.root,
        &[
            "worktree",
            "add",
            "--detach",
            "--force",
            &worktree.to_string_lossy(),
            &commit,
        ],
    )?;
    let version = fs::read_to_string(worktree.join(crate::hil_bisect::PROTOCOL_SOURCE))
        .ok()
        .and_then(|source| crate::hil_bisect::protocol_version(&source));
    if version != Some(current) {
        return Err(format!(
            "arm {arm} ({}) speaks HIL protocol {}, this checkout {current}; an A/B comparison \
             runs both arms on this checkout's runner",
            spec.revision,
            version.map_or_else(|| String::from("unknown"), |version| version.to_string())
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
            commit: git_text(&path, &["rev-parse", "HEAD"])?,
            dirty: !git_text(&path, &["status", "--porcelain"])?.is_empty(),
            path,
        });
    }
    let snapshot = oer_hil_runner_core::image::snapshot::capture_with_overrides(
        &worktree,
        &[],
        false,
        &overrides
            .iter()
            .map(|entry| (entry.dependency, entry.path.clone()))
            .collect::<Vec<_>>(),
    )?;
    Ok(PreparedArm {
        arm,
        variant: Variant { commit, overrides },
        worktree,
        snapshot: snapshot.directory().to_owned(),
    })
}

impl PreparedArm {
    fn command(
        &self,
        session: &Session<'_>,
        scenarios: &[String],
        launch: &Launch,
        receipt: &Path,
    ) -> Result<std::process::Command> {
        let Session {
            ctx,
            owner,
            id,
            directory,
        } = *session;
        let mut command = ctx.cargo();
        command
            .current_dir(&ctx.root)
            .args(["hil", "run"])
            .args(scenarios);
        match launch {
            Launch::Build(seed) => {
                command
                    .arg("--source-snapshot")
                    .arg(&self.snapshot)
                    .args(["--layout-seed", &seed.to_string()]);
            }
            Launch::Replay(run) => {
                command.args(["--firmware-from", run]);
            }
        }
        let experiment = Experiment {
            id: id.to_owned(),
            arm: self.arm,
            variant: self.variant.clone(),
        };
        let log =
            fs::File::create(directory.join(format!("run-{}-{}.log", self.arm, unix_millis())))?;
        command
            .env(oer_hil_arbiter::OWNER_ENV, owner)
            .env(
                oer_hil_runner_core::experiment::EXPERIMENT_ENV,
                serde_json::to_string(&experiment)?,
            )
            .env(RUN_RECEIPT_ENV, receipt)
            .stdout(log.try_clone()?)
            .stderr(log);
        Ok(command)
    }

    /// One run of this arm under its own lease; the created run's id.
    fn run(&self, session: &Session<'_>, scenarios: &[String], launch: Launch) -> Result<String> {
        let receipt = tempfile::NamedTempFile::new()?;
        let status = self
            .command(session, scenarios, &launch, receipt.path())?
            .status()?;
        created_run(receipt.path(), status)
    }

    /// One run of this arm inside the experiment's lease.
    fn run_leased(
        &self,
        session: &Session<'_>,
        scenarios: &[String],
        launch: Launch,
        grant: &oer_hil_arbiter::Grant,
    ) -> Result<String> {
        let receipt = tempfile::NamedTempFile::new()?;
        let status = self
            .command(session, scenarios, &launch, receipt.path())?
            .envs(grant.environment())
            .status()?;
        created_run(receipt.path(), status)
    }
}

fn created_run(receipt: &Path, status: std::process::ExitStatus) -> Result<String> {
    fs::read_to_string(receipt)?
        .lines()
        .last()
        .map(str::to_owned)
        .ok_or_else(|| format!("the runner created no run ({status})").into())
}

/// The runner's run receipt variable; see oer-hil-runner-core.
const RUN_RECEIPT_ENV: &str = "OER_HIL_RUN_RECEIPT";

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
        let verdict = match c.verdict {
            hil_perf::AbVerdict::Significant { better } => {
                format!("significant, {better:?} better")
            }
            hil_perf::AbVerdict::Changed { higher } => {
                format!("significant change, {higher:?} higher (ungated)")
            }
            hil_perf::AbVerdict::WithinNoise => String::from("within noise"),
            hil_perf::AbVerdict::InsufficientRepetitions => {
                String::from("insufficient repetitions")
            }
        };
        text.push_str(&format!(
            "  {} {}: A {:.3} ±{:.3} (n={}), B {:.3} ±{:.3} (n={}) {}; B−A {:+.3} ±{:.3}: {verdict}{}\n",
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
            if entry.better.is_none() { " (ungated)" } else { "" }
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
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(report)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()?;
    if !status.success() {
        return Err(format!("git {} failed in {}", args.join(" "), root.display()).into());
    }
    Ok(())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!("git {} failed", args.join(" ")).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests;
