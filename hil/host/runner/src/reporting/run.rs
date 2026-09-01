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

use serde::{Deserialize, Serialize};

#[cfg(test)]
use std::sync::atomic::Ordering;

use super::build::{self, SourceMaterial};
use crate::{Result, qualification::scenario::ImageClass};

mod archive;
mod integrity;
mod render;

pub(crate) use integrity::{atomic_json, collect_attachments};
pub(super) use integrity::{
    atomic_write, collect_integrity_files, sha256_file, write_integrity_index,
};

pub(crate) const RUN_SCHEMA: u16 = 2;
static UNIQUE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RunState {
    Running,
    Completed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Outcome {
    Passed,
    Failed,
    Broken,
    Skipped,
    Blocked,
    Interrupted,
}

impl Outcome {
    pub(crate) const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FailureKind {
    Scenario,
    Precondition,
    ImageBuild,
    ImageFlash,
    Infrastructure,
}

impl FailureKind {
    const fn id(self) -> &'static str {
        match self {
            Self::Scenario => "scenario",
            Self::Precondition => "precondition",
            Self::ImageBuild => "image-build",
            Self::ImageFlash => "image-flash",
            Self::Infrastructure => "infrastructure",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Failure {
    pub(crate) kind: FailureKind,
    pub(crate) message: String,
}

impl Failure {
    pub(crate) fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Attachment {
    pub(crate) path: PathBuf,
    pub(crate) media_type: String,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MeasurementUnit {
    Count,
    Bytes,
    BitsPerSecond,
    Microseconds,
    BasisPoints,
}

impl MeasurementUnit {
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Bytes => "bytes",
            Self::BitsPerSecond => "bit/s",
            Self::Microseconds => "us",
            Self::BasisPoints => "bp",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Comparison {
    AtLeast,
    AtMost,
    Exactly,
}

impl Comparison {
    pub(super) const fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "&gt;=",
            Self::AtMost => "&lt;=",
            Self::Exactly => "=",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct Threshold {
    pub(crate) comparison: Comparison,
    pub(crate) value: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MeasurementVerdict {
    Passed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Measurement {
    pub(crate) name: String,
    pub(crate) value: u64,
    pub(crate) unit: MeasurementUnit,
    pub(crate) threshold: Option<Threshold>,
    pub(crate) verdict: Option<MeasurementVerdict>,
}

impl Measurement {
    pub(crate) fn observed(name: impl Into<String>, value: u64, unit: MeasurementUnit) -> Self {
        Self {
            name: name.into(),
            value,
            unit,
            threshold: None,
            verdict: None,
        }
    }

    pub(crate) fn evaluated(mut self, comparison: Comparison, threshold: u64) -> Self {
        self.threshold = Some(Threshold {
            comparison,
            value: threshold,
        });
        self.verdict = Some(if measurement_passes(self.value, comparison, threshold) {
            MeasurementVerdict::Passed
        } else {
            MeasurementVerdict::Failed
        });
        self
    }

    pub(super) fn is_consistent(&self) -> bool {
        match (self.threshold, self.verdict) {
            (None, None) => true,
            (Some(threshold), Some(verdict)) => {
                let expected =
                    if measurement_passes(self.value, threshold.comparison, threshold.value) {
                        MeasurementVerdict::Passed
                    } else {
                        MeasurementVerdict::Failed
                    };
                verdict == expected
            }
            _ => false,
        }
    }
}

const fn measurement_passes(value: u64, comparison: Comparison, threshold: u64) -> bool {
    match comparison {
        Comparison::AtLeast => value >= threshold,
        Comparison::AtMost => value <= threshold,
        Comparison::Exactly => value == threshold,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RepetitionResult {
    pub(crate) schema: u16,
    pub(crate) repetition: u8,
    pub(crate) outcome: Outcome,
    pub(crate) started_unix_millis: u64,
    pub(crate) duration_millis: u64,
    pub(crate) artifact_directory: PathBuf,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) measurements: Vec<Measurement>,
    pub(crate) failure: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ScenarioResult {
    pub(crate) schema: u16,
    pub(crate) scenario: String,
    pub(crate) image: ImageClass,
    pub(crate) outcome: Outcome,
    pub(crate) required_repetitions: u8,
    pub(crate) repetitions: Vec<RepetitionResult>,
    pub(crate) failure: Option<Failure>,
}

impl ScenarioResult {
    pub(crate) fn from_repetitions(
        scenario: String,
        image: ImageClass,
        required_repetitions: u8,
        repetitions: Vec<RepetitionResult>,
    ) -> Self {
        let outcome = aggregate_outcome(repetitions.iter().map(|entry| entry.outcome));
        Self {
            schema: RUN_SCHEMA,
            scenario,
            image,
            outcome,
            required_repetitions,
            repetitions,
            failure: None,
        }
    }

    pub(crate) fn blocked(
        scenario: String,
        image: ImageClass,
        required_repetitions: u8,
        failure: Failure,
    ) -> Self {
        Self {
            schema: RUN_SCHEMA,
            scenario,
            image,
            outcome: Outcome::Blocked,
            required_repetitions,
            repetitions: Vec::new(),
            failure: Some(failure),
        }
    }
}

pub(super) fn aggregate_outcome(outcomes: impl IntoIterator<Item = Outcome>) -> Outcome {
    let observed = outcomes.into_iter().collect::<Vec<_>>();
    if !observed.is_empty() && observed.iter().all(|outcome| outcome.is_passed()) {
        Outcome::Passed
    } else if observed.contains(&Outcome::Interrupted) {
        Outcome::Interrupted
    } else if observed.contains(&Outcome::Broken) {
        Outcome::Broken
    } else if observed.contains(&Outcome::Failed) {
        Outcome::Failed
    } else if observed.contains(&Outcome::Blocked) {
        Outcome::Blocked
    } else {
        Outcome::Skipped
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SuiteCounts {
    pub(crate) scenarios: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) broken: usize,
    pub(crate) skipped: usize,
    pub(crate) blocked: usize,
    pub(crate) interrupted: usize,
}

impl SuiteCounts {
    pub(super) fn from_results(results: &[ScenarioResult]) -> Self {
        let mut counts = Self {
            scenarios: results.len(),
            passed: 0,
            failed: 0,
            broken: 0,
            skipped: 0,
            blocked: 0,
            interrupted: 0,
        };
        for result in results {
            match result.outcome {
                Outcome::Passed => counts.passed += 1,
                Outcome::Failed => counts.failed += 1,
                Outcome::Broken => counts.broken += 1,
                Outcome::Skipped => counts.skipped += 1,
                Outcome::Blocked => counts.blocked += 1,
                Outcome::Interrupted => counts.interrupted += 1,
            }
        }
        counts
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SuiteResult {
    pub(crate) schema: u16,
    pub(crate) run_id: String,
    pub(crate) target: String,
    pub(crate) outcome: Outcome,
    pub(crate) started_unix_millis: u64,
    pub(crate) finished_unix_millis: u64,
    pub(crate) duration_millis: u64,
    pub(crate) counts: SuiteCounts,
    pub(crate) scenarios: Vec<ScenarioResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PlanDisposition {
    Selected,
    Filtered,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PlanEntry {
    pub(crate) scenario: String,
    pub(crate) image: ImageClass,
    pub(crate) repetitions: u8,
    pub(crate) disposition: PlanDisposition,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct RunPlan {
    pub(crate) schema: u16,
    pub(crate) run_id: String,
    pub(crate) selection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) firmware: Option<PlannedFirmware>,
    pub(crate) entries: Vec<PlanEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "source")]
pub(crate) enum PlannedFirmware {
    BuildCurrent,
    Replay {
        source_run_id: String,
        image: ImageClass,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_id: Option<String>,
        application_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RepositoryProvenance {
    pub(super) commit: String,
    pub(super) dirty: bool,
    pub(super) workspace_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct FirmwareReplayOrigin {
    pub(super) source_run_id: String,
    pub(super) source_integrity_sha256: String,
    pub(super) firmware_repository: RepositoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) source_build_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RunnerProvenance {
    package: String,
    version: String,
    protocol_version: u16,
    host_os: String,
    host_arch: String,
    tools: Vec<ToolVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct ToolVersion {
    name: String,
    version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct CellProvenance {
    pub(super) cell_id: String,
    pub(super) device_id: String,
    pub(super) serial_device: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct FirmwareArtifact {
    pub(super) image: ImageClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) replayed_from: Option<FirmwareReplayOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) build_provenance_path: Option<PathBuf>,
    pub(super) application_path: PathBuf,
    pub(super) application_size_bytes: u64,
    pub(super) application_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runtime_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runtime_elf_size_bytes: Option<u64>,
    pub(super) runtime_elf_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runtime_bin_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) runtime_bin_size_bytes: Option<u64>,
    pub(super) runtime_bin_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) bootstrap_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) bootstrap_elf_size_bytes: Option<u64>,
    pub(super) bootstrap_elf_sha256: String,
}

#[derive(Serialize)]
struct EventRecord<'a> {
    timestamp_unix_millis: u64,
    kind: &'a str,
    scenario: Option<&'a str>,
    image: Option<ImageClass>,
    outcome: Option<Outcome>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RunManifest {
    pub(super) schema: u16,
    pub(super) run_id: String,
    pub(super) target: String,
    pub(super) state: RunState,
    pub(super) started_unix_millis: u64,
    pub(super) finished_unix_millis: Option<u64>,
    pub(super) duration_millis: Option<u64>,
    invocation: Vec<String>,
    pub(super) repository: RepositoryProvenance,
    runner: RunnerProvenance,
    pub(super) cell: CellProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lab_provenance_path: Option<PathBuf>,
    pub(super) firmware: Vec<FirmwareArtifact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct IntegrityFile {
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct IntegrityIndex {
    pub(super) schema: u16,
    pub(super) run_id: String,
    pub(super) files: Vec<IntegrityFile>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CompletionReport {
    pub(crate) schema: u16,
    pub(crate) run_id: String,
    pub(crate) outcome: Outcome,
    pub(crate) run_directory: PathBuf,
    pub(crate) suite_report: PathBuf,
    pub(crate) junit_report: PathBuf,
    pub(crate) html_report: PathBuf,
    pub(crate) integrity_report: PathBuf,
    pub(crate) history_report: PathBuf,
    pub(crate) history_html: PathBuf,
}

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
        provenance: &crate::transport::lab_provenance::LabProvenance,
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
        let history = super::history::rebuild_at(&self.target_directory, &self.manifest.target)?;
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
mod tests {
    use super::*;
    use crate::reporting::build::{SourceLimitation, SourceRebuildStatus, capture_source_material};

    fn temporary_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "open-radio-hil-{label}-{}-{}",
            std::process::id(),
            UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn manifest() -> RunManifest {
        RunManifest {
            schema: RUN_SCHEMA,
            run_id: String::from("run<&>"),
            target: String::from("esp32s31"),
            state: RunState::Completed,
            started_unix_millis: 1,
            finished_unix_millis: Some(2),
            duration_millis: Some(1),
            invocation: vec![String::from("cargo hil")],
            repository: RepositoryProvenance {
                commit: String::from("abc123"),
                dirty: false,
                workspace_sha256: String::from("00"),
            },
            runner: RunnerProvenance {
                package: String::from("runner"),
                version: String::from("1"),
                protocol_version: 1,
                host_os: String::from("linux"),
                host_arch: String::from("x86_64"),
                tools: Vec::new(),
            },
            cell: CellProvenance {
                cell_id: String::from("cell-1"),
                device_id: String::from("dut-1"),
                serial_device: PathBuf::from("/dev/ttyACM0"),
            },
            lab_provenance_path: None,
            firmware: Vec::new(),
        }
    }

    fn failed_suite() -> SuiteResult {
        let failure = Failure::new(FailureKind::Scenario, "bad <frame> & timeout");
        let scenarios = vec![ScenarioResult::from_repetitions(
            String::from("udp-rx"),
            ImageClass::Correctness,
            1,
            vec![RepetitionResult {
                schema: RUN_SCHEMA,
                repetition: 1,
                outcome: Outcome::Failed,
                started_unix_millis: 1,
                duration_millis: 250,
                artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
                attachments: Vec::new(),
                measurements: vec![
                    Measurement::observed("udp.rx.loss", 2, MeasurementUnit::Count)
                        .evaluated(Comparison::AtMost, 0),
                ],
                failure: Some(failure),
            }],
        )];
        SuiteResult {
            schema: RUN_SCHEMA,
            run_id: String::from("run<&>"),
            target: String::from("esp32s31"),
            outcome: Outcome::Failed,
            started_unix_millis: 1,
            finished_unix_millis: 251,
            duration_millis: 250,
            counts: SuiteCounts::from_results(&scenarios),
            scenarios,
        }
    }

    fn session(directory: &Path) -> RunSession {
        let mut manifest = manifest();
        manifest.run_id = directory
            .file_name()
            .expect("test run directory has a name")
            .to_string_lossy()
            .into_owned();
        manifest.state = RunState::Running;
        manifest.finished_unix_millis = None;
        manifest.duration_millis = None;
        atomic_json(&directory.join("manifest.json"), &manifest).unwrap();
        let repository_root = directory
            .parent()
            .expect("test run directory has a parent")
            .to_owned();
        manifest.repository = RepositoryProvenance {
            commit: String::new(),
            dirty: true,
            workspace_sha256: String::from(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
        };
        RunSession {
            repository_root: repository_root.clone(),
            target_directory: directory
                .parent()
                .expect("test run directory has a parent")
                .to_owned(),
            directory: directory.to_owned(),
            source_materials: vec![SourceMaterial {
                name: String::from("repository"),
                checkout_path: repository_root,
                remote: Some(String::from("https://example.invalid/repository.git")),
                commit: String::new(),
                dirty: true,
                workspace_sha256: String::from(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                ),
                rebuild_status: SourceRebuildStatus::Incomplete,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: vec![SourceLimitation::RepositoryStateNotCaptured],
            }],
            manifest,
            started: Instant::now(),
            events: File::create(directory.join("events.jsonl")).unwrap(),
            finished: false,
        }
    }

    fn integrated_session(target_directory: &Path) -> RunSession {
        let directory = target_directory.join("runs").join(manifest().run_id);
        fs::create_dir_all(&directory).unwrap();
        let mut session = session(&directory);
        session.target_directory = target_directory.to_owned();
        session
    }

    fn write_test_build_materials(root: &Path) {
        for relative in [
            "Cargo.lock",
            "hil/targets/esp32s31/Cargo.lock",
            "hil/targets/esp32s31/Cargo.toml",
            "hil/targets/esp32s31/stack.toml",
            "hil/targets/esp32s31/partitions/hil.csv",
        ] {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("test material: {relative}\n")).unwrap();
        }
    }

    #[test]
    fn junit_preserves_failure_and_escapes_xml() {
        let xml = render::junit(&failed_suite(), &manifest());
        roxmltree::Document::parse(&xml).expect("valid JUnit XML");
        assert!(xml.contains("tests=\"1\" failures=\"1\""));
        assert!(xml.contains("bad &lt;frame&gt; &amp; timeout"));
        assert!(xml.contains("run_id\" value=\"run&lt;&amp;&gt;"));
        assert!(xml.contains("repetition-001"));
        assert!(xml.contains("measurement.udp.rx.loss=2 count"));
    }

    #[test]
    fn html_is_derived_from_the_same_suite_record() {
        let html = render::html(&failed_suite(), &manifest());
        assert!(html.contains("udp-rx"));
        assert!(html.contains("bad &lt;frame&gt; &amp; timeout"));
        assert!(html.contains("0/1 scenarios passed"));
        assert!(html.contains("udp.rx.loss"));
        assert!(html.contains("&lt;= 0 count"));
    }

    #[test]
    fn evaluated_measurement_binds_threshold_and_verdict() {
        let passed = Measurement::observed("icmp.rtt.p95", 900, MeasurementUnit::Microseconds)
            .evaluated(Comparison::AtMost, 1_000);
        let failed = Measurement::observed("icmp.rtt.p95", 1_001, MeasurementUnit::Microseconds)
            .evaluated(Comparison::AtMost, 1_000);
        assert_eq!(passed.verdict, Some(MeasurementVerdict::Passed));
        assert_eq!(failed.verdict, Some(MeasurementVerdict::Failed));
        assert!(passed.is_consistent());
        assert!(failed.is_consistent());
    }

    #[test]
    fn unique_run_directories_never_replace_a_previous_run() {
        let root = temporary_directory("run");
        let first = create_unique_directory(&root, "123-abc").unwrap();
        fs::write(first.join("evidence"), b"retained").unwrap();
        let second = create_unique_directory(&root, "123-abc").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(first.join("evidence")).unwrap(), b"retained");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repository_archive_seals_tracked_patch_and_marks_untracked_content_incomplete() {
        let base = temporary_directory("source-archive");
        let repository = base.join("repository");
        fs::create_dir(&repository).unwrap();
        for arguments in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "HIL Test"],
            &["config", "user.email", "hil@example.invalid"],
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repository.git",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(arguments)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(repository.join("tracked.txt"), b"base\n").unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["add", "tracked.txt"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["commit", "-m", "base"])
                .status()
                .unwrap()
                .success()
        );
        fs::write(repository.join("tracked.txt"), b"changed\n").unwrap();
        let tracked_run = base.join("tracked-run");
        fs::create_dir(&tracked_run).unwrap();
        let tracked_source = capture_source_material(
            "repository",
            &repository,
            &tracked_run,
            Path::new("source/repository.patch"),
        )
        .unwrap();
        assert!(tracked_source.dirty);
        assert_eq!(
            tracked_source.rebuild_status,
            SourceRebuildStatus::TrackedPatch
        );
        assert!(tracked_source.limitations.is_empty());
        assert!(tracked_source.tracked_patch_size_bytes.unwrap() != 0);
        assert!(
            fs::read_to_string(
                tracked_run.join(tracked_source.tracked_patch_path.expect("tracked patch"))
            )
            .unwrap()
            .contains("+changed")
        );

        fs::write(repository.join("untracked.txt"), b"untracked\n").unwrap();
        let incomplete_run = base.join("incomplete-run");
        fs::create_dir(&incomplete_run).unwrap();
        let incomplete_source = capture_source_material(
            "repository",
            &repository,
            &incomplete_run,
            Path::new("source/repository.patch"),
        )
        .unwrap();
        assert_eq!(
            incomplete_source.rebuild_status,
            SourceRebuildStatus::Incomplete
        );
        assert_eq!(
            incomplete_source.limitations,
            [SourceLimitation::UntrackedContentNotArchived]
        );
        assert_eq!(incomplete_source.untracked_files.len(), 1);
        assert_eq!(
            incomplete_source.untracked_files[0].path,
            Path::new("untracked.txt")
        );
        assert_eq!(incomplete_source.untracked_files[0].size_bytes, 10);
        assert_eq!(incomplete_source.untracked_files[0].sha256.len(), 64);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn attachments_are_sorted_and_content_addressed() {
        let root = temporary_directory("attachments");
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("z.log"), b"serial evidence").unwrap();
        fs::write(root.join("nested/capture.pcapng"), b"pcap evidence").unwrap();
        let attachments = collect_attachments(&root, Path::new("scenario/repetition-001"))
            .expect("index artifacts");
        assert_eq!(attachments.len(), 2);
        assert_eq!(
            attachments[0].path,
            PathBuf::from("scenario/repetition-001/nested/capture.pcapng")
        );
        assert_eq!(attachments[0].media_type, "application/vnd.tcpdump.pcap");
        assert_eq!(attachments[1].media_type, "text/plain");
        assert_eq!(attachments[1].size_bytes, 15);
        assert_eq!(attachments[1].sha256.len(), 64);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn firmware_record_archives_the_exact_application() {
        let root = temporary_directory("firmware");
        write_test_build_materials(&root);
        let run_directory = root.join("run");
        fs::create_dir(&run_directory).unwrap();
        let application = root.join("application.bin");
        let runtime_elf = root.join("runtime.elf");
        let runtime_bin = root.join("runtime.bin");
        let bootstrap_elf = root.join("bootstrap.elf");
        let effective_embedded_lock = root.join("hil/targets/esp32s31/Cargo.lock");
        fs::write(&application, b"application bytes").unwrap();
        fs::write(&runtime_elf, b"runtime elf").unwrap();
        fs::write(&runtime_bin, b"runtime bin").unwrap();
        fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

        let mut first_session = session(&run_directory);
        first_session
            .record_firmware(
                ImageClass::Correctness,
                &application,
                &runtime_elf,
                &runtime_bin,
                &bootstrap_elf,
                &effective_embedded_lock,
            )
            .unwrap();
        let artifact = &first_session.manifest.firmware[0];
        assert_eq!(
            artifact.application_path,
            PathBuf::from("firmware/correctness/application.bin")
        );
        assert_eq!(artifact.application_size_bytes, 17);
        assert_eq!(
            fs::read(run_directory.join(&artifact.application_path)).unwrap(),
            b"application bytes"
        );
        assert_eq!(artifact.application_sha256.len(), 64);
        assert_eq!(
            fs::read(
                run_directory.join(
                    artifact
                        .runtime_elf_path
                        .as_ref()
                        .expect("runtime ELF path")
                )
            )
            .unwrap(),
            b"runtime elf"
        );
        assert_eq!(artifact.runtime_elf_size_bytes, Some(11));
        assert_eq!(
            fs::read(
                run_directory.join(
                    artifact
                        .runtime_bin_path
                        .as_ref()
                        .expect("runtime bin path")
                )
            )
            .unwrap(),
            b"runtime bin"
        );
        assert_eq!(
            fs::read(
                run_directory.join(
                    artifact
                        .bootstrap_elf_path
                        .as_ref()
                        .expect("bootstrap ELF path")
                )
            )
            .unwrap(),
            b"bootstrap elf"
        );
        let provenance_path = artifact
            .build_provenance_path
            .as_ref()
            .expect("build provenance path");
        let provenance: super::super::build::BuildProvenance =
            serde_json::from_slice(&fs::read(run_directory.join(provenance_path)).unwrap())
                .unwrap();
        assert_eq!(provenance.build_id, artifact.build_id.clone().unwrap());
        assert_eq!(provenance.subjects.len(), 4);
        assert!(!provenance.source_reconstructable);
        let object_root = root.join("objects/sha256");
        let first_objects = collect_integrity_files(&object_root).unwrap();
        assert_eq!(first_objects.len(), 5);
        first_session.finished = true;
        drop(first_session);

        let second_run_directory = root.join("run-2");
        fs::create_dir(&second_run_directory).unwrap();
        let mut second = session(&second_run_directory);
        second
            .record_firmware(
                ImageClass::Correctness,
                &application,
                &runtime_elf,
                &runtime_bin,
                &bootstrap_elf,
                &effective_embedded_lock,
            )
            .unwrap();
        assert_eq!(
            collect_integrity_files(&object_root).unwrap(),
            first_objects
        );
        second.finished = true;
        drop(second);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replayed_firmware_bundle_is_self_contained_after_origin_removal() {
        let root = temporary_directory("firmware-replay");
        let target_directory = root.join("target/hil/esp32s31");
        let runs_directory = target_directory.join("runs");
        let repository_root = target_directory.clone();
        fs::create_dir_all(&runs_directory).unwrap();
        write_test_build_materials(&repository_root);

        let application = repository_root.join("application.bin");
        let runtime_elf = repository_root.join("runtime.elf");
        let runtime_bin = repository_root.join("runtime.bin");
        let bootstrap_elf = repository_root.join("bootstrap.elf");
        let effective_embedded_lock = repository_root.join("hil/targets/esp32s31/Cargo.lock");
        fs::write(&application, b"application bytes").unwrap();
        fs::write(&runtime_elf, b"runtime elf").unwrap();
        fs::write(&runtime_bin, b"runtime bin").unwrap();
        fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

        let source_directory = runs_directory.join("source-run");
        fs::create_dir(&source_directory).unwrap();
        let mut source = session(&source_directory);
        source.target_directory = target_directory.clone();
        source.repository_root = repository_root.clone();
        source.source_materials[0].checkout_path = repository_root.clone();
        source
            .record_firmware(
                ImageClass::Correctness,
                &application,
                &runtime_elf,
                &runtime_bin,
                &bootstrap_elf,
                &effective_embedded_lock,
            )
            .unwrap();
        source.finish(Vec::new()).unwrap();

        let archived = super::super::verification::archived_firmware(
            &root,
            "esp32s31",
            "source-run",
            ImageClass::Correctness,
        )
        .unwrap();
        let replay_directory = runs_directory.join("replay-run");
        fs::create_dir(&replay_directory).unwrap();
        let mut replay = session(&replay_directory);
        replay.target_directory = target_directory;
        replay.repository_root = repository_root.clone();
        replay.source_materials[0].checkout_path = repository_root;
        let replayed_application = replay.record_replayed_firmware(&archived).unwrap();
        assert_eq!(
            fs::read(&replayed_application).unwrap(),
            b"application bytes"
        );
        assert_eq!(replay.manifest.firmware.len(), 1);
        assert_eq!(
            replay.manifest.firmware[0]
                .replayed_from
                .as_ref()
                .expect("replay origin")
                .source_run_id,
            "source-run"
        );
        replay.finish(Vec::new()).unwrap();

        fs::remove_dir_all(source_directory).unwrap();
        let verified =
            super::super::verification::verify(&root, "esp32s31", Some("replay-run")).unwrap();
        assert_eq!(verified.verified_run_ids, ["replay-run"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replay_import_rejects_paths_outside_the_sealed_bundle() {
        assert!(archive::validate_replayed_source_path(Path::new("../application.bin")).is_err());
        assert!(archive::validate_replayed_source_path(Path::new("/tmp/application.bin")).is_err());
        assert!(archive::validate_replayed_source_path(Path::new("firmware/runtime.elf")).is_ok());
    }

    #[test]
    fn outcome_aggregation_is_fail_closed() {
        assert_eq!(aggregate_outcome([]), Outcome::Skipped);
        assert_eq!(aggregate_outcome([Outcome::Passed]), Outcome::Passed);
        assert_eq!(
            aggregate_outcome([Outcome::Passed, Outcome::Blocked]),
            Outcome::Blocked
        );
        assert_eq!(
            aggregate_outcome([Outcome::Failed, Outcome::Interrupted]),
            Outcome::Interrupted
        );
    }

    #[test]
    fn finish_writes_all_views_and_completes_manifest() {
        let root = temporary_directory("finish");
        let scenarios = failed_suite().scenarios;
        let (suite, completion) = integrated_session(&root).finish(scenarios).unwrap();
        assert_eq!(suite.outcome, Outcome::Failed);
        assert!(completion.suite_report.is_file());
        assert!(completion.junit_report.is_file());
        assert!(completion.html_report.is_file());
        assert!(completion.integrity_report.is_file());
        assert!(completion.history_report.is_file());
        assert!(completion.history_html.is_file());
        let history: super::super::history::HistoryReport =
            serde_json::from_slice(&fs::read(&completion.history_report).unwrap()).unwrap();
        assert_eq!(history.counts.runs, 1);
        let final_manifest: RunManifest = serde_json::from_slice(
            &fs::read(completion.run_directory.join("manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(final_manifest.state, RunState::Completed);
        assert!(final_manifest.finished_unix_millis.is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dropped_session_marks_manifest_interrupted() {
        let root = temporary_directory("interrupted");
        drop(session(&root));
        let final_manifest: RunManifest =
            serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(final_manifest.state, RunState::Interrupted);
        assert!(final_manifest.finished_unix_millis.is_some());
        assert!(root.join("integrity.json").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
