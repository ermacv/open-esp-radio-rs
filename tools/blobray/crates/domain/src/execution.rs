//! Concrete execution contracts. Scenarios are explicit inputs, never inferred facts.
use crate::*;

/// Exact compiled entry; additional sources are explicitly mapped into the session.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub revision: RevisionId,
    pub source: FunctionSource,
    pub entry: u32,
    pub companions: Vec<u64>,
    pub abi: CallAbi,
    pub stack: MemorySeed,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySeed {
    pub address: u32,
    pub length: u32,
    /// None leaves unwritten bytes unknown; never silently zero-filled.
    pub fill: Option<u8>,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterCell {
    pub address: u32,
    pub width: u8,
    pub value: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub arguments: [u32; 8],
    pub memory: Vec<MemorySeed>,
    /// Explicit register-bank model: reads observe the latest written value.
    pub mmio: Vec<RegisterCell>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCase {
    pub name: String,
    pub vendor: Invocation,
    pub replacement: Option<Invocation>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseExecution {
    Independent,
    Stateful,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompiledBinding {
    ProductionEntry,
    SharedCore,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRequest {
    pub schema: u32,
    pub vendor: ExecutionTarget,
    pub replacement: Option<ExecutionTarget>,
    pub binding: Option<CompiledBinding>,
    pub cases: Vec<ExecutionCase>,
    pub case_execution: CaseExecution,
    /// Hard capacity; exhaustion is a resource failure, not truncated evidence.
    pub max_events: u32,
    pub compare_return: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionEvent {
    Read { address: u32, width: u8, value: u32 },
    Write { address: u32, width: u8, value: u32 },
    Fence { predecessor: u8, successor: u8 },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionStop {
    Returned { low: Option<u32>, high: Option<u32> },
    Incomplete { pc: u32, reason: ExecutionGap },
    BlockedByPriorPhase,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionGap {
    UnsupportedInstruction,
    Memory { address: u32, access: MemoryAccess },
    UnknownRegister { register: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryAccess {
    Fetch,
    Read,
    Write,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionObservation {
    pub stop: ExecutionStop,
    pub steps: u64,
    pub events: Vec<ExecutionEvent>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ComparisonVerdict {
    Match,
    Diff,
    Incomplete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseComparison {
    pub verdict: ComparisonVerdict,
    /// First differing event, or the common length when lengths differ.
    pub event: Option<u32>,
    pub return_difference: bool,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionManifest {
    pub schema: u32,
    pub project: ProjectId,
    pub request: ExecutionRequest,
    pub producer: ExecutionProducer,
    pub records: ArtifactId,
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
}
/// Implementations pinned at admission, including environment and comparison policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProducer {
    pub executor: String,
    pub environment: String,
    pub verifier: String,
}
/// Session-owned memory. None denotes unknown/inaccessible bytes, never zero.
pub trait ExecutionMemory {
    fn read(
        &mut self,
        address: u32,
        width: u8,
        access: MemoryAccess,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
    fn write(
        &mut self,
        address: u32,
        width: u8,
        value: u32,
        control: &mut dyn RunControl,
    ) -> Result<bool>;
    fn event(&mut self, event: ExecutionEvent, control: &mut dyn RunControl) -> Result<()>;
}
/// Concrete ISA capability; no input selection, repository, models or verdict authority.
pub trait Executor {
    fn identity(&self) -> &'static str;
    fn execute(
        &self,
        entry: u32,
        stack: u32,
        arguments: &[u32; 8],
        memory: &mut dyn ExecutionMemory,
        control: &mut dyn RunControl,
    ) -> Result<(ExecutionStop, u64)>;
}
impl ExecutionRequest {
    pub fn validate(&self) -> Result<()> {
        let bad = || {
            Error::new(
                ErrorCode::InvalidRequest,
                "invalid execution request or capacity",
            )
        };
        if self.schema != 1
            || self.cases.is_empty()
            || self.cases.len() > 128
            || self.max_events == 0
            || self.max_events > 65536
            || self.replacement.is_some() != self.binding.is_some()
        {
            return Err(bad());
        }
        for target in std::iter::once(&self.vendor).chain(&self.replacement) {
            if target.entry & 1 != 0 || target.entry >= u32::MAX - 1 || target.companions.len() > 64
            {
                return Err(bad());
            }
            target.stack.validate()?;
            if target.stack.length < 16
                || (u64::from(target.stack.address) + u64::from(target.stack.length)) % 16 != 0
            {
                return Err(bad());
            }
        }
        for case in &self.cases {
            if case.name.is_empty()
                || case.name.len() > 256
                || case.replacement.is_some() != self.replacement.is_some()
            {
                return Err(bad());
            }
            for input in std::iter::once(&case.vendor).chain(&case.replacement) {
                if input.memory.len() > 128 || input.mmio.len() > 1024 {
                    return Err(bad());
                }
                for (index, seed) in input.memory.iter().enumerate() {
                    seed.validate()?;
                    if input.memory[..index].iter().any(|s| {
                        u64::from(s.address) < u64::from(seed.address) + u64::from(seed.length)
                            && u64::from(seed.address) < u64::from(s.address) + u64::from(s.length)
                    }) {
                        return Err(bad());
                    }
                }
                for cell in &input.mmio {
                    if !matches!(cell.width, 1 | 2 | 4)
                        || cell.address % u32::from(cell.width) != 0
                        || u64::from(cell.address) + u64::from(cell.width) > u64::from(u32::MAX)
                        || (cell.width < 4 && cell.value >> (cell.width * 8) != 0)
                    {
                        return Err(bad());
                    }
                }
            }
        }
        Ok(())
    }
}
impl MemorySeed {
    pub fn validate(&self) -> Result<()> {
        if self.length == 0
            || self.bytes.len() as u64 > u64::from(self.length)
            || u64::from(self.address) + u64::from(self.length) >= u64::from(u32::MAX)
        {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "invalid memory seed range",
            ));
        }
        Ok(())
    }
}

/// Streaming evidence; each record is bounded independently of trace length.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExecutionEvidence {
    Event {
        case: u32,
        replacement: bool,
        event: ExecutionEvent,
    },
    Outcome {
        case: u32,
        replacement: bool,
        stop: ExecutionStop,
        steps: u64,
    },
    Comparison {
        case: u32,
        result: CaseComparison,
    },
}
