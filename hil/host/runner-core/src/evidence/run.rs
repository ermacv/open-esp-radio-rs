//! Canonical, immutable records for one host HIL invocation.

use oer_process::CommandExt as _;
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::sync::atomic::Ordering;

use crate::evidence::{build, build::SourceMaterial};
use crate::{Result, image::ImageClass};

mod archive;
mod attempt;
pub use attempt::completed_attempts;
mod integrity;
mod model;
mod snapshot;
pub mod validation;
use crate::evidence::reporting::render;

pub use integrity::collect_attachments;
// Evidence submodules share the durable-file helpers through this owner.
pub(super) use crate::durable::{atomic_json, atomic_write, sha256_file, unix_millis};
pub(super) use integrity::{collect_integrity_files, write_integrity_index};
pub use model::RunManifest;
pub use model::RunnerProvenance;
pub use model::{
    Attachment, Comparison, CompletionReport, Failure, FailureKind, Measurement, MeasurementUnit,
    MeasurementVerdict, Outcome, PlanDisposition, PlanEntry, PlannedFirmware, RUN_SCHEMA,
    RepetitionResult, RunEventKind, RunPlan, RunState, ScenarioResult, SuiteCounts, SuiteResult,
    Threshold,
};
pub use model::{Boot, SubjectRecord};
pub(super) use model::{
    CellProvenance, FirmwareArtifact, FirmwareReplayOrigin, IntegrityFile, IntegrityIndex,
    RepositoryProvenance, aggregate_outcome,
};
use model::{EventRecord, ToolVersion};

pub struct RunSession {
    target_directory: PathBuf,
    directory: PathBuf,
    source_materials: Vec<SourceMaterial>,
    frozen_sources: Option<crate::image::snapshot::FrozenSources>,
    snapshot_materials: Vec<build::BuildFileMaterial>,
    manifest: RunManifest,
    started: Instant,
    events: File,
    finished: bool,
}

struct UnpublishedRunDirectory {
    path: PathBuf,
    published: bool,
}

impl UnpublishedRunDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: false,
        }
    }

    fn publish(&mut self) {
        self.published = true;
    }
}

impl Drop for UnpublishedRunDirectory {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

impl RunSession {
    pub fn create(
        root: &Path,
        target: &str,
        cell_id: &str,
        device_id: &str,
        serial_device: &Path,
        invocation: Vec<OsString>,
    ) -> Result<Self> {
        let started = Instant::now();
        let started_unix_millis = unix_millis()?;
        let run_id = create_run_id(started_unix_millis);
        let target_directory = root.join("target/hil").join(target);
        let runs = target_directory.join("runs");
        fs::create_dir_all(&runs)?;
        let directory = create_unique_directory(&runs, &run_id)?;
        let mut unpublished_directory = UnpublishedRunDirectory::new(directory.clone());
        let run_id = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("HIL run directory does not have a UTF-8 name")?
            .to_owned();
        let source_materials = build::capture_sources(root, &directory)?;
        let repository_source = source_materials
            .first()
            .ok_or("HIL source material set has no primary repository")?;
        let repository = RepositoryProvenance {
            commit: repository_source.commit.clone(),
            dirty: repository_source.dirty,
            workspace_sha256: repository_source.workspace_sha256.clone(),
        };
        let mut runner = runner_provenance()?;
        // The observer build is shared by the runs of one observer: it is
        // stored once beside the runs directory, where that directory really
        // is when it links to the shared store, and the manifest names it by
        // digest.
        if let Some(observer) = &runner.observer {
            runner.observer = Some(oer_hil_schema::observer_store::detach(
                observer,
                &observer_directory(&runs)?,
            )?);
        }
        let events = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(directory.join("events.jsonl"))?;
        let manifest = RunManifest {
            schema: RUN_SCHEMA,
            run_id,
            target: target.to_owned(),
            state: RunState::Running,
            started_unix_millis,
            finished_unix_millis: None,
            duration_millis: None,
            invocation: invocation
                .into_iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            repository,
            runner,
            cell: CellProvenance {
                cell_id: cell_id.to_owned(),
                device_id: device_id.to_owned(),
                serial_device: serial_device.to_owned(),
            },
            lab_provenance_path: None,
            firmware: Vec::new(),
            experiment: crate::experiment::Experiment::from_environment()?,
            messages_used: Vec::new(),
        };
        atomic_json(&directory.join("manifest.json"), &manifest)?;
        record_created_run(&manifest.run_id)?;
        let mut session = Self {
            target_directory,
            directory,
            source_materials,
            frozen_sources: None,
            snapshot_materials: Vec::new(),
            manifest,
            started,
            events,
            finished: false,
        };
        session.record_event(RunEventKind::RunStarted, None, None, None)?;
        unpublished_directory.publish();
        Ok(session)
    }

    pub fn id(&self) -> &str {
        &self.manifest.run_id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The source the run's current-source images are built from.
    pub fn repository(&self) -> &RepositoryProvenance {
        &self.manifest.repository
    }

    pub fn write_plan(&self, plan: &RunPlan) -> Result<()> {
        atomic_json(&self.directory.join("plan.json"), plan)
    }

    pub fn record_lab_provenance(
        &mut self,
        provenance: &crate::lab::provenance::LabProvenance,
    ) -> Result<()> {
        let path = PathBuf::from("lab-provenance.json");
        atomic_json(&self.directory.join(&path), provenance)?;
        self.manifest.lab_provenance_path = Some(path);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)
    }

    pub fn scenario_directory(&self, scenario: &str) -> PathBuf {
        self.directory.join("scenarios").join(scenario)
    }

    pub fn record_event(
        &mut self,
        kind: RunEventKind,
        scenario: Option<&str>,
        image: Option<ImageClass>,
        outcome: Option<Outcome>,
    ) -> Result<()> {
        let mut record = serde_json::to_vec(&EventRecord {
            timestamp_unix_millis: unix_millis()?,
            kind,
            scenario,
            image,
            outcome,
        })?;
        record.push(b'\n');
        self.events.write_all(&record)?;
        self.events.sync_data()?;
        Ok(())
    }

    pub fn finish(
        mut self,
        scenarios: Vec<ScenarioResult>,
    ) -> Result<(SuiteResult, CompletionReport)> {
        let finished_unix_millis = unix_millis()?;
        let duration_millis = duration_millis(self.started.elapsed());
        let counts = SuiteCounts::from_results(&scenarios);
        let outcome = if scenarios.iter().all(|result| result.outcome.is_passed()) {
            Outcome::Passed
        } else {
            Outcome::Failed
        };
        let suite = SuiteResult {
            schema: RUN_SCHEMA,
            run_id: self.manifest.run_id.clone(),
            target: self.manifest.target.clone(),
            outcome,
            started_unix_millis: self.manifest.started_unix_millis,
            finished_unix_millis,
            duration_millis,
            counts,
            scenarios,
        };
        atomic_json(&self.directory.join("suite.json"), &suite)?;
        atomic_write(
            &self.directory.join("junit.xml"),
            render::junit(&suite, &self.manifest).as_bytes(),
        )?;
        atomic_write(
            &self.directory.join("report.html"),
            render::html(&suite, &self.manifest).as_bytes(),
        )?;
        self.record_event(RunEventKind::RunFinished, None, None, Some(outcome))?;
        self.manifest.messages_used = messages_used(&self.directory)?;
        self.manifest.state = RunState::Completed;
        self.manifest.finished_unix_millis = Some(finished_unix_millis);
        self.manifest.duration_millis = Some(duration_millis);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        let integrity_report = write_integrity_index(&self.directory, &self.manifest.run_id)?;
        self.finished = true;
        let completion = CompletionReport {
            schema: RUN_SCHEMA,
            run_id: self.manifest.run_id.clone(),
            outcome,
            run_directory: self.directory.clone(),
            suite_report: self.directory.join("suite.json"),
            junit_report: self.directory.join("junit.xml"),
            html_report: self.directory.join("report.html"),
            integrity_report,
        };
        Ok((suite, completion))
    }
}

impl Drop for RunSession {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.record_event(
            RunEventKind::RunInterrupted,
            None,
            None,
            Some(Outcome::Interrupted),
        );
        self.manifest.messages_used = messages_used(&self.directory).unwrap_or_default();
        self.manifest.state = RunState::Interrupted;
        self.manifest.finished_unix_millis = unix_millis().ok();
        self.manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        let _ = atomic_json(&self.directory.join("manifest.json"), &self.manifest);
        match write_integrity_index(&self.directory, &self.manifest.run_id) {
            Ok(integrity) => {
                let _ = crate::emit_json(
                    &serde_json::json!({
                        "schema": RUN_SCHEMA, "run_id": self.manifest.run_id,
                        "outcome": "interrupted", "run_directory": self.directory,
                        "integrity_report": integrity,
                    }),
                    false,
                );
            }
            Err(error) => eprintln!("cannot seal interrupted HIL run: {error}"),
        }
    }
}

/// Every message path the run's protocol captures record: the target's
/// messages and the host's requests, sorted and unique.
fn messages_used(directory: &Path) -> Result<Vec<String>> {
    fn captures(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                captures(&path, found)?;
            } else if path
                .file_name()
                .is_some_and(|name| name == "protocol.jsonl")
            {
                found.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    captures(directory, &mut files)?;
    let mut paths = std::collections::BTreeSet::new();
    for file in files {
        for line in fs::read_to_string(&file)?.lines() {
            let record: serde_json::Value = serde_json::from_str(line)
                .map_err(|error| format!("{}: {error}", file.display()))?;
            let path = match record["record"].as_str() {
                Some("target-event") => &record["message"]["path"],
                Some("host-command") => &record["command"]["path"],
                _ => continue,
            };
            if let Some(path) = path.as_str() {
                paths.insert(path.to_owned());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn create_run_id(started_unix_millis: u64) -> String {
    format!("{started_unix_millis}-{:08x}", std::process::id())
}

fn create_unique_directory(parent: &Path, base: &str) -> Result<PathBuf> {
    for suffix in 0_u16..=u16::MAX {
        let name = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base}-{suffix:04}")
        };
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("cannot allocate a unique HIL run directory".into())
}

use oer_hil_schema::artifacts as observer_artifacts;

/// Build identity of the executable that records runs.
#[derive(Clone, Copy, Debug)]
pub struct RunnerBuild {
    /// The executable's embedded host build record, as JSON.
    pub record: &'static str,
    pub package: &'static str,
    pub version: &'static str,
}

static RUNNER_BUILD: std::sync::OnceLock<RunnerBuild> = std::sync::OnceLock::new();

/// Register the running executable's identity before it records any run.
pub fn register_runner(build: RunnerBuild) -> Result<()> {
    RUNNER_BUILD
        .set(build)
        .map_err(|_| "runner build identity was registered more than once".into())
}

fn runner_build() -> Result<RunnerBuild> {
    // Unit tests record runs without the runner executable's build script.
    #[cfg(test)]
    let _ = RUNNER_BUILD.set(RunnerBuild {
        record: "{}",
        package: "oer-hil-runner",
        version: "0.0.0",
    });
    RUNNER_BUILD
        .get()
        .copied()
        .ok_or_else(|| "runner build identity is not registered".into())
}

/// Names the file the invoking tool reads back the IDs of the runs this
/// runner process created from, one per line.
pub const RUN_RECEIPT_ENV: &str = "OER_HIL_RUN_RECEIPT";

/// Append `run_id` to the invoking tool's run receipt, when it asked for one.
fn record_created_run(run_id: &str) -> Result<()> {
    let Some(path) = std::env::var_os(RUN_RECEIPT_ENV) else {
        return Ok(());
    };
    use std::io::Write as _;
    let mut receipt = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(receipt, "{run_id}")?;
    Ok(())
}

/// The directory holding the observer builds of the runs in `runs`: the
/// parent of the directory `runs` resolves to.
pub fn observer_directory(runs: &Path) -> Result<PathBuf> {
    Ok(fs::canonicalize(runs)?
        .parent()
        .ok_or("HIL runs directory has no parent")?
        .to_owned())
}

pub fn runner_provenance() -> Result<RunnerProvenance> {
    // On Linux this reads the actual running inode even after a rebuild replaces
    // the pathname. The source record comes from this executable's build script.
    #[cfg(target_os = "linux")]
    let executable_sha256 = Some(sha256_file(Path::new("/proc/self/exe"))?);
    // A pathname on another host does not establish the running inode after
    // replacement. Retain the embedded build and leave this identity unknown.
    #[cfg(not(target_os = "linux"))]
    let executable_sha256: Option<String> = None;
    let runner = runner_build()?;
    let mut build: serde_json::Value = serde_json::from_str(runner.record)?;
    if let Some(path) = std::env::var_os("OER_OBSERVER_RECEIPT") {
        let receipt: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
        if receipt["executable_sha256"].as_str() != executable_sha256.as_deref() {
            return Err("observer receipt does not identify the running executable".into());
        }
        let mut embedded = receipt["build"].clone();
        embedded["resolved"] = build["resolved"].clone();
        if embedded != build {
            return Err("observer receipt does not identify the embedded build".into());
        }
        observer_artifacts::apply(
            &mut build["resolved"],
            receipt["artifacts"]
                .as_array()
                .ok_or("observer artifacts missing")?,
        )?;
        build["resolved"]["selected_profile"] = receipt["profile"].clone();
        if build != receipt["build"] {
            return Err("invalid observer compilation receipt".into());
        }
    }
    use sha2::{Digest, Sha256};
    let build_sha256 = format!("{:x}", Sha256::digest(serde_json::to_vec(&build)?));
    Ok(RunnerProvenance {
        observer: Some(
            serde_json::json!({"schema":1,"executable_sha256":executable_sha256,"build_sha256":build_sha256,"build":build}),
        ),
        package: runner.package.to_owned(),
        version: runner.version.to_owned(),
        messages_lock_sha256: format!("{:x}", Sha256::digest(oer_hil_protocol::MESSAGES_LOCK)),
        host_os: std::env::consts::OS.to_owned(),
        host_arch: std::env::consts::ARCH.to_owned(),
        tools: ["rustc", "cargo", "espflash"]
            .into_iter()
            .map(|name| ToolVersion {
                name: name.to_owned(),
                version: command_version(name),
            })
            .collect(),
    })
}

fn command_version(program: &str) -> Option<String> {
    let output = Command::new(program)
        .arg("--version")
        .supervised_output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests;
