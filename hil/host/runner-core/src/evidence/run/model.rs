//! Serialized vocabulary for one HIL invocation and its derived views.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::image::ImageClass;

pub const RUN_SCHEMA: u16 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Running,
    Completed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Passed,
    Failed,
    Broken,
    Skipped,
    Blocked,
    Interrupted,
}

impl Outcome {
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Scenario,
    Precondition,
    ImageBuild,
    ImageFlash,
    Infrastructure,
}

impl FailureKind {
    pub const fn id(self) -> &'static str {
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
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeasurementUnit {
    Count,
    Bytes,
    BitsPerSecond,
    Microseconds,
    BasisPoints,
}

impl MeasurementUnit {
    pub const fn id(self) -> &'static str {
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
pub enum Comparison {
    AtLeast,
    AtMost,
    Exactly,
}

impl Comparison {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "&gt;=",
            Self::AtMost => "&lt;=",
            Self::Exactly => "=",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Threshold {
    pub comparison: Comparison,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeasurementVerdict {
    Passed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Measurement {
    pub name: String,
    pub value: u64,
    pub unit: MeasurementUnit,
    pub threshold: Option<Threshold>,
    pub verdict: Option<MeasurementVerdict>,
}

impl Measurement {
    pub fn observed(name: impl Into<String>, value: u64, unit: MeasurementUnit) -> Self {
        Self {
            name: name.into(),
            value,
            unit,
            threshold: None,
            verdict: None,
        }
    }

    pub fn evaluated(mut self, comparison: Comparison, threshold: u64) -> Self {
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

    pub(in crate::evidence) fn is_consistent(&self) -> bool {
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
pub struct RepetitionResult {
    pub schema: u16,
    pub repetition: u8,
    pub outcome: Outcome,
    pub started_unix_millis: u64,
    pub duration_millis: u64,
    pub artifact_directory: PathBuf,
    pub attachments: Vec<Attachment>,
    pub measurements: Vec<Measurement>,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScenarioResult {
    pub schema: u16,
    pub scenario: String,
    pub image: ImageClass,
    pub outcome: Outcome,
    pub required_repetitions: u8,
    pub repetitions: Vec<RepetitionResult>,
    pub failure: Option<Failure>,
}

impl ScenarioResult {
    pub fn from_repetitions(
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

    pub fn blocked(
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

pub(in crate::evidence) fn aggregate_outcome(
    outcomes: impl IntoIterator<Item = Outcome>,
) -> Outcome {
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
pub struct SuiteCounts {
    pub scenarios: usize,
    pub passed: usize,
    pub failed: usize,
    pub broken: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub interrupted: usize,
}

impl SuiteCounts {
    pub fn from_results(results: &[ScenarioResult]) -> Self {
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
pub struct SuiteResult {
    pub schema: u16,
    pub run_id: String,
    pub target: String,
    pub outcome: Outcome,
    pub started_unix_millis: u64,
    pub finished_unix_millis: u64,
    pub duration_millis: u64,
    pub counts: SuiteCounts,
    pub scenarios: Vec<ScenarioResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanDisposition {
    Selected,
    Filtered,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanEntry {
    pub scenario: String,
    pub image: ImageClass,
    pub repetitions: u8,
    pub disposition: PlanDisposition,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements: Option<crate::lab::requirements::Requirements>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunPlan {
    pub schema: u16,
    pub run_id: String,
    pub selection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<PlannedFirmware>,
    pub entries: Vec<PlanEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "source")]
pub enum PlannedFirmware {
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
pub struct RepositoryProvenance {
    pub commit: String,
    pub dirty: bool,
    pub workspace_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FirmwareReplayOrigin {
    pub source_run_id: String,
    pub source_integrity_sha256: String,
    pub firmware_repository: RepositoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_build_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerProvenance {
    /// Identity captured by the running process and embedded at its own build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observer: Option<serde_json::Value>,
    pub package: String,
    pub version: String,
    pub protocol_version: u16,
    pub host_os: String,
    pub host_arch: String,
    pub tools: Vec<ToolVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolVersion {
    pub(super) name: String,
    pub(super) version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CellProvenance {
    pub cell_id: String,
    pub device_id: String,
    pub serial_device: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FirmwareArtifact {
    pub image: ImageClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed_from: Option<FirmwareReplayOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_provenance_path: Option<PathBuf>,
    pub application_path: PathBuf,
    pub application_size_bytes: u64,
    pub application_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_elf_size_bytes: Option<u64>,
    pub runtime_elf_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_bin_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_bin_size_bytes: Option<u64>,
    pub runtime_bin_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_elf_size_bytes: Option<u64>,
    pub bootstrap_elf_sha256: String,
}

#[derive(Serialize)]
pub(super) struct EventRecord<'a> {
    pub(super) timestamp_unix_millis: u64,
    pub(super) kind: &'a str,
    pub(super) scenario: Option<&'a str>,
    pub(super) image: Option<ImageClass>,
    pub(super) outcome: Option<Outcome>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunManifest {
    pub schema: u16,
    pub run_id: String,
    pub target: String,
    pub state: RunState,
    pub started_unix_millis: u64,
    pub finished_unix_millis: Option<u64>,
    pub duration_millis: Option<u64>,
    pub(super) invocation: Vec<String>,
    pub repository: RepositoryProvenance,
    pub(super) runner: RunnerProvenance,
    pub cell: CellProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lab_provenance_path: Option<PathBuf>,
    pub firmware: Vec<FirmwareArtifact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::evidence) struct IntegrityFile {
    pub(in crate::evidence) path: PathBuf,
    pub(in crate::evidence) size_bytes: u64,
    pub(in crate::evidence) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::evidence) struct IntegrityIndex {
    pub(in crate::evidence) schema: u16,
    pub(in crate::evidence) run_id: String,
    pub(in crate::evidence) files: Vec<IntegrityFile>,
}

#[derive(Debug, Serialize)]
pub struct CompletionReport {
    pub schema: u16,
    pub run_id: String,
    pub outcome: Outcome,
    pub run_directory: PathBuf,
    pub suite_report: PathBuf,
    pub junit_report: PathBuf,
    pub html_report: PathBuf,
    pub integrity_report: PathBuf,
    pub history_report: Option<PathBuf>,
    pub history_html: Option<PathBuf>,
    pub history_failure: Option<String>,
}
