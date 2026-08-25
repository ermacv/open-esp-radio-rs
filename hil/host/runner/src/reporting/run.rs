//! Canonical, immutable records for one host HIL invocation.

use std::{
    ffi::OsString,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, qualification::scenario::ImageClass};

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
    pub(crate) entries: Vec<PlanEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RepositoryProvenance {
    pub(super) commit: String,
    pub(super) dirty: bool,
    pub(super) workspace_sha256: String,
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
    pub(super) application_path: PathBuf,
    pub(super) application_size_bytes: u64,
    pub(super) application_sha256: String,
    pub(super) runtime_elf_sha256: String,
    pub(super) runtime_bin_sha256: String,
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
    target_directory: PathBuf,
    directory: PathBuf,
    manifest: RunManifest,
    started: Instant,
    events: File,
    finished: bool,
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
        let repository = repository_provenance(root)?;
        let runner = runner_provenance();
        let run_id = create_run_id(started_unix_millis);
        let target_directory = root.join("target/hil").join(target);
        let runs = target_directory.join("runs");
        fs::create_dir_all(&runs)?;
        let directory = create_unique_directory(&runs, &run_id)?;
        let run_id = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("HIL run directory does not have a UTF-8 name")?
            .to_owned();
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
            firmware: Vec::new(),
        };
        atomic_json(&directory.join("manifest.json"), &manifest)?;
        let mut session = Self {
            target_directory,
            directory,
            manifest,
            started,
            events,
            finished: false,
        };
        session.record_event("run-started", None, None, None)?;
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

    pub(crate) fn record_firmware(
        &mut self,
        image: ImageClass,
        application: &Path,
        runtime_elf: &Path,
        runtime_bin: &Path,
        bootstrap_elf: &Path,
    ) -> Result<PathBuf> {
        let application_path = PathBuf::from("firmware")
            .join(image.id())
            .join("application.bin");
        let archived_application = self.directory.join(&application_path);
        let application_size_bytes = archive_file(application, &archived_application)?;
        let artifact = FirmwareArtifact {
            image,
            application_path,
            application_size_bytes,
            application_sha256: sha256_file(&archived_application)?,
            runtime_elf_sha256: sha256_file(runtime_elf)?,
            runtime_bin_sha256: sha256_file(runtime_bin)?,
            bootstrap_elf_sha256: sha256_file(bootstrap_elf)?,
        };
        self.manifest.firmware.retain(|entry| entry.image != image);
        self.manifest.firmware.push(artifact);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        Ok(archived_application)
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
            render_junit(&suite, &self.manifest).as_bytes(),
        )?;
        atomic_write(
            &self.directory.join("report.html"),
            render_html(&suite, &self.manifest).as_bytes(),
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

pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub(crate) fn unix_millis() -> Result<u64> {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    u64::try_from(millis).map_err(|_| "host timestamp exceeds the HIL report range".into())
}

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn collect_attachments(
    output: &Path,
    artifact_directory: &Path,
) -> Result<Vec<Attachment>> {
    let mut attachments = Vec::new();
    collect_attachments_below(output, Path::new(""), artifact_directory, &mut attachments)?;
    Ok(attachments)
}

fn collect_attachments_below(
    directory: &Path,
    relative: &Path,
    artifact_directory: &Path,
    attachments: &mut Vec<Attachment>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_attachments_below(
                &entry.path(),
                &child_relative,
                artifact_directory,
                attachments,
            )?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            attachments.push(Attachment {
                path: artifact_directory.join(&child_relative),
                media_type: attachment_media_type(&child_relative).to_owned(),
                size_bytes: metadata.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "HIL artifact is neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn attachment_media_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("json") => "application/json",
        Some("jsonl") => "application/x-ndjson",
        Some("pcap") | Some("pcapng") => "application/vnd.tcpdump.pcap",
        Some("html") => "text/html",
        Some("md") => "text/markdown",
        Some("log") | Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
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

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("report path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let counter = UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("report path has no file name: {}", path.display()))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn write_integrity_index(directory: &Path, run_id: &str) -> Result<PathBuf> {
    let path = directory.join("integrity.json");
    let index = IntegrityIndex {
        schema: RUN_SCHEMA,
        run_id: run_id.to_owned(),
        files: collect_integrity_files(directory)?,
    };
    atomic_json(&path, &index)?;
    Ok(path)
}

pub(super) fn collect_integrity_files(directory: &Path) -> Result<Vec<IntegrityFile>> {
    let mut files = Vec::new();
    collect_integrity_files_below(directory, Path::new(""), &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_integrity_files_below(
    directory: &Path,
    relative: &Path,
    files: &mut Vec<IntegrityFile>,
) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let child_relative = relative.join(entry.file_name());
        if file_type.is_dir() {
            collect_integrity_files_below(&entry.path(), &child_relative, files)?;
        } else if file_type.is_file() {
            if child_relative == Path::new("integrity.json") {
                continue;
            }
            files.push(IntegrityFile {
                path: child_relative,
                size_bytes: entry.metadata()?.len(),
                sha256: sha256_file(&entry.path())?,
            });
        } else {
            return Err(format!(
                "HIL run bundle contains neither a regular file nor a directory: {}",
                entry.path().display()
            )
            .into());
        }
    }
    Ok(())
}

fn archive_file(source: &Path, destination: &Path) -> Result<u64> {
    let source_metadata = fs::symlink_metadata(source)?;
    if !source_metadata.file_type().is_file() {
        return Err(format!(
            "firmware application is not a regular file: {}",
            source.display()
        )
        .into());
    }
    if destination.try_exists()? {
        return Err(format!(
            "firmware application is already archived: {}",
            destination.display()
        )
        .into());
    }
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "firmware archive path has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent)?;
    let counter = UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".application.bin.tmp-{}-{counter}",
        std::process::id()
    ));
    let result = (|| -> Result<u64> {
        let mut input = File::open(source)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let size = std::io::copy(&mut input, &mut output)?;
        output.flush()?;
        output.sync_all()?;
        fs::rename(&temporary, destination)?;
        File::open(parent)?.sync_all()?;
        Ok(size)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn repository_provenance(root: &Path) -> Result<RepositoryProvenance> {
    let commit = git_output(root, &["rev-parse", "HEAD"])?;
    let status = git_output(
        root,
        &["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    let tracked_diff = git_output_bytes(root, &["diff", "--binary", "HEAD", "--"])?;
    let untracked = git_output_bytes(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut digest = Sha256::new();
    digest.update(status.as_bytes());
    digest.update(&tracked_diff);
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        digest.update(path);
        let path = String::from_utf8(path.to_vec())?;
        digest.update(fs::read(root.join(path))?);
    }
    Ok(RepositoryProvenance {
        commit,
        dirty: !status.is_empty(),
        workspace_sha256: format!("{:x}", digest.finalize()),
    })
}

fn git_output(root: &Path, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git_output_bytes(root, arguments)?)?
        .trim()
        .to_owned())
}

fn git_output_bytes(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed with status {}",
            arguments.join(" "),
            output.status
        )
        .into());
    }
    Ok(output.stdout)
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

pub(super) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn render_junit(suite: &SuiteResult, manifest: &RunManifest) -> String {
    let mut tests = 0_usize;
    let mut failures = 0_usize;
    let mut errors = 0_usize;
    let mut skipped = 0_usize;
    for scenario in &suite.scenarios {
        if scenario.repetitions.is_empty() {
            tests += 1;
            match scenario.outcome {
                Outcome::Failed => failures += 1,
                Outcome::Broken | Outcome::Blocked | Outcome::Interrupted => errors += 1,
                Outcome::Skipped => skipped += 1,
                Outcome::Passed => {}
            }
            continue;
        }
        for repetition in &scenario.repetitions {
            tests += 1;
            match repetition.outcome {
                Outcome::Failed => failures += 1,
                Outcome::Broken | Outcome::Blocked | Outcome::Interrupted => errors += 1,
                Outcome::Skipped => skipped += 1,
                Outcome::Passed => {}
            }
        }
    }

    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        xml,
        "<testsuites name=\"open-esp-radio-hil\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{:.3}\">",
        suite.duration_millis as f64 / 1_000.0
    );
    let _ = writeln!(
        xml,
        "  <testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{:.3}\">",
        xml_escape(&suite.target),
        suite.duration_millis as f64 / 1_000.0
    );
    xml.push_str("    <properties>\n");
    let _ = writeln!(
        xml,
        "      <property name=\"run_id\" value=\"{}\"/>",
        xml_escape(&suite.run_id)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"git_commit\" value=\"{}\"/>",
        xml_escape(&manifest.repository.commit)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"git_dirty\" value=\"{}\"/>",
        manifest.repository.dirty
    );
    let _ = writeln!(
        xml,
        "      <property name=\"workspace_sha256\" value=\"{}\"/>",
        manifest.repository.workspace_sha256
    );
    let _ = writeln!(
        xml,
        "      <property name=\"cell_id\" value=\"{}\"/>",
        xml_escape(&manifest.cell.cell_id)
    );
    let _ = writeln!(
        xml,
        "      <property name=\"device_id\" value=\"{}\"/>",
        xml_escape(&manifest.cell.device_id)
    );
    xml.push_str("    </properties>\n");
    for scenario in &suite.scenarios {
        if scenario.repetitions.is_empty() {
            render_junit_case(&mut xml, scenario, None);
            continue;
        }
        for repetition in &scenario.repetitions {
            render_junit_case(&mut xml, scenario, Some(repetition));
        }
    }
    xml.push_str("  </testsuite>\n</testsuites>\n");
    xml
}

fn render_junit_case(
    xml: &mut String,
    scenario: &ScenarioResult,
    repetition: Option<&RepetitionResult>,
) {
    let name = repetition.map_or_else(
        || scenario.scenario.clone(),
        |repetition| {
            format!(
                "{}[repetition-{:03}]",
                scenario.scenario, repetition.repetition
            )
        },
    );
    let outcome = repetition.map_or(scenario.outcome, |repetition| repetition.outcome);
    let duration_millis = repetition.map_or(0, |repetition| repetition.duration_millis);
    let failure = repetition.map_or(scenario.failure.as_ref(), |repetition| {
        repetition.failure.as_ref()
    });
    let _ = writeln!(
        xml,
        "    <testcase classname=\"hil.{}\" name=\"{}\" time=\"{:.3}\">",
        scenario.image.id(),
        xml_escape(&name),
        duration_millis as f64 / 1_000.0
    );
    let message = failure.map_or("", |failure| failure.message.as_str());
    match outcome {
        Outcome::Passed => {}
        Outcome::Failed => {
            let kind = failure.map_or("scenario", |failure| failure.kind.id());
            let _ = writeln!(
                xml,
                "      <failure type=\"{}\" message=\"{}\">{}</failure>",
                xml_escape(kind),
                xml_escape(message),
                xml_escape(message)
            );
        }
        Outcome::Broken | Outcome::Blocked | Outcome::Interrupted => {
            let kind = failure.map_or("infrastructure", |failure| failure.kind.id());
            let _ = writeln!(
                xml,
                "      <error type=\"{}\" message=\"{}\">{}</error>",
                xml_escape(kind),
                xml_escape(message),
                xml_escape(message)
            );
        }
        Outcome::Skipped => {
            let _ = writeln!(xml, "      <skipped message=\"{}\"/>", xml_escape(message));
        }
    }
    let mut system_output = String::new();
    if let Some(repetition) = repetition {
        let _ = writeln!(
            system_output,
            "artifacts={}",
            repetition.artifact_directory.display()
        );
        for measurement in &repetition.measurements {
            let _ = writeln!(
                system_output,
                "measurement.{}={} {}",
                measurement.name,
                measurement.value,
                measurement.unit.id(),
            );
        }
    }
    if !system_output.is_empty() {
        let _ = writeln!(
            xml,
            "      <system-out>{}</system-out>",
            xml_escape(system_output.trim_end())
        );
    }
    xml.push_str("    </testcase>\n");
}

fn render_html(suite: &SuiteResult, manifest: &RunManifest) -> String {
    let mut rows = String::new();
    for scenario in &suite.scenarios {
        let detail = scenario.failure.as_ref().map_or_else(
            || {
                let failures = scenario
                    .repetitions
                    .iter()
                    .filter_map(|entry| entry.failure.as_ref())
                    .map(|failure| failure.message.as_str())
                    .collect::<Vec<_>>();
                failures.join("; ")
            },
            |failure| failure.message.clone(),
        );
        let attachments = scenario
            .repetitions
            .iter()
            .flat_map(|repetition| repetition.attachments.iter())
            .map(|attachment| {
                let path = attachment.path.display().to_string();
                format!(
                    "<a href=\"{}\">{}</a>",
                    html_escape(&path),
                    html_escape(
                        &attachment
                            .path
                            .file_name()
                            .unwrap_or(attachment.path.as_os_str())
                            .to_string_lossy(),
                    )
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        let measurements = scenario
            .repetitions
            .iter()
            .flat_map(|repetition| {
                repetition.measurements.iter().map(move |measurement| {
                    let threshold = measurement.threshold.map_or_else(String::new, |threshold| {
                        format!(
                            " ({} {} {})",
                            threshold.comparison.symbol(),
                            threshold.value,
                            measurement.unit.id(),
                        )
                    });
                    let class = if measurement.verdict == Some(MeasurementVerdict::Failed) {
                        "fail"
                    } else {
                        ""
                    };
                    format!(
                        "<span class=\"{class}\"><code>{}</code>={} {}{threshold}</span>",
                        html_escape(&measurement.name),
                        measurement.value,
                        measurement.unit.id(),
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("<br>");
        let _ = writeln!(
            rows,
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{:?}</td><td>{}/{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            html_escape(&scenario.scenario),
            scenario.image.id(),
            if scenario.outcome.is_passed() {
                "pass"
            } else {
                "fail"
            },
            scenario.outcome,
            scenario
                .repetitions
                .iter()
                .filter(|entry| entry.outcome.is_passed())
                .count(),
            scenario.required_repetitions,
            html_escape(&detail),
            if attachments.is_empty() {
                "&mdash;"
            } else {
                &attachments
            },
            if measurements.is_empty() {
                "&mdash;"
            } else {
                &measurements
            },
        );
    }
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>HIL {run}</title>\
         <style>body{{font:14px system-ui,sans-serif;margin:2rem;max-width:1200px}}\
         table{{border-collapse:collapse;width:100%}}th,td{{border:1px solid #bbb;padding:.45rem;text-align:left}}\
         .pass{{color:#087830;font-weight:700}}.fail{{color:#b42318;font-weight:700}}code{{background:#eee;padding:.1rem .25rem}}</style>\
         </head><body><h1>Open ESP radio HIL</h1>\
         <p>Run <code>{run}</code> · cell <code>{cell}</code> · device <code>{device}</code> · commit <code>{commit}</code>{dirty}</p>\
         <p>Outcome: <strong class=\"{class}\">{outcome:?}</strong> · {passed}/{total} scenarios passed · {duration:.3} s</p>\
         <table><thead><tr><th>Scenario</th><th>Image</th><th>Outcome</th><th>Repetitions</th><th>Failure</th><th>Artifacts</th><th>Measurements</th></tr></thead>\
         <tbody>{rows}</tbody></table></body></html>\n",
        run = html_escape(&suite.run_id),
        cell = html_escape(&manifest.cell.cell_id),
        device = html_escape(&manifest.cell.device_id),
        commit = html_escape(&manifest.repository.commit),
        dirty = if manifest.repository.dirty {
            " · dirty workspace"
        } else {
            ""
        },
        class = if suite.outcome.is_passed() {
            "pass"
        } else {
            "fail"
        },
        outcome = suite.outcome,
        passed = suite.counts.passed,
        total = suite.counts.scenarios,
        duration = suite.duration_millis as f64 / 1_000.0,
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn html_escape(value: &str) -> String {
    xml_escape(value)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        manifest.state = RunState::Running;
        manifest.finished_unix_millis = None;
        manifest.duration_millis = None;
        atomic_json(&directory.join("manifest.json"), &manifest).unwrap();
        RunSession {
            target_directory: directory.to_owned(),
            directory: directory.to_owned(),
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

    #[test]
    fn junit_preserves_failure_and_escapes_xml() {
        let xml = render_junit(&failed_suite(), &manifest());
        roxmltree::Document::parse(&xml).expect("valid JUnit XML");
        assert!(xml.contains("tests=\"1\" failures=\"1\""));
        assert!(xml.contains("bad &lt;frame&gt; &amp; timeout"));
        assert!(xml.contains("run_id\" value=\"run&lt;&amp;&gt;"));
        assert!(xml.contains("repetition-001"));
        assert!(xml.contains("measurement.udp.rx.loss=2 count"));
    }

    #[test]
    fn html_is_derived_from_the_same_suite_record() {
        let html = render_html(&failed_suite(), &manifest());
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
        let run_directory = root.join("run");
        fs::create_dir(&run_directory).unwrap();
        let application = root.join("application.bin");
        let runtime_elf = root.join("runtime.elf");
        let runtime_bin = root.join("runtime.bin");
        let bootstrap_elf = root.join("bootstrap.elf");
        fs::write(&application, b"application bytes").unwrap();
        fs::write(&runtime_elf, b"runtime elf").unwrap();
        fs::write(&runtime_bin, b"runtime bin").unwrap();
        fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

        let mut session = session(&run_directory);
        session
            .record_firmware(
                ImageClass::Correctness,
                &application,
                &runtime_elf,
                &runtime_bin,
                &bootstrap_elf,
            )
            .unwrap();
        let artifact = &session.manifest.firmware[0];
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
        session.finished = true;
        drop(session);
        fs::remove_dir_all(root).unwrap();
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
