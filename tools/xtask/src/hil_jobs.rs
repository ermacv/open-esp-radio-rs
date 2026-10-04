//! Enqueued HIL runs: `cargo hil run … --enqueue [--after JOB]`, `cargo hil
//! wait JOB`, and `--after JOB` in the foreground.
//!
//! An enqueued run is a job: a detached `cargo hil` process that runs the same
//! command, recorded in the arbiter directory's `jobs/<id>.json`. The command
//! prints the job's id and returns at once. The job itself moves the record
//! through its states (waiting for the job it runs after, started, finished
//! with the typed outcome of its runs), so `wait` and `queue` read one file
//! instead of guessing from process names. A job whose process is gone
//! without finishing is abandoned.

use std::{ffi::OsString, fs, path::PathBuf, time::Duration};

use oer_hil_schema::run::Outcome;
use serde::{Deserialize, Serialize};

use crate::{Context, Result};

/// Names the job a `cargo hil` process runs as.
pub const JOB_ENV: &str = "OER_HIL_JOB";
/// The xtask, runner and runner receipt a job runs with, fixed when it was
/// enqueued (or, for an experiment, when it started).
pub const FROZEN_XTASK_ENV: &str = "OER_HIL_JOB_XTASK";
pub const FROZEN_RUNNER_ENV: &str = "OER_HIL_JOB_RUNNER";
pub const FROZEN_RECEIPT_ENV: &str = "OER_HIL_JOB_RECEIPT";
/// The source snapshot an enqueued `run` builds from, captured when it was
/// enqueued.
pub const FROZEN_SNAPSHOT_ENV: &str = "OER_HIL_JOB_SNAPSHOT";
pub const ENQUEUE: &str = "--enqueue";
pub const AFTER: &str = "--after";
pub const AFTER_ANY: &str = "--after-any";
/// How long a job's record is kept.
const RECORD_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// How often a waiting process reads the job it waits for.
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Job {
    pub id: String,
    pub owner: String,
    /// The `cargo hil` arguments the job runs.
    pub command: Vec<String>,
    pub checkout: PathBuf,
    /// The job this one starts after.
    pub after: Option<String>,
    /// Whether it starts after that job whatever its outcome.
    #[serde(default)]
    pub after_any: bool,
    pub enqueued_unix_millis: u64,
    /// The detached process and its start time, which survive PID reuse.
    pub pid: Option<u32>,
    pub pid_started_unix_millis: Option<u64>,
    /// Where a detached job's output goes; a run in the foreground has none.
    pub log: Option<PathBuf>,
    /// The files the job was fixed with when it was enqueued; a sweep keeps
    /// them while the job has not ended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<PathBuf>,
    #[serde(flatten)]
    pub state: JobState,
}

/// Where an unfinished job is, from the arbiter's view of its process.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Waiting for the job it runs after.
    WaitingForJob,
    /// Building its images; it has not asked for the stand yet.
    Building,
    /// Queued in the arbiter.
    WaitingForStand,
    /// Holding its lease.
    Holding,
    Finished,
}

impl Phase {
    pub const fn describe(self) -> &'static str {
        match self {
            Self::WaitingForJob => "waits for its job",
            Self::Building => "building images",
            Self::WaitingForStand => "waiting for the stand",
            Self::Holding => "holding the stand",
            Self::Finished => "finished",
        }
    }
}

/// The phase of `job` given the arbiter's holders and queue: a holder or a
/// queued request made by the job's process, or by a child of it (the
/// runner), is that job's.
pub fn phase(job: &Job, status: &oer_hil_arbiter::Status) -> Phase {
    phase_of(
        job,
        &status
            .holders
            .iter()
            .map(|holder| holder.pid)
            .collect::<Vec<_>>(),
        &status
            .queue
            .iter()
            .map(|queued| queued.pid)
            .collect::<Vec<_>>(),
        parent_pid,
    )
}

/// [`phase`] from the PIDs of the holders and of the queued requests, with
/// `parent` naming a process's parent.
fn phase_of(
    job: &Job,
    holders: &[u32],
    queued: &[u32],
    parent: impl Fn(u32) -> Option<u32>,
) -> Phase {
    let ours = |pid: u32| job.pid.is_some() && (Some(pid) == job.pid || parent(pid) == job.pid);
    match job.state {
        JobState::Finished { .. } => Phase::Finished,
        JobState::Pending if job.after.is_some() => Phase::WaitingForJob,
        _ if holders.iter().any(|pid| ours(*pid)) => Phase::Holding,
        _ if queued.iter().any(|pid| ours(*pid)) => Phase::WaitingForStand,
        _ => Phase::Building,
    }
}

/// The parent of the live process `pid`: field 4 of `/proc/<pid>/stat`,
/// after the parenthesised command name.
fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum JobState {
    /// Waiting for its `after` job, or not started yet.
    Pending,
    /// Building, waiting for the stand, or running.
    Started,
    Finished {
        outcome: JobOutcome,
        runs: Vec<String>,
    },
}

/// A job's outcome, and `wait`'s exit status for it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobOutcome {
    Passed,
    Failed,
    Interrupted,
    Blocked,
    Broken,
    /// The command ended without creating a run.
    NoRun,
    /// The job's process is gone without finishing.
    Abandoned,
}

impl JobOutcome {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Interrupted => 2,
            Self::Blocked => 3,
            Self::Broken => 4,
            Self::NoRun => 5,
            Self::Abandoned => 6,
        }
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Blocked => "blocked",
            Self::Broken => "broken",
            Self::NoRun => "no-run",
            Self::Abandoned => "abandoned",
        }
    }

    /// The outcome of runs with these suite outcomes: the worst of them.
    pub fn of_runs(outcomes: &[Option<Outcome>]) -> Self {
        let rank = |outcome: Self| match outcome {
            Self::Passed => 0,
            Self::Failed => 1,
            Self::Blocked => 2,
            Self::Broken => 3,
            Self::Interrupted => 4,
            Self::NoRun => 5,
            Self::Abandoned => 6,
        };
        if outcomes.is_empty() {
            return Self::NoRun;
        }
        outcomes
            .iter()
            .map(|outcome| match outcome {
                Some(Outcome::Passed) => Self::Passed,
                Some(Outcome::Failed) => Self::Failed,
                Some(Outcome::Blocked | Outcome::Skipped) => Self::Blocked,
                Some(Outcome::Broken) => Self::Broken,
                Some(Outcome::Interrupted | Outcome::BoardQuarantined) | None => Self::Interrupted,
            })
            .max_by_key(|outcome| rank(*outcome))
            .unwrap_or(Self::NoRun)
    }
}

impl std::fmt::Display for JobOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// The job a run starts after, and whether it starts whatever that job's
/// outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct After {
    pub job: String,
    /// `--after-any`: start even when the job created no judged run.
    pub any: bool,
}

impl JobOutcome {
    /// Whether a job with `--after` (not `--after-any`) starts after one
    /// that ended so: after a judged run, passed or failed, not after one
    /// that created none, was blocked, broken, interrupted or abandoned.
    pub const fn lets_dependents_start(self) -> bool {
        matches!(self, Self::Passed | Self::Failed)
    }
}

/// The jobs of the stand, in its arbiter directory.
pub struct Jobs {
    directory: PathBuf,
}

impl Jobs {
    pub fn open() -> Result<Self> {
        Ok(Self::at(
            oer_hil_arbiter::Arbiter::open()?.directory().join("jobs"),
        ))
    }

    pub fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(format!("`{id}` is not a job id").into());
        }
        Ok(self.directory.join(format!("{id}.json")))
    }

    pub fn read(&self, id: &str) -> Result<Job> {
        let path = self.path(id)?;
        let bytes = fs::read(&path).map_err(|_| format!("no job `{id}`"))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn write(&self, job: &Job) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let path = self.path(&job.id)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(job)?)?;
        fs::rename(temporary, path)?;
        Ok(())
    }

    /// Every job that has not finished, oldest first. A job whose process is
    /// gone is recorded as abandoned and left out.
    pub fn unfinished(&self) -> Vec<Job> {
        let mut jobs = fs::read_dir(&self.directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                self.read(name.strip_suffix(".json")?).ok()
            })
            .filter(|job| !matches!(job.state, JobState::Finished { .. }))
            .filter(|job| {
                if alive(job) {
                    return true;
                }
                let mut abandoned = job.clone();
                abandoned.state = JobState::Finished {
                    outcome: JobOutcome::Abandoned,
                    runs: Vec::new(),
                };
                let _ = self.write(&abandoned);
                false
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| (a.enqueued_unix_millis, &a.id).cmp(&(b.enqueued_unix_millis, &b.id)));
        jobs
    }

    /// Jobs that ended without a judged run within the last `window`,
    /// newest first, at most `limit`: what `queue` shows so a lost chain is
    /// seen. Records older than a week are removed.
    pub fn recently_ended_unjudged(&self, window: Duration, limit: usize) -> Vec<Job> {
        let now = unix_millis();
        let mut jobs = fs::read_dir(&self.directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let job = self.read(name.strip_suffix(".json")?).ok()?;
                let age = now.saturating_sub(job.enqueued_unix_millis);
                if age > RECORD_RETENTION.as_millis() as u64 {
                    let _ = fs::remove_file(entry.path());
                    return None;
                }
                Some(job)
            })
            .filter(|job| {
                matches!(
                    job.state,
                    JobState::Finished { outcome, .. } if !outcome.lets_dependents_start()
                ) && now.saturating_sub(job.enqueued_unix_millis) <= window.as_millis() as u64
            })
            .collect::<Vec<_>>();
        jobs.sort_by(|a, b| (b.enqueued_unix_millis, &b.id).cmp(&(a.enqueued_unix_millis, &a.id)));
        jobs.truncate(limit);
        jobs
    }

    /// The job's outcome once it has one: finished, or abandoned by its
    /// process.
    pub fn settled(&self, id: &str) -> Result<Option<JobOutcome>> {
        let job = self.read(id)?;
        Ok(match job.state {
            JobState::Finished { outcome, .. } => Some(outcome),
            _ if !alive(&job) => Some(JobOutcome::Abandoned),
            _ => None,
        })
    }

    /// Block until job `id` settles.
    pub fn wait(&self, id: &str) -> Result<JobOutcome> {
        loop {
            if let Some(outcome) = self.settled(id)? {
                return Ok(outcome);
            }
            oer_process::check_cancelled()?;
            std::thread::sleep(POLL);
        }
    }
}

/// Whether the job's process still runs; a job not yet given one counts as
/// running for a minute after it was enqueued.
fn alive(job: &Job) -> bool {
    match (job.pid, job.pid_started_unix_millis) {
        (Some(pid), Some(started)) => {
            oer_hil_arbiter::process_started_unix_millis(pid) == Some(started)
        }
        _ => unix_millis().saturating_sub(job.enqueued_unix_millis) < 60_000,
    }
}

/// A job id unique on this host: its enqueue time and the process making it.
fn new_id(enqueued_unix_millis: u64) -> String {
    format!("{enqueued_unix_millis}-{:x}", std::process::id())
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// What a job runs with, fixed before it waits: a copy of this xtask, the
/// checkout's runner with its receipt, and for a `run` the source snapshot.
/// Edits, pulls and rebuilds of the checkout after that do not reach the job,
/// which otherwise built them in the middle of an experiment.
#[derive(Clone, Debug)]
pub struct Frozen {
    pub xtask: PathBuf,
    pub runner: PathBuf,
    pub receipt: PathBuf,
    pub snapshot: Option<PathBuf>,
}

impl Frozen {
    /// This xtask and the checkout's runner as they are now.
    pub fn capture(ctx: &Context) -> Result<Self> {
        use sha2::{Digest as _, Sha256};
        let (runner, receipt) = crate::hil::prepare(ctx)?;
        let bytes = fs::read(std::env::current_exe()?)?;
        let directory = ctx
            .root
            .join("target/hil/jobs/xtask")
            .join(format!("{:x}", Sha256::digest(&bytes)));
        fs::create_dir_all(&directory)?;
        let xtask = directory.join("oer-xtask");
        if !xtask.is_file() {
            let mut copy = tempfile::NamedTempFile::new_in(&directory)?;
            std::io::Write::write_all(&mut copy, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                copy.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o755))?;
            }
            copy.persist(&xtask)?;
        }
        Ok(Self {
            xtask,
            runner,
            receipt,
            snapshot: None,
        })
    }

    /// The parts this process's job was fixed with, if it is such a job.
    pub fn inherited() -> Result<Option<Self>> {
        let variable = |name| std::env::var_os(name).map(PathBuf::from);
        let (Some(xtask), Some(runner), Some(receipt)) = (
            variable(FROZEN_XTASK_ENV),
            variable(FROZEN_RUNNER_ENV),
            variable(FROZEN_RECEIPT_ENV),
        ) else {
            return Ok(None);
        };
        for path in [&xtask, &runner, &receipt] {
            if !path.is_file() {
                return Err(format!(
                    "{} was fixed for this job when it was enqueued and is gone; enqueue it again",
                    path.display()
                )
                .into());
            }
        }
        Ok(Some(Self {
            xtask,
            runner,
            receipt,
            snapshot: variable(FROZEN_SNAPSHOT_ENV),
        }))
    }

    /// Every file or directory the job needs to stay.
    pub fn paths(&self) -> Vec<PathBuf> {
        [&self.xtask, &self.runner, &self.receipt]
            .into_iter()
            .cloned()
            .chain(self.snapshot.clone())
            .collect()
    }

    /// A command running this frozen xtask's `hil` in `ctx`'s checkout, with
    /// the frozen parts in its environment.
    pub fn hil_command(&self, ctx: &Context) -> std::process::Command {
        let mut command = ctx.command(&self.xtask);
        command
            .arg("--root")
            .arg(&ctx.root)
            .arg("hil")
            .env(FROZEN_XTASK_ENV, &self.xtask)
            .env(FROZEN_RUNNER_ENV, &self.runner)
            .env(FROZEN_RECEIPT_ENV, &self.receipt);
        match &self.snapshot {
            Some(snapshot) => command.env(FROZEN_SNAPSHOT_ENV, snapshot),
            None => command.env_remove(FROZEN_SNAPSHOT_ENV),
        };
        command
    }
}

/// `--enqueue` and `--after JOB` (or `--after-any JOB`) taken from `cargo
/// hil` arguments, up to a `--`.
pub fn take(args: Vec<OsString>) -> Result<(bool, Option<After>, Vec<OsString>)> {
    let mut enqueue = false;
    let mut after = None;
    let mut remaining = Vec::new();
    let mut rest = args.into_iter();
    while let Some(argument) = rest.next() {
        let text = argument.to_str().unwrap_or_default();
        let dependency = [(AFTER, false), (AFTER_ANY, true)]
            .into_iter()
            .find_map(|(flag, any)| {
                if text == flag {
                    Some((flag, any, None))
                } else {
                    text.strip_prefix(flag)
                        .and_then(|value| value.strip_prefix('='))
                        .map(|value| (flag, any, Some(value.to_owned())))
                }
            });
        if text == "--" {
            remaining.push(argument);
            remaining.extend(rest.by_ref());
            break;
        } else if text == ENQUEUE {
            enqueue = true;
        } else if let Some((flag, any, value)) = dependency {
            let job = match value {
                Some(value) => value,
                None => rest
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| format!("{flag} requires a job id"))?,
            };
            if after.replace(After { job, any }).is_some() {
                return Err("--after or --after-any is given twice".into());
            }
        } else {
            remaining.push(argument);
        }
    }
    Ok((enqueue, after, remaining))
}

/// Record a job for `args` and start it detached; returns its id.
pub fn enqueue(
    ctx: &Context,
    owner: &str,
    args: &[OsString],
    after: Option<After>,
    frozen: &Frozen,
) -> Result<String> {
    let jobs = Jobs::open()?;
    if let Some(after) = &after {
        jobs.read(&after.job)?;
    }
    let enqueued = unix_millis();
    let id = new_id(enqueued);
    let log_directory = ctx.root.join("target/hil/jobs");
    fs::create_dir_all(&log_directory)?;
    let log = log_directory.join(format!("{id}.log"));
    let mut job = Job {
        id: id.clone(),
        owner: owner.to_owned(),
        command: args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        checkout: ctx.root.clone(),
        after: after.as_ref().map(|after| after.job.clone()),
        after_any: after.as_ref().is_some_and(|after| after.any),
        enqueued_unix_millis: enqueued,
        pid: None,
        pid_started_unix_millis: None,
        log: Some(log.clone()),
        fixed: frozen.paths(),
        state: JobState::Pending,
    };
    jobs.write(&job)?;
    let output = fs::File::create(&log)?;
    let mut command = frozen.hil_command(ctx);
    command
        .args(args)
        .args(after.iter().flat_map(|after| {
            [
                if after.any { AFTER_ANY } else { AFTER },
                after.job.as_str(),
            ]
        }))
        .env(JOB_ENV, &id)
        .env(oer_hil_arbiter::OWNER_ENV, owner)
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // Its own process group: the job outlives this command and its
        // terminal's signals.
        command.process_group(0);
    }
    let child = command.spawn()?;
    job.pid = Some(child.id());
    job.pid_started_unix_millis = oer_hil_arbiter::process_started_unix_millis(child.id());
    jobs.write(&job)?;
    Ok(id)
}

/// The job this process runs as, if any: waits for the job it runs after,
/// marks it started, and finishes it when dropped.
pub struct Running {
    jobs: Jobs,
    id: Option<String>,
    finished: bool,
}

impl Running {
    /// Record this run as a job (a detached one already has its record),
    /// wait for `after` (a job id from `--after`) and mark the job started.
    /// Every run is a job, so `queue` shows it while it builds.
    pub fn begin(
        ctx: &Context,
        owner: &str,
        args: &[OsString],
        after: Option<&After>,
    ) -> Result<Self> {
        let jobs = Jobs::open()?;
        let id = match std::env::var(JOB_ENV) {
            Ok(id) => id,
            Err(_) => {
                let enqueued = unix_millis();
                let pid = std::process::id();
                let job = Job {
                    id: new_id(enqueued),
                    owner: owner.to_owned(),
                    command: args
                        .iter()
                        .map(|argument| argument.to_string_lossy().into_owned())
                        .collect(),
                    checkout: ctx.root.clone(),
                    after: after.map(|after| after.job.clone()),
                    after_any: after.is_some_and(|after| after.any),
                    enqueued_unix_millis: enqueued,
                    pid: Some(pid),
                    pid_started_unix_millis: oer_hil_arbiter::process_started_unix_millis(pid),
                    log: None,
                    fixed: Vec::new(),
                    state: JobState::Pending,
                };
                jobs.write(&job)?;
                job.id
            }
        };
        let running = Self {
            jobs,
            id: Some(id.clone()),
            finished: false,
        };
        if let Some(After { job, any }) = after {
            eprintln!("hil: waiting for job {job}");
            let outcome = running.jobs.wait(job)?;
            if !any && !outcome.lets_dependents_start() {
                return Err(format!(
                    "job {job} ended {outcome} without a judged run; not starting (pass \
                     {AFTER_ANY} to start after it whatever its outcome)"
                )
                .into());
            }
            eprintln!("hil: job {job} ended {outcome}; starting");
        }
        let mut job = running.jobs.read(&id)?;
        job.state = JobState::Started;
        running.jobs.write(&job)?;
        Ok(running)
    }

    /// Finish the job with its runs' outcomes.
    pub fn finish(&mut self, runs: &[String], outcomes: &[Option<Outcome>]) -> Result<()> {
        self.finished = true;
        let Some(id) = &self.id else {
            return Ok(());
        };
        let mut job = self.jobs.read(id)?;
        job.state = JobState::Finished {
            outcome: JobOutcome::of_runs(outcomes),
            runs: runs.to_vec(),
        };
        self.jobs.write(&job)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = self.finish(&[], &[])
        {
            eprintln!("hil: the job's record was not finished: {error}");
        }
    }
}

/// `cargo hil wait JOB`: block until the job settles, print its outcome and
/// runs, and exit with the outcome's code.
pub fn wait_command(args: &[OsString]) -> Result<std::process::ExitCode> {
    let [id] = args else {
        return Err("usage: cargo hil wait JOB".into());
    };
    let id = id.to_str().ok_or("a job id is text")?;
    let jobs = Jobs::open()?;
    let outcome = jobs.wait(id)?;
    let job = jobs.read(id)?;
    let runs = match &job.state {
        JobState::Finished { runs, .. } => runs.join(" "),
        _ => String::new(),
    };
    println!(
        "job {id}: {outcome}; runs: {runs}{}",
        job.log
            .as_deref()
            .map(|log| format!("; log: {}", log.display()))
            .unwrap_or_default()
    );
    Ok(std::process::ExitCode::from(outcome.exit_code()))
}

/// The last line of a job's log, its reason when it ended without a run.
fn last_log_line(job: &Job) -> Option<String> {
    let text = fs::read_to_string(job.log.as_ref()?).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Jobs that ended without a judged run, for `cargo hil queue`.
pub fn describe_ended(jobs: &[Job]) -> String {
    let mut text = String::new();
    for job in jobs {
        let JobState::Finished { outcome, .. } = &job.state else {
            continue;
        };
        text.push_str(&format!(
            "ended:   {} {} `cargo hil {}` {outcome}{}\n",
            job.id,
            job.owner,
            job.command.join(" "),
            last_log_line(job)
                .map(|line| format!(": {line}"))
                .unwrap_or_default()
        ));
    }
    text
}

/// A job with its phase, as `queue --json` and the dashboard show it.
#[derive(Serialize)]
pub struct JobView<'a> {
    #[serde(flatten)]
    pub job: &'a Job,
    pub phase: Phase,
}

pub fn views<'a>(jobs: &'a [Job], status: &oer_hil_arbiter::Status) -> Vec<JobView<'a>> {
    jobs.iter()
        .map(|job| JobView {
            job,
            phase: phase(job, status),
        })
        .collect()
}

/// The unfinished jobs, for `cargo hil queue`: runs still building, and
/// who waits for whom.
pub fn describe(jobs: &[Job], status: &oer_hil_arbiter::Status) -> String {
    describe_views(&views(jobs, status))
}

fn describe_views(views: &[JobView<'_>]) -> String {
    let mut text = String::new();
    for JobView { job, phase } in views {
        let state = match (phase, &job.after) {
            (Phase::WaitingForJob, Some(after)) => format!("waits for job {after}"),
            (phase, _) => String::from(phase.describe()),
        };
        text.push_str(&format!(
            "job:     {} {} `cargo hil {}` {state}\n",
            job.id,
            job.owner,
            job.command.join(" ")
        ));
    }
    text
}

#[cfg(test)]
mod tests;

/// `--board` and `--peer-board` of a command that starts runs, passed on to
/// each run: the pool's boards to use where several of a chip qualify,
/// until the stand's scheduler assigns them.
#[derive(Clone, Debug, Default, Eq, PartialEq, clap::Args)]
pub(crate) struct BoardChoiceArgs {
    /// The device under test, by its stand-file id.
    #[arg(long)]
    pub board: Option<String>,
    /// The peer board, by its stand-file id.
    #[arg(long)]
    pub peer_board: Option<String>,
}

impl BoardChoiceArgs {
    /// The runner arguments that name the chosen boards.
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if let Some(board) = &self.board {
            arguments.extend([String::from("--board"), board.clone()]);
        }
        if let Some(board) = &self.peer_board {
            arguments.extend([String::from("--peer-board"), board.clone()]);
        }
        arguments
    }
}

#[cfg(test)]
mod board_choice_tests {
    use super::BoardChoiceArgs;

    #[test]
    fn the_chosen_boards_reach_each_run() {
        assert!(BoardChoiceArgs::default().arguments().is_empty());
        let both = BoardChoiceArgs {
            board: Some("s31-b".into()),
            peer_board: Some("c5-a".into()),
        };
        assert_eq!(
            both.arguments(),
            ["--board", "s31-b", "--peer-board", "c5-a"]
        );
    }
}
