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
pub const ENQUEUE: &str = "--enqueue";
pub const AFTER: &str = "--after";
/// How often a waiting process reads the job it waits for.
const POLL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Job {
    pub id: String,
    pub owner: String,
    /// The `cargo hil` arguments the job runs.
    pub command: Vec<String>,
    pub checkout: PathBuf,
    /// The job this one starts after, whatever its outcome.
    pub after: Option<String>,
    pub enqueued_unix_millis: u64,
    /// The detached process and its start time, which survive PID reuse.
    pub pid: Option<u32>,
    pub pid_started_unix_millis: Option<u64>,
    /// Where a detached job's output goes; a run in the foreground has none.
    pub log: Option<PathBuf>,
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

/// `--enqueue` and `--after JOB` taken from `cargo hil` arguments, up to a
/// `--`.
pub fn take(args: Vec<OsString>) -> Result<(bool, Option<String>, Vec<OsString>)> {
    let mut enqueue = false;
    let mut after = None;
    let mut remaining = Vec::new();
    let mut rest = args.into_iter();
    while let Some(argument) = rest.next() {
        let text = argument.to_str().unwrap_or_default();
        if text == "--" {
            remaining.push(argument);
            remaining.extend(rest.by_ref());
            break;
        } else if text == ENQUEUE {
            enqueue = true;
        } else if text == AFTER || text.starts_with("--after=") {
            let value = match text.strip_prefix("--after=") {
                Some(value) => value.to_owned(),
                None => rest
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--after requires a job id")?,
            };
            if after.replace(value).is_some() {
                return Err("--after is given twice".into());
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
    after: Option<String>,
) -> Result<String> {
    let jobs = Jobs::open()?;
    if let Some(after) = &after {
        jobs.read(after)?;
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
        after: after.clone(),
        enqueued_unix_millis: enqueued,
        pid: None,
        pid_started_unix_millis: None,
        log: Some(log.clone()),
        state: JobState::Pending,
    };
    jobs.write(&job)?;
    let output = fs::File::create(&log)?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&ctx.root)
        .arg("hil")
        .args(args)
        .args(after.iter().flat_map(|after| [AFTER, after.as_str()]))
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
        after: Option<&str>,
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
                    after: after.map(str::to_owned),
                    enqueued_unix_millis: enqueued,
                    pid: Some(pid),
                    pid_started_unix_millis: oer_hil_arbiter::process_started_unix_millis(pid),
                    log: None,
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
        if let Some(after) = after {
            eprintln!("hil: waiting for job {after}");
            let outcome = running.jobs.wait(after)?;
            eprintln!("hil: job {after} ended {outcome}; starting");
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
