//! Canonical, immutable records for one host HIL invocation.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicU64,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::sync::atomic::Ordering;

use crate::evidence::{build, build::SourceMaterial};
use crate::{Result, image::ImageClass};

mod archive;
mod integrity;
mod model;
pub(crate) mod validation;
use crate::reporting::render;

pub(crate) use integrity::atomic_write;
pub(crate) use integrity::{atomic_json, collect_attachments};
pub(super) use integrity::{collect_integrity_files, sha256_file, write_integrity_index};
pub(crate) use model::RunManifest;
pub(crate) use model::{
    Attachment, Comparison, CompletionReport, Failure, FailureKind, Measurement, MeasurementUnit,
    MeasurementVerdict, Outcome, PlanDisposition, PlanEntry, PlannedFirmware, RUN_SCHEMA,
    RepetitionResult, RunPlan, RunState, ScenarioResult, SuiteCounts, SuiteResult, Threshold,
};
pub(super) use model::{
    CellProvenance, FirmwareArtifact, FirmwareReplayOrigin, IntegrityFile, IntegrityIndex,
    RepositoryProvenance, aggregate_outcome,
};
use model::{EventRecord, RunnerProvenance, ToolVersion};

static UNIQUE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct RunSession {
    repository_root: PathBuf,
    target_directory: PathBuf,
    directory: PathBuf,
    source_materials: Vec<SourceMaterial>,
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
    pub(crate) fn create(
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
        let runner = runner_provenance();
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
        };
        atomic_json(&directory.join("manifest.json"), &manifest)?;
        let mut session = Self {
            repository_root: root.to_owned(),
            target_directory,
            directory,
            source_materials,
            manifest,
            started,
            events,
            finished: false,
        };
        session.record_event("run-started", None, None, None)?;
        unpublished_directory.publish();
        Ok(session)
    }

    pub(crate) fn id(&self) -> &str {
        &self.manifest.run_id
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn write_plan(&self, plan: &RunPlan) -> Result<()> {
        atomic_json(&self.directory.join("plan.json"), plan)
    }

    pub(crate) fn record_lab_provenance(
        &mut self,
        provenance: &crate::lab::provenance::LabProvenance,
    ) -> Result<()> {
        let path = PathBuf::from("lab-provenance.json");
        atomic_json(&self.directory.join(&path), provenance)?;
        self.manifest.lab_provenance_path = Some(path);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)
    }

    pub(crate) fn scenario_directory(&self, scenario: &str) -> PathBuf {
        self.directory.join("scenarios").join(scenario)
    }

    pub(crate) fn record_event(
        &mut self,
        kind: &str,
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

    pub(crate) fn finish(
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
        self.record_event("run-finished", None, None, Some(outcome))?;
        self.manifest.state = RunState::Completed;
        self.manifest.finished_unix_millis = Some(finished_unix_millis);
        self.manifest.duration_millis = Some(duration_millis);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        let integrity_report = write_integrity_index(&self.directory, &self.manifest.run_id)?;
        self.finished = true;
        let history =
            crate::reporting::history::rebuild_at(&self.target_directory, &self.manifest.target)?;
        let completion = CompletionReport {
            schema: RUN_SCHEMA,
            run_id: self.manifest.run_id.clone(),
            outcome,
            run_directory: self.directory.clone(),
            suite_report: self.directory.join("suite.json"),
            junit_report: self.directory.join("junit.xml"),
            html_report: self.directory.join("report.html"),
            integrity_report,
            history_report: history.history_report,
            history_html: history.html_report,
        };
        Ok((suite, completion))
    }
}

impl Drop for RunSession {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.record_event("run-interrupted", None, None, Some(Outcome::Interrupted));
        self.manifest.state = RunState::Interrupted;
        self.manifest.finished_unix_millis = unix_millis().ok();
        self.manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        let _ = atomic_json(&self.directory.join("manifest.json"), &self.manifest);
        let _ = write_integrity_index(&self.directory, &self.manifest.run_id);
    }
}

pub(crate) fn unix_millis() -> Result<u64> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    u64::try_from(millis).map_err(|_| "host timestamp exceeds the HIL report range".into())
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
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

fn runner_provenance() -> RunnerProvenance {
    RunnerProvenance {
        package: env!("CARGO_PKG_NAME").to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        protocol_version: open_esp_radio_hil_protocol::PROTOCOL_VERSION,
        host_os: std::env::consts::OS.to_owned(),
        host_arch: std::env::consts::ARCH.to_owned(),
        tools: ["rustc", "cargo", "espflash"]
            .into_iter()
            .map(|name| ToolVersion {
                name: name.to_owned(),
                version: command_version(name),
            })
            .collect(),
    }
}

fn command_version(program: &str) -> Option<String> {
    let output = Command::new(program).arg("--version").output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests;
