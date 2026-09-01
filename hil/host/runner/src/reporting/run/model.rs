//! Serialized vocabulary for one HIL invocation and its derived views.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::qualification::scenario::ImageClass;

pub(crate) const RUN_SCHEMA: u16 = 2;

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
    pub(super) const fn id(self) -> &'static str {
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
    pub(in crate::reporting) const fn id(self) -> &'static str {
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
    pub(in crate::reporting) const fn symbol(self) -> &'static str {
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

    pub(in crate::reporting) fn is_consistent(&self) -> bool {
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

pub(in crate::reporting) fn aggregate_outcome(
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
    pub(in crate::reporting) fn from_results(results: &[ScenarioResult]) -> Self {
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
pub(in crate::reporting) struct RepositoryProvenance {
    pub(in crate::reporting) commit: String,
    pub(in crate::reporting) dirty: bool,
    pub(in crate::reporting) workspace_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::reporting) struct FirmwareReplayOrigin {
    pub(in crate::reporting) source_run_id: String,
    pub(in crate::reporting) source_integrity_sha256: String,
    pub(in crate::reporting) firmware_repository: RepositoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) source_build_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct RunnerProvenance {
    pub(super) package: String,
    pub(super) version: String,
    pub(super) protocol_version: u16,
    pub(super) host_os: String,
    pub(super) host_arch: String,
    pub(super) tools: Vec<ToolVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct ToolVersion {
    pub(super) name: String,
    pub(super) version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::reporting) struct CellProvenance {
    pub(in crate::reporting) cell_id: String,
    pub(in crate::reporting) device_id: String,
    pub(in crate::reporting) serial_device: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::reporting) struct FirmwareArtifact {
    pub(in crate::reporting) image: ImageClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) replayed_from: Option<FirmwareReplayOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) build_provenance_path: Option<PathBuf>,
    pub(in crate::reporting) application_path: PathBuf,
    pub(in crate::reporting) application_size_bytes: u64,
    pub(in crate::reporting) application_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) runtime_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) runtime_elf_size_bytes: Option<u64>,
    pub(in crate::reporting) runtime_elf_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) runtime_bin_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) runtime_bin_size_bytes: Option<u64>,
    pub(in crate::reporting) runtime_bin_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) bootstrap_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) bootstrap_elf_size_bytes: Option<u64>,
    pub(in crate::reporting) bootstrap_elf_sha256: String,
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
pub(in crate::reporting) struct RunManifest {
    pub(in crate::reporting) schema: u16,
    pub(in crate::reporting) run_id: String,
    pub(in crate::reporting) target: String,
    pub(in crate::reporting) state: RunState,
    pub(in crate::reporting) started_unix_millis: u64,
    pub(in crate::reporting) finished_unix_millis: Option<u64>,
    pub(in crate::reporting) duration_millis: Option<u64>,
    pub(super) invocation: Vec<String>,
    pub(in crate::reporting) repository: RepositoryProvenance,
    pub(super) runner: RunnerProvenance,
    pub(in crate::reporting) cell: CellProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::reporting) lab_provenance_path: Option<PathBuf>,
    pub(in crate::reporting) firmware: Vec<FirmwareArtifact>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::reporting) struct IntegrityFile {
    pub(in crate::reporting) path: PathBuf,
    pub(in crate::reporting) size_bytes: u64,
    pub(in crate::reporting) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(in crate::reporting) struct IntegrityIndex {
    pub(in crate::reporting) schema: u16,
    pub(in crate::reporting) run_id: String,
    pub(in crate::reporting) files: Vec<IntegrityFile>,
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
