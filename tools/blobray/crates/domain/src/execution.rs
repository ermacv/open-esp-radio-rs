//! Concrete execution contracts. Scenarios are explicit inputs, never inferred facts.
use crate::*;

/// Native concrete request and manifest format.
pub const EXECUTION_SCHEMA: u32 = 4;
/// Maximum explicitly supplied RV32 ABI words per invocation.
pub const MAX_EXECUTION_ARGUMENT_WORDS: usize = 256;

/// Captured address space; each invocation selects its own entry within the mappings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionTarget {
    pub revision: RevisionId,
    pub source: FunctionSource,
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
    /// Exact guest PC in this target's captured address space.
    pub entry: u32,
    /// Already lowered RV32 integer ABI words: a0..a7, then ascending stack words.
    /// `None` and omitted register words remain unknown, including on a filled stack.
    /// Clients lower multiword/variadic arguments and insert ABI padding explicitly.
    pub arguments: Vec<Option<u32>>,
    pub memory: Vec<ExecutionRegion>,
    /// Explicit register-bank model: reads observe the latest written value.
    pub mmio: Vec<RegisterCell>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCase {
    pub name: String,
    pub reset: SessionReset,
    pub vendor: Invocation,
    pub replacement: Option<Invocation>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionReset {
    /// Recreate captured images and discard all previous mutable state/dependencies.
    Cold,
    /// Continue the preceding successful phase with session-owned regions.
    Warm,
}
/// Lifetime of a caller-declared RAM mapping. Images/stack have fixed owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegionLifetime {
    Phase,
    Session,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRegion {
    pub seed: MemorySeed,
    pub lifetime: RegionLifetime,
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
    Atomic,
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

    /// Read one aligned known word and replace this session's reservation.
    /// Unknown/inaccessible data returns None and leaves no reservation.
    fn load_reserved(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
    /// Check store permissions even without a reservation, then conditionally
    /// write a word. Some(false) is a failed reservation, None an invalid access.
    /// Every attempt clears the reservation, including failure.
    fn store_conditional(
        &mut self,
        address: u32,
        value: u32,
        order: ExecutionOrdering,
        control: &mut dyn RunControl,
    ) -> Result<Option<bool>>;
    /// One indivisible read/update/write of a known aligned writable word.
    /// Calls `update` exactly once on success, never on invalid/unknown memory.
    /// Returns the old word and invalidates overlapping reservations. The callback
    /// supplies pure ISA arithmetic; it must not acquire resources or call memory.
    fn modify_word(
        &mut self,
        address: u32,
        order: ExecutionOrdering,
        update: &mut dyn FnMut(u32) -> u32,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>>;
}
/// Ordering requested by the guest atomic instruction. A single-hart environment
/// executes memory operations in program order; this is not a concurrency model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionOrdering {
    pub acquire: bool,
    pub release: bool,
}
/// Concrete ISA capability; no input selection, repository, models or verdict authority.
pub trait Executor {
    fn identity(&self) -> &'static str;
    fn execute(
        &self,
        entry: u32,
        stack: u32,
        arguments: &[Option<u32>; 8],
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
        if self.schema != EXECUTION_SCHEMA
            || self.cases.is_empty()
            || self
                .cases
                .first()
                .is_some_and(|case| case.reset != SessionReset::Cold)
            || self.cases.len() > 128
            || self.max_events == 0
            || self.max_events > 65536
            || self.replacement.is_some() != self.binding.is_some()
        {
            return Err(bad());
        }
        for target in std::iter::once(&self.vendor).chain(&self.replacement) {
            if target.companions.len() > 64 {
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
            for (input, target) in std::iter::once((&case.vendor, &self.vendor))
                .chain(case.replacement.as_ref().zip(self.replacement.as_ref()))
            {
                if input.entry & 1 != 0 || input.entry >= u32::MAX - 1 {
                    return Err(bad());
                }
                input.entry_stack(&target.stack)?;
                if input.memory.len() > 128 || input.mmio.len() > 1024 {
                    return Err(bad());
                }
                for (index, region) in input.memory.iter().enumerate() {
                    let seed = &region.seed;
                    seed.validate()?;
                    if input.memory[..index].iter().any(|r| {
                        let s = &r.seed;
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
impl Invocation {
    /// Initial integer argument registers; absent words are unknown, never zero.
    pub fn register_arguments(&self) -> [Option<u32>; 8] {
        let mut registers = [None; 8];
        for (dst, src) in registers.iter_mut().zip(&self.arguments) {
            *dst = *src;
        }
        registers
    }

    /// Validate the argument/stack geometry and return the aligned entry SP.
    /// Stack words occupy an upward-growing prefix at SP of a 16-byte rounded
    /// area at the top of the declared stack. Remaining bytes below SP are the
    /// callee's stack capacity. No minimum callee frame size is inferred.
    pub fn entry_stack(&self, stack: &MemorySeed) -> Result<u32> {
        stack.validate()?;
        if self.arguments.len() > MAX_EXECUTION_ARGUMENT_WORDS {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "too many execution argument words",
            ));
        }
        let reserved = (self.arguments.len().saturating_sub(8) as u32 * 4).next_multiple_of(16);
        let top = stack.address + stack.length; // MemorySeed validates checked range.
        if stack.length < 16 || !top.is_multiple_of(16) || reserved > stack.length {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "execution arguments do not fit aligned stack",
            ));
        }
        Ok(top - reserved)
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
