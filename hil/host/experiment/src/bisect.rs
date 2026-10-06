//! Bisection: find the first commit at which a scenario stops passing.
//!
//! Every tested revision is checked out, detached, in the bisection's own
//! worktree below `target/hil/bisect/<id>/`. A revision whose HIL protocol
//! version is this checkout's runs on this checkout's runner and host code
//! with the revision's firmware, built from a source snapshot of the
//! worktree. A revision with another protocol version runs its own runner,
//! built in the worktree, inside this bisection's whole-stand lease: its
//! runner gets a private arbiter directory holding a copy of the stand file
//! (and of an older device registry), so it neither waits for nor disturbs
//! the shared arbiter, whose schema it may not read, and the worktree's run
//! directory links to the shared store. Every step is launched through
//! [`crate::launch::launch_run`], and its run is the one its receipt names.
//!
//! A revision whose image does not build or does not link, or whose own
//! runner cannot run, is broken: it is neither good nor bad, and the search
//! steps around it. A step that could not judge the revision for another
//! reason (a blocked or broken run, an interrupted one, a quarantined board)
//! ends the bisection, since its later steps would meet the same stand. One
//! typed report, `report.json`, records every step and the result.

use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use oer_hil_analysis::Run;
use oer_hil_run_bundle::RunStore;
use oer_hil_schema::run::{FailureKind, Outcome};
use oer_process::git::Worktree;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    job::Running,
    launch::{self, Launch, launch_run},
};

/// The file naming a revision's HIL wire: revisions with equal locks speak
/// the same protocol.
const MESSAGES_LOCK: &str = "hil/protocol/messages.lock";
const REPORT_SCHEMA: u16 = 2;

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

/// The verdict of a finished run of the bisected scenario, or why it judged
/// nothing: the scenario's own outcome decides, and a scenario whose image
/// did not build is broken, told from a link failure by its build log.
fn judge(run: &Run) -> std::result::Result<Verdict, String> {
    let id = run.id().to_owned();
    let Some(scenario) = run.scenarios().first() else {
        return Err(format!(
            "run {id} ended {} without judging the revision",
            run.status()
        ));
    };
    match scenario.outcome {
        Outcome::Passed => return Ok(Verdict::Good { run: id }),
        Outcome::Failed => return Ok(Verdict::Bad { run: id }),
        _ => {}
    }
    let build = scenario
        .failure
        .as_ref()
        .or_else(|| {
            scenario
                .repetitions
                .iter()
                .find_map(|repetition| repetition.failure.as_ref())
        })
        .filter(|failure| failure.kind == FailureKind::ImageBuild);
    match build {
        Some(failure) => {
            let log = fs::read_to_string(
                run.directory()
                    .join("firmware")
                    .join(scenario.image.id())
                    .join("build.log"),
            )
            .unwrap_or_default();
            Ok(Verdict::Broken {
                why: build_failure(&format!("{}\n{log}", failure.message)),
                run: Some(id),
            })
        }
        None => Err(format!(
            "run {id} ended {} without judging the revision",
            scenario.outcome
        )),
    }
}

/// What one bisection searches.
#[derive(Clone, Debug)]
pub struct Spec {
    /// A commit at which the scenario passes.
    pub good: String,
    /// A later commit, descending from `good`, at which it does not.
    pub bad: String,
    pub scenario: String,
    /// Build every tested image with this code layout seed.
    pub layout_seed: Option<NonZeroU32>,
    /// The runner arguments naming the boards every run uses; a revision's
    /// own runner from before the stand file does not know them.
    pub boards: Vec<String>,
}

/// A finished bisection: its report, where it is written, and the exit
/// status its conclusion gives: 0 first bad commit found, 1 ambiguous, 2
/// aborted.
pub struct Finished {
    pub report: Report,
    pub path: PathBuf,
    pub code: u8,
}

/// Bisect `spec` in the checkout at `checkout` for `owner`, reporting each
/// step as it ends.
pub fn run(
    checkout: &Path,
    owner: &str,
    spec: &Spec,
    mut progress: impl FnMut(&str),
) -> Result<Finished> {
    let good = oer_process::git::commit(checkout, &spec.good)?;
    let bad = oer_process::git::commit(checkout, &spec.bad)?;
    if oer_process::git::run(checkout, ["merge-base", "--is-ancestor", &good, &bad]).is_err() {
        return Err(format!(
            "--good {} is not an ancestor of --bad {}",
            spec.good, spec.bad
        )
        .into());
    }
    let commits = oer_process::git::lines(
        checkout,
        [
            "rev-list",
            "--ancestry-path",
            "--reverse",
            &format!("{good}..{bad}"),
        ],
    )?;
    let current = messages_lock(checkout).ok_or("this checkout names no HIL wire")?;
    let id = oer_durable::unix_millis().to_string();
    let directory = checkout.join("target/hil/bisect").join(&id);
    fs::create_dir_all(&directory)?;
    let mut report = Report {
        schema: REPORT_SCHEMA,
        scenario: spec.scenario.clone(),
        layout_seed: spec.layout_seed,
        good,
        bad,
        commits: commits.clone(),
        steps: Vec::new(),
        conclusion: None,
    };
    let path = directory.join("report.json");
    write_report(&path, &report)?;
    progress(&format!(
        "bisecting {} over {} commits; report {}",
        spec.scenario,
        commits.len(),
        path.display()
    ));
    let bisection = Bisection {
        checkout,
        owner,
        spec,
        current,
        runner: launch::Runner::prepare(checkout)?,
        store: RunStore::shared()?,
        worktree: directory.join(format!("bisect-{id}")),
        arbiter: directory.join("arbiter"),
    };
    let mut worktree = None;
    let found = search(commits.len(), |index| {
        let step = bisection.step(&mut worktree, &commits[index])?;
        progress(&format!("bisect {}", step_line(&step)));
        let probe = match step.verdict {
            Verdict::Good { .. } => Probe::Good,
            Verdict::Bad { .. } => Probe::Bad,
            Verdict::Broken { .. } => Probe::Broken,
        };
        report.steps.push(step);
        write_report(&path, &report)?;
        Ok(probe)
    });
    if let Some(worktree) = worktree
        && let Err(error) = worktree.remove()
    {
        progress(&format!(
            "the bisection's worktree was not removed: {error}"
        ));
    }
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
    Ok(Finished { report, path, code })
}

struct Bisection<'a> {
    checkout: &'a Path,
    owner: &'a str,
    spec: &'a Spec,
    current: String,
    /// This checkout's runner, prepared once for every step it judges.
    runner: launch::Runner,
    store: RunStore,
    worktree: PathBuf,
    arbiter: PathBuf,
}

impl Bisection<'_> {
    fn step(&self, worktree: &mut Option<Worktree>, commit: &str) -> Result<Step> {
        match worktree {
            Some(worktree) => worktree.checkout(commit)?,
            None => *worktree = Some(Worktree::detached(self.checkout, &self.worktree, commit)?),
        }
        let subject = oer_process::git::text(&self.worktree, ["log", "-1", "--format=%s"])?;
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

    /// The run's layout seed and chosen boards.
    fn seed_arguments(&self) -> Vec<String> {
        let mut arguments = self
            .spec
            .layout_seed
            .map(|seed| vec![String::from("--layout-seed"), seed.to_string()])
            .unwrap_or_default();
        arguments.extend(self.spec.boards.iter().cloned());
        arguments
    }

    /// This checkout's runner, building the revision's firmware from a
    /// snapshot of the worktree, as a job of its own.
    fn with_current_runner(&self) -> Result<Verdict> {
        let snapshot = oer_hil_source_snapshot::capture(
            &self.worktree,
            &[],
            false,
            &oer_hil_image::source_snapshot_store()?,
        )?;
        let mut arguments = vec![
            String::from("run"),
            self.spec.scenario.clone(),
            String::from("--source-snapshot"),
            snapshot.directory().to_string_lossy().into_owned(),
        ];
        arguments.extend(self.seed_arguments());
        let arguments = arguments
            .into_iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>();
        let mut job = Running::new(self.checkout, self.owner, &arguments)?;
        let launched = launch_run(
            &Launch::new(self.checkout, &self.runner)
                .args(arguments)
                .env(oer_stand_owners::OWNER_ENV, self.owner)
                .context(
                    oer_process::Context::current()?
                        .clone()
                        .with(oer_stand_owners::OWNER_KEY, self.owner)
                        .with(oer_stand_arbiter::jobs::JOB_KEY, job.id()),
                ),
        )?;
        job.finish_with(&self.store, &launched.runs)?;
        self.judge_run(&launched.run()?.to_string())
    }

    /// The revision's own runner, inside a whole-stand lease of the shared
    /// arbiter, with a private arbiter holding a copy of the stand file.
    fn with_revision_runner(&self) -> Result<Verdict> {
        let arbiter = oer_stand_arbiter::Arbiter::open()?;
        fs::create_dir_all(&self.arbiter)?;
        // A revision's runner reads the user's stand file; a revision before
        // the stand model read it beside its arbiter's state, and one before
        // the stand file the arbiter's device registry the shared directory
        // may still hold: the private arbiter gets copies of both.
        fs::copy(arbiter.stand_file(), self.arbiter.join("stand.toml"))?;
        let devices = arbiter.directory().join("devices.json");
        if devices.exists() {
            fs::copy(&devices, self.arbiter.join("devices.json"))?;
        }
        let _grant = arbiter.acquire(&oer_stand_arbiter::Request {
            owner: self.owner.to_owned(),
            work: format!("bisect {} with the revision's runner", self.spec.scenario),
            scenarios: vec![self.spec.scenario.clone()],
            claims: vec![oer_stand_claims::Claim::stand()],
        })?;
        // The revision's runner records its runs in the shared store, and
        // runs without an observer receipt: its receipt format is the
        // revision's own.
        let runner = match launch::Runner::prepare(&self.worktree)
            .and_then(|runner| Ok((runner, self.store.link(&self.worktree)?)))
        {
            Ok((runner, _)) => launch::Runner {
                receipt: None,
                ..runner
            },
            Err(error) => {
                eprintln!("hil: the revision's runner does not build: {error}");
                return Ok(Verdict::Broken {
                    why: Broken::RunnerUnavailable,
                    run: None,
                });
            }
        };
        let mut arguments = vec![String::from("run"), self.spec.scenario.clone()];
        arguments.extend(self.seed_arguments());
        let launched = launch_run(
            &Launch::new(&self.worktree, &runner)
                .args(arguments)
                .env(oer_stand_file::paths::ARBITER_ENV, &self.arbiter)
                .env(oer_stand_owners::OWNER_ENV, self.owner)
                .context(
                    oer_process::Context::default().with(oer_stand_owners::OWNER_KEY, self.owner),
                ),
        )?;
        match launched.runs.last() {
            Some(run) => self.judge_run(run.as_str()),
            None => {
                eprintln!(
                    "hil: the revision's runner created no run ({})",
                    launched.status
                );
                Ok(Verdict::Broken {
                    why: Broken::RunnerUnavailable,
                    run: None,
                })
            }
        }
    }

    fn judge_run(&self, id: &str) -> Result<Verdict> {
        Ok(judge(&Run::open(&self.store, id)?)?)
    }
}

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
    oer_durable::atomic_json(path, report)
}

#[cfg(test)]
mod tests;
