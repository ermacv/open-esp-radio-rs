//! Cooperative resource ports. These contain no platform clock or process policy.
use crate::*;

pub const WORK_POLICY: u32 = 1;
pub const DEFAULT_WORK_UNITS: u64 = 1_000_000_000;
pub const WORK_BLOCK: usize = 4096;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunPhase {
    PrepareObject,
    PrepareSection,
    PlanInvestigation,
    IndexResearch,
    LoadResearch,
    ComposeResearch,
    Execute,
    Compare,
    Materialize,
    Link,
    ValidateImage,
    AnalyzeFunction,
    AnalyzeValues,
    #[default]
    Starting,
    Capture,
    ReadCaptured,
    Members,
    Elf,
    ValidateRevision,
    Serialize,
    Retain,
    Publish,
}

/// Physical position, not arbitrary input text. Ordinals are zero based.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPosition {
    pub phase: RunPhase,
    pub input: Option<u64>,
    pub member: Option<u64>,
    pub table: Option<u64>,
    pub entry: Option<u64>,
    pub artifact: Option<[u8; 32]>,
}
impl RunPosition {
    pub fn artifact(&mut self, id: &ArtifactId) {
        let mut digest = [0; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            // ArtifactId has already validated lowercase hexadecimal encoding.
            *byte = u8::from_str_radix(&id.as_str()[i * 2..i * 2 + 2], 16).unwrap();
        }
        self.artifact = Some(digest);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopReason {
    Work,
    Deadline,
    Cancelled,
}

/// Fixed-size failure context can be formed without allocating a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlStop {
    pub reason: StopReason,
    pub requested_units: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunProgress {
    #[serde(default)]
    pub measurements: WorkMeasurements,
    #[serde(default)]
    pub phases: PhaseMeasurements,
    pub sequence: u64,
    pub position: RunPosition,
    pub work_used: u64,
    pub elapsed_ms: u64,
    pub stop: Option<ControlStop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_memory: Option<WorkingMemoryObservation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_storage: Option<TemporaryUsage>,
}

/// All costs are charged before work. A zero charge is still a checkpoint.
/// Closures adapt existing low-level callers; application operations supply a
/// metered implementation. Byte work is charged in bounded blocks per operation.
pub trait RunControl {
    fn measure(&mut self, _metric: WorkMetric, _amount: u64) {}
    /// Current accounting, when supplied by an application-owned run context.
    fn progress(&self) -> Option<RunProgress> {
        None
    }
    fn checkpoint(&mut self, units: u64) -> Result<()>;
    fn working_memory(&mut self, _observation: WorkingMemoryObservation) {}
    fn memory_phases(&mut self, _phases: &PhaseMeasurements) {}
    fn temporary_storage(&mut self, _observation: TemporaryUsage) {}
    fn position(&self) -> RunPosition {
        RunPosition::default()
    }
    fn set_position(&mut self, _position: RunPosition) {}
    fn phase(&mut self, phase: RunPhase) -> Result<()> {
        let mut position = self.position();
        position.phase = phase;
        self.set_position(position);
        self.checkpoint(0)
    }
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        self.checkpoint(bytes.div_ceil(WORK_BLOCK) as u64)
    }
}
impl<F: FnMut() -> Result<()>> RunControl for F {
    fn checkpoint(&mut self, _: u64) -> Result<()> {
        self()
    }
}

/// Host clock and observation capability; no library installs signal handlers.
pub trait RunEnvironment {
    fn now_ms(&self) -> u64;
    fn cancelled(&self) -> bool;
    fn observe(&self, progress: &RunProgress) -> Result<()>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressRecord {
    pub schema: u32,
    pub run: RunId,
    pub progress: RunProgress,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitObservation {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub cgroup_oom: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemorySource {
    SampledTreeRss,
    CgroupPeak,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryObservation {
    pub source: MemorySource,
    pub peak_bytes: u64,
}

/// Bounded observations, distinct from the primary Error on the run.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDiagnostics {
    /// Observations from image linking, distinct from the worker process exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linker: Option<crate::LinkerDiagnostics>,
    pub progress: Option<RunProgress>,
    pub exit: Option<ExitObservation>,
    pub memory: Option<MemoryObservation>,
    pub stderr_tail: Vec<u8>,
    pub stderr_truncated: bool,
    pub secondary: Vec<Error>,
    pub secondary_truncated: bool,
}
impl RunDiagnostics {
    pub fn secondary(&mut self, mut error: Error) {
        truncate_message(&mut error.message);
        if self.secondary.len() < 4 {
            self.secondary.push(error);
        } else {
            self.secondary_truncated = true;
        }
    }
}
/// Bound human prose independently of lossless artifact metadata.
pub fn truncate_message(message: &mut String) {
    if message.len() > 1024 {
        let mut end = 1024;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(" [truncated]");
    }
}
