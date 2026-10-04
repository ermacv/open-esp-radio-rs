//! `cargo hil bisect`: find the first commit at which a scenario stops
//! passing.
//!
//! Every tested revision is checked out, detached, in the bisection's own
//! worktree below `target/hil/bisect/<id>/`. A revision whose HIL protocol
//! version is this checkout's runs on this checkout's runner and host code
//! with the revision's firmware, built from a source snapshot of the
//! worktree. A revision with another protocol version runs its own runner,
//! built in the worktree, inside this bisection's whole-stand lease: its
//! runner gets a private arbiter directory holding a copy of the stand file
//! (and of an older device registry), so it neither waits for nor disturbs
//! the shared arbiter, whose schema it may not read.
//!
//! A revision whose image does not build or does not link, or whose own
//! runner cannot run, is broken: it is neither good nor bad, and the search
//! steps around it. A step that could not judge the revision for another
//! reason (a blocked or broken run, an interrupted one, a quarantined board)
//! ends the bisection, since its later steps would meet the same stand. One
//! typed report, `report.json`, records every step and the result.

use std::{
    ffi::OsString,
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_schema::run::{FailureKind, Outcome};
use serde::{Deserialize, Serialize};

use crate::{Context, Result, runs};

/// The file naming a revision's HIL wire: revisions with equal locks speak
/// the same protocol.
const MESSAGES_LOCK: &str = "hil/protocol/messages.lock";
const REPORT_SCHEMA: u16 = 2;

#[derive(clap::Parser)]
#[command(name = "cargo hil bisect")]
pub(crate) struct BisectCli {
    /// A commit at which the scenario passes.
    #[arg(long)]
    good: String,
    /// A later commit, descending from GOOD, at which it does not.
    #[arg(long)]
    bad: String,
    #[arg(long)]
    scenario: String,
    /// Build every tested image with this code layout seed.
    #[arg(long, value_name = "SEED")]
    layout_seed: Option<NonZeroU32>,
    #[command(flatten)]
    boards: crate::jobs::BoardChoiceArgs,
}

/// What one tested revision showed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum Verdict {
    Good { run: String },
    Bad { run: String },
    Broken { why: Broken, run: Option<String> },
}

/// Why a revision could not be judged good or bad.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Broken {
    /// The image failed to compile.
    DoesNotBuild,
    /// The image compiled but did not link.
    DoesNotLink,
    /// The revision's own runner could not build or start a run.
    RunnerUnavailable,
}

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DoesNotBuild => "does not build",
            Self::DoesNotLink => "does not link",
            Self::RunnerUnavailable => "its runner cannot run",
        })
    }
}

/// Which runner judged a revision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "runner", rename_all = "kebab-case")]
pub enum Runner {
    /// This checkout's runner with the revision's firmware.
    Current,
    /// The revision's own runner, for its other wire.
    Revision,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Step {
    pub commit: String,
    pub subject: String,
    #[serde(flatten)]
    pub runner: Runner,
    #[serde(flatten)]
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum Conclusion {
    /// The first commit at which the scenario does not pass.
    FirstBad { commit: String },
    /// Broken revisions hide which of these is the first bad one.
    Ambiguous { candidates: Vec<String> },
    /// A step could not judge its revision; the search stopped.
    Aborted { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Report {
    pub schema: u16,
    pub scenario: String,
    pub layout_seed: Option<NonZeroU32>,
    pub good: String,
    pub bad: String,
    /// The commits between GOOD (excluded) and BAD (included), oldest first.
    pub commits: Vec<String>,
    pub steps: Vec<Step>,
    pub conclusion: Option<Conclusion>,
}

/// What a probe of one index showed, for the search.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Probe {
    Good,
    Bad,
    Broken,
}

/// Where the search ended, as indices into the commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Found {
    FirstBad(usize),
    Ambiguous(std::ops::RangeInclusive<usize>),
}

/// Binary search over `len` commits whose last is known bad and whose
/// predecessor (outside the list) is known good. A broken index is skipped:
/// the next probe is the untested index closest to the middle.
pub fn search(len: usize, mut probe: impl FnMut(usize) -> Result<Probe>) -> Result<Found> {
    if len == 0 {
        return Err("the bisection has no commits".into());
    }
    let mut good: Option<usize> = None;
    let mut bad = len - 1;
    let mut broken = std::collections::BTreeSet::new();
    loop {
        let low = good.map_or(0, |index| index + 1);
        let middle = (low + bad) / 2;
        let next = (low..bad)
            .filter(|index| !broken.contains(index))
            .min_by_key(|index| index.abs_diff(middle));
        let Some(index) = next else {
            return Ok(if (low..bad).any(|index| broken.contains(&index)) {
                Found::Ambiguous(low..=bad)
            } else {
                Found::FirstBad(bad)
            });
        };
        match probe(index)? {
            Probe::Good => good = Some(index),
            Probe::Bad => bad = index,
            Probe::Broken => {
                broken.insert(index);
            }
        }
    }
}

/// The HIL wire of the tree at `root`, or `None` for a revision older than
/// the keyed protocol, whose wire no lock names.
pub(crate) fn messages_lock(root: &Path) -> Option<String> {
    fs::read_to_string(root.join(MESSAGES_LOCK)).ok()
}

/// Whether an image build log shows a link failure rather than a compile
/// failure.
fn build_failure(log: &str) -> Broken {
    const LINK_ERRORS: [&str; 5] = [
        "error: linking with",
        "rust-lld: error:",
        "ld.lld: error:",
        "undefined symbol:",
        "relocation R_",
    ];
    if LINK_ERRORS.iter().any(|marker| log.contains(marker)) {
        Broken::DoesNotLink
    } else {
        Broken::DoesNotBuild
    }
}

/// The verdict of a finished run of the scenario, or why it judged nothing.
fn judge(run: &runs::Run) -> std::result::Result<Verdict, String> {
    let id = run.id.clone();
    match run.outcome {
        Some(Outcome::Passed) => return Ok(Verdict::Good { run: id }),
        Some(Outcome::Failed) => return Ok(Verdict::Bad { run: id }),
        _ => {}
    }
    let build = run.scenarios.iter().find_map(|scenario| {
        let failure = scenario.failure.as_ref().or_else(|| {
            scenario
                .repetitions
                .iter()
                .find_map(|repetition| repetition.failure.as_ref())
        })?;
        (failure.0 == FailureKind::ImageBuild).then(|| (scenario.image.clone(), failure.1.clone()))
    });
    match build {
        Some((image, message)) => {
            let log =
                fs::read_to_string(run.directory.join("firmware").join(image).join("build.log"))
                    .unwrap_or_default();
            Ok(Verdict::Broken {
                why: build_failure(&format!("{message}\n{log}")),
                run: Some(id),
            })
        }
        None => Err(format!(
            "run {id} ended {} without judging the revision",
            runs::status(run)
        )),
    }
}

pub fn run(ctx: &Context, owner: &str, args: &[OsString]) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let cli = BisectCli::try_parse_from(
        std::iter::once(OsString::from("cargo hil bisect")).chain(args.iter().cloned()),
    )?;
    let good = commit(&ctx.root, &cli.good)?;
    let bad = commit(&ctx.root, &cli.bad)?;
    if git(&ctx.root, &["merge-base", "--is-ancestor", &good, &bad]).is_err() {
        return Err(format!(
            "--good {} is not an ancestor of --bad {}",
            cli.good, cli.bad
        )
        .into());
    }
    let commits = git_text(
        &ctx.root,
        &[
            "rev-list",
            "--ancestry-path",
            "--reverse",
            &format!("{good}..{bad}"),
        ],
    )?
    .lines()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let current = messages_lock(&ctx.root).ok_or("this checkout names no HIL wire")?;
    let id = unix_millis().to_string();
    let directory = ctx.root.join("target/hil/bisect").join(&id);
    fs::create_dir_all(&directory)?;
    let mut report = Report {
        schema: REPORT_SCHEMA,
        scenario: cli.scenario.clone(),
        layout_seed: cli.layout_seed,
        good,
        bad,
        commits: commits.clone(),
        steps: Vec::new(),
        conclusion: None,
    };
    let path = directory.join("report.json");
    write_report(&path, &report)?;
    eprintln!(
        "hil: bisecting {} over {} commits; report {}",
        cli.scenario,
        commits.len(),
        path.display()
    );
    let bisection = Bisection {
        ctx,
        owner,
        scenario: &cli.scenario,
        layout_seed: cli.layout_seed,
        boards: &cli.boards,
        current,
        worktree: directory.join(format!("bisect-{id}")),
        arbiter: directory.join("arbiter"),
    };
    let found = search(commits.len(), |index| {
        let step = bisection.step(&commits[index])?;
        eprintln!("hil: bisect {}", step_line(&step));
        let probe = match step.verdict {
            Verdict::Good { .. } => Probe::Good,
            Verdict::Bad { .. } => Probe::Bad,
            Verdict::Broken { .. } => Probe::Broken,
        };
        report.steps.push(step);
        write_report(&path, &report)?;
        Ok(probe)
    });
    let _ = git(
        &ctx.root,
        &[
            "worktree",
            "remove",
            "--force",
            &bisection.worktree.to_string_lossy(),
        ],
    );
    let (conclusion, code) = match found {
        Ok(Found::FirstBad(index)) => (
            Conclusion::FirstBad {
                commit: commits[index].clone(),
            },
            0,
        ),
        Ok(Found::Ambiguous(range)) => (
            Conclusion::Ambiguous {
                candidates: commits[range].to_vec(),
            },
            1,
        ),
        Err(error) => (
            Conclusion::Aborted {
                reason: error.to_string(),
            },
            2,
        ),
    };
    report.conclusion = Some(conclusion);
    write_report(&path, &report)?;
    print!("{}", summary(&report));
    Ok(std::process::ExitCode::from(code))
}

struct Bisection<'a> {
    ctx: &'a Context,
    owner: &'a str,
    scenario: &'a str,
    layout_seed: Option<NonZeroU32>,
    /// The boards every run uses; a revision's own runner from before the
    /// stand file does not know these flags.
    boards: &'a crate::jobs::BoardChoiceArgs,
    current: String,
    worktree: PathBuf,
    arbiter: PathBuf,
}

impl Bisection<'_> {
    fn step(&self, commit: &str) -> Result<Step> {
        self.checkout(commit)?;
        let subject = git_text(&self.worktree, &["log", "-1", "--format=%s"])?;
        let (runner, verdict) = if messages_lock(&self.worktree).as_ref() == Some(&self.current) {
            (Runner::Current, self.with_current_runner()?)
        } else {
            (Runner::Revision, self.with_revision_runner()?)
        };
        Ok(Step {
            commit: commit.to_owned(),
            subject,
            runner,
            verdict,
        })
    }

    fn checkout(&self, commit: &str) -> Result<()> {
        if self.worktree.join(".git").exists() {
            git(
                &self.worktree,
                &["checkout", "--quiet", "--detach", "--force", commit],
            )?;
            git(&self.worktree, &["clean", "-fdq"])
        } else {
            git(&self.ctx.root, &["worktree", "prune"])?;
            git(
                &self.ctx.root,
                &[
                    "worktree",
                    "add",
                    "--detach",
                    "--force",
                    &self.worktree.to_string_lossy(),
                    commit,
                ],
            )
        }
    }

    /// The run's layout seed and chosen boards.
    fn seed_arguments(&self) -> Vec<String> {
        let mut arguments = self
            .layout_seed
            .map(|seed| vec![String::from("--layout-seed"), seed.to_string()])
            .unwrap_or_default();
        arguments.extend(self.boards.arguments());
        arguments
    }

    /// This checkout's runner, building the revision's firmware from a
    /// snapshot of the worktree.
    fn with_current_runner(&self) -> Result<Verdict> {
        let snapshot = oer_hil_source_snapshot::capture(
            &self.worktree,
            &[],
            false,
            &oer_hil_image::source_snapshot_store()?,
        )?;
        let receipt = tempfile::NamedTempFile::new()?;
        let status = self
            .ctx
            .cargo()
            .current_dir(&self.ctx.root)
            .args(["hil", "run", self.scenario, "--source-snapshot"])
            .arg(snapshot.directory())
            .args(self.seed_arguments())
            .env(oer_hil_arbiter::OWNER_ENV, self.owner)
            .env(RUN_RECEIPT_ENV, receipt.path())
            .status()?;
        let run = fs::read_to_string(receipt.path())?
            .lines()
            .last()
            .map(str::to_owned)
            .ok_or_else(|| format!("the runner created no run ({status})"))?;
        self.judge_run(&run)
    }

    /// The revision's own runner, inside a whole-stand lease of the shared
    /// arbiter, with a private arbiter holding a copy of the stand file.
    fn with_revision_runner(&self) -> Result<Verdict> {
        let arbiter = oer_hil_arbiter::Arbiter::open()?;
        fs::create_dir_all(&self.arbiter)?;
        // The revision's runner reads the boards from the stand file beside
        // its arbiter's state, or, before the stand file, from the arbiter's
        // device registry the shared directory may still hold.
        fs::copy(arbiter.stand_file(), self.arbiter.join("stand.toml"))?;
        let devices = arbiter.directory().join("devices.json");
        if devices.exists() {
            fs::copy(&devices, self.arbiter.join("devices.json"))?;
        }
        let started = unix_millis();
        let _grant = arbiter.acquire(&oer_hil_arbiter::Request {
            owner: self.owner.to_owned(),
            work: format!("bisect {} with the revision's runner", self.scenario),
            scenarios: vec![self.scenario.to_owned()],
            claims: vec![oer_hil_arbiter::Claim::stand()],
        })?;
        let status = self
            .ctx
            .cargo()
            .current_dir(&self.worktree)
            .args(["hil", "run", self.scenario])
            .args(self.seed_arguments())
            .env(oer_hil_arbiter::DIRECTORY_ENV, &self.arbiter)
            .env(oer_hil_arbiter::OWNER_ENV, self.owner)
            .env_remove(oer_hil_arbiter::LEASE_ENV)
            .status()?;
        let runs = crate::store::shared_runs()?;
        let checkout = self.worktree.file_name().and_then(|name| name.to_str());
        match runs::newest_of(&runs, checkout).filter(|run| run.started_millis >= started) {
            Some(run) => self.judge_run(&run.id),
            None => {
                eprintln!("hil: the revision's runner created no run ({status})");
                Ok(Verdict::Broken {
                    why: Broken::RunnerUnavailable,
                    run: None,
                })
            }
        }
    }

    fn judge_run(&self, id: &str) -> Result<Verdict> {
        let runs = crate::store::shared_runs()?;
        let run = runs::load(&runs.join(id)).ok_or_else(|| format!("run {id} is unreadable"))?;
        Ok(judge(&run)?)
    }
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// The runner's run receipt variable; see oer-hil-evidence.
const RUN_RECEIPT_ENV: &str = "OER_HIL_RUN_RECEIPT";

fn step_line(step: &Step) -> String {
    let short = &step.commit[..12.min(step.commit.len())];
    let runner = match step.runner {
        Runner::Current => String::new(),
        Runner::Revision => String::from(" [own runner]"),
    };
    let verdict = match &step.verdict {
        Verdict::Good { run } => format!("good (run {run})"),
        Verdict::Bad { run } => format!("bad (run {run})"),
        Verdict::Broken { why, run } => match run {
            Some(run) => format!("broken: {why} (run {run})"),
            None => format!("broken: {why}"),
        },
    };
    format!("{short} {verdict}{runner}  {}", step.subject)
}

pub fn summary(report: &Report) -> String {
    let mut text = format!("bisect {}:\n", report.scenario);
    for step in &report.steps {
        text.push_str(&format!("  {}\n", step_line(step)));
    }
    match &report.conclusion {
        Some(Conclusion::FirstBad { commit }) => {
            text.push_str(&format!("first bad commit: {commit}\n"));
        }
        Some(Conclusion::Ambiguous { candidates }) => {
            text.push_str("broken revisions hide the first bad commit among:\n");
            for commit in candidates {
                text.push_str(&format!("  {commit}\n"));
            }
        }
        Some(Conclusion::Aborted { reason }) => {
            text.push_str(&format!("bisection stopped: {reason}\n"));
        }
        None => text.push_str("bisection unfinished\n"),
    }
    text
}

fn write_report(path: &Path, report: &Report) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(report)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn commit(root: &Path, revision: &str) -> Result<String> {
    git_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("{revision} names no commit").into())
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
