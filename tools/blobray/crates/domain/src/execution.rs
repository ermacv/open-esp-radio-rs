//! Concrete execution contracts. Scenarios are explicit inputs, never inferred facts.
use crate::*;

/// Native concrete request and manifest format.
pub const EXECUTION_SCHEMA: u32 = 21;
/// Upper bound of one canonical execution request payload. Requests are retained
/// by identity; control messages, journal rows and manifests carry only the hash.
pub const MAX_EXECUTION_REQUEST_BYTES: usize = 16 * 1024 * 1024;
/// Maximum phases in one request; a whole finite matrix fits in one request.
pub const MAX_EXECUTION_CASES: usize = 4096;
/// Maximum recorded events of one execution phase. A bounded poll loop that
/// exhausts a 100,000-operation budget must be observable without truncation.
pub const MAX_EXECUTION_EVENTS: u32 = 1 << 20;
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
    pub goal: ExecutionGoal,
    /// Already lowered RV32 integer ABI words: a0..a7, then ascending stack words.
    /// `None` and omitted register words remain unknown, including on a filled stack.
    /// Every other integer register except `ra`, `sp`, `gp` and `tp` starts at zero,
    /// so an isolated root can save callee-saved registers in its prologue.
    /// Clients lower multiword/variadic arguments and insert ABI padding explicitly.
    pub arguments: Vec<Option<u32>>,
    pub memory: Vec<ExecutionRegion>,
    pub models: Vec<DeviceDeclaration>,
    pub calls: Vec<CallDeclaration>,
    pub tables: Vec<RuntimeTable>,
    pub services: Vec<FifoService>,
    pub observe_memory: Vec<MemorySelection>,
    pub observe_calls: Option<CallCapture>,
    pub observe_timeline: TimelineCapture,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCase {
    pub name: String,
    pub reset: SessionReset,
    /// Fill byte of both sides' stack bytes the target's stack seed leaves
    /// unwritten in this case's phases, instead of the targets' own fill.
    /// Explicit seed bytes and argument words still take precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_fill: Option<u8>,
    pub relation: Option<ComparisonRelation>,
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
/// Physical code boundary in a source explicitly mapped by the execution target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSymbol {
    pub source: FunctionSource,
    pub symbol: SymbolId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ExecutionGoal {
    Return,
    ObserveDequeue {
        service: String,
        value: Option<u32>,
    },
    ReachSymbol {
        target: ExecutionSymbol,
    },
    /// Stop at transfer, before executing the callee. Ordinary calls use x1/x5;
    /// optionally include x0 tail transfers, excluding canonical ABI returns.
    ObserveCall {
        target: ExecutionSymbol,
        include_tail: bool,
    },
}
/// Validated physical boundary supplied by application; backend never resolves symbols.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedExecutionGoal {
    Return,
    /// Application validates the selected live service and signals its successful dequeue.
    ObserveDequeue,
    ReachSymbol {
        address: u32,
    },
    ObserveCall {
        address: u32,
        include_tail: bool,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct ExecutionStart {
    pub entry: u32,
    pub stack: u32,
    pub arguments: [Option<u32>; 8],
    pub goal: ResolvedExecutionGoal,
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
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionEvent {
    Memory {
        site: u32,
        transaction: MemoryTransaction,
    },
    Branch {
        site: u32,
        target: u32,
        fallthrough: u32,
        taken: bool,
    },
    CallTransfer {
        site: u32,
        target: u32,
        tail: bool,
        indirect: bool,
        stack: Option<u32>,
        target_kind: ObservedCallTarget,
        words: u16,
    },
    TransferArgument {
        word: u16,
        value: ObservedWord,
    },
    ServiceCall {
        instance: u16,
        binding: u16,
        site: u32,
        target: u32,
        tail: bool,
    },
    ServiceArgument {
        word: u16,
        value: Option<u32>,
    },
    ServiceInput {
        address: u32,
        width: u8,
        value: u32,
    },
    ServiceOutput {
        address: u32,
        width: u8,
        value: u32,
    },
    ServiceResult {
        instance: u16,
        transition: FifoTransition,
        depth: u32,
        words: [Option<u32>; 2],
    },
    RuntimeTable {
        instance: u16,
        event: RuntimeTableEvent,
    },
    ModeledCall {
        site: u32,
        target: u32,
        tail: bool,
        response: u32,
        boundary: CallBoundary,
    },
    CallArgument {
        word: u16,
        value: Option<u32>,
    },
    CallReturn {
        words: [Option<u32>; 2],
    },
    CallOutput {
        address: u32,
        width: u8,
        value: u32,
        scope: CallOutputScope,
    },
    Allocation {
        address: u32,
        requested: u32,
        capacity: u32,
        lifetime: RegionLifetime,
    },
    DelayMicros {
        value: u32,
    },
    Read {
        address: u32,
        width: u8,
        value: u32,
    },
    Write {
        address: u32,
        width: u8,
        value: u32,
    },
    Fence {
        predecessor: u8,
        successor: u8,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionStop {
    ObservedDequeue {
        instance: u16,
        value: u32,
    },
    Returned {
        low: Option<u32>,
        high: Option<u32>,
    },
    ReachedSymbol {
        pc: u32,
    },
    ObservedCall {
        pc: u32,
        target: u32,
        tail: bool,
    },
    /// Entry returned before reaching the requested non-return goal.
    GoalNotReached {
        low: Option<u32>,
        high: Option<u32>,
    },
    Incomplete {
        pc: u32,
        reason: ExecutionGap,
    },
    BlockedByPriorPhase,
}
impl ExecutionStop {
    /// Completion of the declared phase goal, independent of comparison verdict.
    pub fn completed(&self) -> bool {
        matches!(
            self,
            Self::Returned { .. }
                | Self::ReachedSymbol { .. }
                | Self::ObservedCall { .. }
                | Self::ObservedDequeue { .. }
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExecutionGap {
    FifoService {
        instance: u16,
        issue: FifoIssue,
    },
    RuntimeInterface {
        instance: Option<u16>,
        issue: RuntimeTableIssue,
    },
    CallModel {
        target: u32,
        issue: CallIssue,
    },
    UnsupportedInstruction,
    Memory {
        address: u32,
        access: MemoryAccess,
    },
    UnknownRegister {
        register: u8,
    },
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
    pub models: Vec<ModelObservation>,
    pub calls: Vec<CallObservation>,
    pub tables: Vec<RuntimeTableObservation>,
    pub services: Vec<FifoObservation>,
    pub final_memory: Vec<FinalMemoryChunk>,
}
impl ExecutionObservation {
    /// Goal reached with all environment obligations due at this boundary satisfied.
    pub fn completed(&self) -> bool {
        self.stop.completed()
            && self
                .services
                .iter()
                .all(|o| o.status != ModelStatus::Incomplete && o.status == o.expected_status())
            && self
                .models
                .iter()
                .all(|m| m.status == m.expected_status() && m.status != ModelStatus::Incomplete)
            && self
                .calls
                .iter()
                .all(|m| m.status == m.expected_status() && m.status != ModelStatus::Incomplete)
            && self
                .tables
                .iter()
                .all(|m| m.status == m.expected_status() && m.status != ModelStatus::Incomplete)
    }
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
    pub effect_claim: Option<EffectClaimCeiling>,
    pub effect_gap: Option<EffectGap>,
    pub verdict: ComparisonVerdict,
    /// First established difference in a selected observation domain.
    pub difference: Option<ComparisonDifference>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionManifest {
    pub effect_contracts: Vec<ResolvedEffectContract>,
    pub projections: Vec<ResolvedProjection>,
    pub call_pairs: Vec<ResolvedCallPair>,
    pub schema: u32,
    pub project: ProjectId,
    /// Identity of the retained canonical request payload.
    pub request: ArtifactId,
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
    /// Identify the current instruction independently of progress/reporting state.
    fn instruction(&mut self, pc: u32);
    /// Observe an eligible transfer before goal completion or dispatch; never execute a model.
    fn observe_call(&mut self, input: &CallInput, control: &mut dyn RunControl) -> Result<()>;
    fn call(&mut self, input: &CallInput, control: &mut dyn RunControl) -> Result<CallDispatch>;
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionOrdering {
    pub acquire: bool,
    pub release: bool,
}
/// Concrete ISA capability; no input selection, repository, models or verdict authority.
pub trait Executor {
    fn identity(&self) -> &'static str;
    fn execute(
        &self,
        start: &ExecutionStart,
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
            || self.cases.len() > MAX_EXECUTION_CASES
            || self.max_events == 0
            || self.max_events > MAX_EXECUTION_EVENTS
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
            match (&case.relation, &case.replacement) {
                (Some(relation), Some(other)) => relation.validate(&case.vendor, other)?,
                (None, None) => {}
                _ => return Err(bad()),
            }
            if let Some(other) = &case.replacement
                && std::mem::discriminant(&case.vendor.goal) != std::mem::discriminant(&other.goal)
            {
                return Err(bad());
            }
            for (input, target) in std::iter::once((&case.vendor, &self.vendor))
                .chain(case.replacement.as_ref().zip(self.replacement.as_ref()))
            {
                if input.entry & 1 != 0 || input.entry >= u32::MAX - 1 {
                    return Err(bad());
                }
                match &input.goal {
                    ExecutionGoal::Return => {}
                    ExecutionGoal::ObserveDequeue { service, .. } => {
                        if service.trim().is_empty() || service.len() > 128 || !service.is_ascii() {
                            return Err(bad());
                        }
                    }
                    ExecutionGoal::ReachSymbol { target: point }
                    | ExecutionGoal::ObserveCall { target: point, .. } => {
                        if point.symbol.object.location != ObjectLocation::Standalone
                            || (point.source != target.source
                                && !matches!(&point.source,
                                FunctionSource::Input { input } if target.companions.contains(input)))
                        {
                            return Err(bad());
                        }
                    }
                }
                input.entry_stack(&target.stack)?;
                input.validate_memory_selection()?;
                if let Some(capture) = &input.observe_calls {
                    capture.validate()?;
                }
                if input.services.len() > MAX_FIFO_SERVICES {
                    return Err(bad());
                }
                for (index, service) in input.services.iter().enumerate() {
                    service.validate()?;
                    if input.services[..index]
                        .iter()
                        .any(|s| s.id == service.id || s.handle == service.handle)
                    {
                        return Err(bad());
                    }
                }
                if input.memory.len() > 128 || input.models.len() > MAX_DEVICE_MODELS {
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
                if input.tables.len() > MAX_RUNTIME_TABLES {
                    return Err(bad());
                }
                for (i, table) in input.tables.iter().enumerate() {
                    table.validate()?;
                    if input.tables[..i].iter().any(|t| t.id == table.id) {
                        return Err(bad());
                    }
                }
                if input.calls.len() > MAX_CALL_MODELS {
                    return Err(bad());
                }
                for (index, call) in input.calls.iter().enumerate() {
                    call.validate()?;
                    if input.calls[..index]
                        .iter()
                        .any(|m| m.id == call.id || m.binding.address == call.binding.address)
                    {
                        return Err(bad());
                    }
                }
                for (index, model) in input.models.iter().enumerate() {
                    model.validate()?;
                    if input.models[..index].iter().any(|m| m.id == model.id) {
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
    FinalMemory {
        case: u32,
        replacement: bool,
        chunk: FinalMemoryChunk,
    },
    FifoService {
        case: u32,
        replacement: bool,
        observation: FifoObservation,
    },
    RuntimeTable {
        case: u32,
        replacement: bool,
        observation: RuntimeTableObservation,
    },
    CallModel {
        case: u32,
        replacement: bool,
        observation: CallObservation,
    },
    Model {
        case: u32,
        replacement: bool,
        observation: ModelObservation,
    },
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
    /// After the last case, the vendor side's records and then the
    /// replacement's: code that side reached over the whole execution, split
    /// into ascending, disjoint address ranges that each fit one record.
    Coverage {
        replacement: bool,
        coverage: ExecutionCoverage,
    },
}
/// Executed instructions in one coverage record; with the branch bound, one
/// record stays within a control message.
pub const MAX_COVERAGE_RECORD_INSTRUCTIONS: usize = 1536;
/// Branches in one coverage record.
pub const MAX_COVERAGE_RECORD_BRANCHES: usize = 512;
/// Indirect transfers in one coverage record.
pub const MAX_COVERAGE_RECORD_TRANSFERS: usize = 256;

/// Code one side reached in its executable captured segments over every phase
/// of an execution. Execution from writable RAM is not captured code.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionCoverage {
    /// Addresses of executed instructions, strictly ascending.
    pub instructions: Vec<u32>,
    /// Conditional branches with the directions they took, ascending by site.
    pub branches: Vec<BranchCoverage>,
    /// Distinct targets of executed indirect calls and jumps (not returns),
    /// ascending by site and target.
    pub transfers: Vec<IndirectTransfer>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndirectTransfer {
    pub site: u32,
    pub target: u32,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchCoverage {
    pub site: u32,
    pub taken: bool,
    pub fallthrough: bool,
}
impl ExecutionCoverage {
    /// Ordered, unique and self-consistent: every observed branch direction
    /// belongs to an executed instruction.
    pub fn validate(&self, c: &mut dyn RunControl) -> Result<()> {
        let invalid = || Error::new(ErrorCode::Integrity, "invalid execution coverage");
        c.checkpoint(
            (self.instructions.len() + self.branches.len() + self.transfers.len()) as u64 / 1024
                + 1,
        )?;
        if self.instructions.len() > MAX_COVERAGE_RECORD_INSTRUCTIONS
            || self.branches.len() > MAX_COVERAGE_RECORD_BRANCHES
            || self.transfers.len() > MAX_COVERAGE_RECORD_TRANSFERS
            || self.transfers.windows(2).any(|w| w[0] >= w[1])
            || self
                .transfers
                .iter()
                .any(|t| self.instructions.binary_search(&t.site).is_err())
            || self.instructions.iter().any(|pc| pc & 1 != 0)
            || self.instructions.windows(2).any(|w| w[0] >= w[1])
            || self.branches.windows(2).any(|w| w[0].site >= w[1].site)
            || self.branches.iter().any(|b| {
                !(b.taken || b.fallthrough) || self.instructions.binary_search(&b.site).is_err()
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    /// Split into ascending records within the record bounds. Each record
    /// holds the branches of its own instructions; empty coverage stays one
    /// empty record.
    pub fn split(self) -> Vec<ExecutionCoverage> {
        let mut records = Vec::new();
        let mut branches = self.branches.into_iter().peekable();
        let mut transfers = self.transfers.into_iter().peekable();
        let mut current = ExecutionCoverage::default();
        for pc in self.instructions {
            let branch = branches.next_if(|b| b.site == pc);
            let mut targets = Vec::new();
            while let Some(transfer) = transfers.next_if(|t| t.site == pc) {
                targets.push(transfer);
            }
            if current.instructions.len() == MAX_COVERAGE_RECORD_INSTRUCTIONS
                || (branch.is_some() && current.branches.len() == MAX_COVERAGE_RECORD_BRANCHES)
                || current.transfers.len() + targets.len() > MAX_COVERAGE_RECORD_TRANSFERS
            {
                records.push(std::mem::take(&mut current));
            }
            current.instructions.push(pc);
            current.branches.extend(branch);
            current.transfers.extend(targets);
        }
        if records.is_empty() || !current.instructions.is_empty() {
            records.push(current);
        }
        records
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    #[test]
    fn coverage_splits_into_bounded_ascending_records_that_keep_branches_with_their_sites() {
        let instructions: Vec<u32> = (0..MAX_COVERAGE_RECORD_INSTRUCTIONS as u32 + 3)
            .map(|i| 0x1000 + 4 * i)
            .collect();
        // More branches than one record holds, all before the instruction bound.
        let branches: Vec<BranchCoverage> = instructions[..MAX_COVERAGE_RECORD_BRANCHES + 1]
            .iter()
            .map(|site| BranchCoverage {
                site: *site,
                taken: true,
                fallthrough: false,
            })
            .collect();
        let whole = ExecutionCoverage {
            instructions: instructions.clone(),
            branches: branches.clone(),
            transfers: vec![],
        };
        let parts = whole.split();
        assert!(parts.len() >= 2);
        let mut last = None;
        for part in &parts {
            part.validate(&mut || Ok(())).unwrap();
            assert!(last.is_none_or(|l| part.instructions[0] > l));
            last = part.instructions.last().copied();
        }
        let rejoined: Vec<u32> = parts.iter().flat_map(|p| p.instructions.clone()).collect();
        assert_eq!(rejoined, instructions);
        let rejoined: Vec<_> = parts.iter().flat_map(|p| p.branches.clone()).collect();
        assert_eq!(rejoined, branches);
        assert_eq!(
            ExecutionCoverage::default().split(),
            vec![ExecutionCoverage::default()]
        );
    }
}
