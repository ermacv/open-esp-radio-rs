//! Function analysis values and ISA port, with explicit source/address identity.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FunctionSource {
    Input { input: u64 },
    Image { image: PreparedImageId },
}
impl FunctionSource {
    pub fn input(&self) -> Option<u64> {
        match self {
            Self::Input { input } => Some(*input),
            Self::Image { .. } => None,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeAddressSpace {
    Section,
    Image,
}

/// Immutable load bytes in the selected image's virtual address space. The
/// caller owns the borrowed view. Mutable, unmapped and ambiguous bytes are unknown.
pub trait ImageMemory {
    fn read_constant(
        &self,
        address: u32,
        bytes: &mut [u8],
        control: &mut dyn RunControl,
    ) -> Result<bool>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRange {
    pub start: u64,
    pub length: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRequest {
    #[serde(default)]
    pub research: Option<ResearchOptions>,
    pub revision: Option<RevisionId>,
    pub source: FunctionSource,
    pub symbol: SymbolId,
    #[serde(default)]
    pub extent: Option<CodeRange>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRecipe {
    pub research: Option<ResearchOptions>,
    pub abi: RiscvAbi,
    pub schema: u32,
    pub policy: u32,
    pub decoder: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<String>,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub source: FunctionSource,
    pub address_space: CodeAddressSpace,
    pub symbol: SymbolId,
    pub payload: ArtifactId,
    pub section: u32,
    pub extent: CodeRange,
    pub user_extent: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionCoverage {
    pub decoding: bool,
    pub control_flow: bool,
    pub references: bool,
}
impl Default for FunctionCoverage {
    fn default() -> Self {
        Self {
            decoding: true,
            control_flow: true,
            references: true,
        }
    }
}
impl FunctionCoverage {
    pub fn complete(self) -> bool {
        self.decoding && self.control_flow && self.references
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionManifest {
    pub schema: u32,
    pub recipe: FunctionRecipe,
    pub records: ArtifactId,
    pub coverage: FunctionCoverage,
    pub instructions: u64,
    pub blocks: u64,
    pub edges: u64,
    pub references: u64,
    pub gaps: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantics: Option<SemanticSummary>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceTarget {
    pub binding: u8,
    pub definition: SymbolDefinition,
    pub symbol: SymbolId,
    pub name: Vec<u8>,
    pub section: Option<u32>,
    pub offset: u64,
    pub symbol_type: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRelocation {
    pub section: u32,
    pub index: u64,
    pub offset: u64,
    pub relocation_type: u32,
    pub addend: Option<i64>,
    pub target: std::sync::Arc<ReferenceTarget>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceKind {
    Call,
    Branch,
    Address,
    Metadata,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct NormalizedReference {
    pub kind: ReferenceKind,
    pub target: std::sync::Arc<ReferenceTarget>,
    pub addend: Option<i64>,
    pub paired: Option<(u32, u64)>,
    pub known: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
/// `link` identifies the ABI link registers x1/x5, not every nonzero destination.
pub enum InstructionFlow {
    Next,
    Branch { displacement: i32 },
    Jump { displacement: i32, link: bool },
    Indirect { base: u8, offset: i32, link: bool },
    Stop,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecodedOp {
    pub length: u8,
    pub text: String,
    pub flow: InstructionFlow,
}
/// Structural classification when the semantic decoder does not model an encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedFlow {
    NonControl,
    Indirect,
    Unknown,
}
/// Receives captured bytes/structural records only. No project, filesystem or discovery authority.
pub trait FunctionDecoder {
    fn identity(&self) -> &'static str;
    /// Unknown is mandatory unless the ISA proves the encoding's control class.
    fn unsupported_flow(&self, _bytes: &[u8]) -> UnsupportedFlow {
        UnsupportedFlow::Unknown
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp>;
    /// `all` is the complete section table sorted by offset, preserving physical IDs.
    fn reference(
        &self,
        relocation: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        control: &mut dyn RunControl,
    ) -> Result<NormalizedReference>;
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FunctionRecord {
    MmioRange {
        offset: u64,
        assertion: AssertionId,
        region: MmioRegion,
    },
    Condition {
        offset: u64,
        test: BranchTest,
        left: AbstractValue,
        right: AbstractValue,
    },
    Expression {
        id: u32,
        offset: u64,
        origin: Option<FunctionAnalysisId>,
        expression: Expression,
    },
    CallInputs {
        offset: u64,
        registers: Vec<AbstractValue>,
    },
    ReturnValue {
        offset: u64,
        low: AbstractValue,
        high: AbstractValue,
    },
    Mmio {
        offset: u64,
        assertion: AssertionId,
        register: MmioRegister,
    },
    CalleeEffect {
        callsite: u64,
        analysis: FunctionAnalysisId,
        offset: u64,
        access: MemoryKind,
        width: u8,
        address: AbstractValue,
        value: Option<AbstractValue>,
    },
    CallResolution {
        offset: u64,
        analysis: Option<FunctionAnalysisId>,
        reason: Option<String>,
    },

    /// A call or out-of-function jump. A resolved target does not prove returns
    /// or effects; unresolved transfers remain visible.
    Transfer {
        offset: u64,
        target: AbstractValue,
        call: bool,
    },
    Value {
        offset: u64,
        register: u8,
        value: AbstractValue,
        relocation: Option<RelocationSite>,
    },
    MemoryAccess {
        offset: u64,
        access: MemoryKind,
        width: u8,
        address: AbstractValue,
        /// Value written, absent for reads. Unknown for an unmodeled atomic result.
        value: Option<AbstractValue>,
        relocation: Option<RelocationSite>,
    },
    SemanticGap {
        offset: u64,
        reason: SemanticGapReason,
    },
    Instruction {
        offset: u64,
        bytes: Vec<u8>,
        decoded: DecodedOp,
    },
    Block {
        id: u64,
        start: u64,
        end: u64,
    },
    Edge {
        from: u64,
        target: Option<u64>,
        relation: EdgeKind,
        external: bool,
    },
    Reference {
        raw: Box<FunctionRelocation>,
        reference_kind: ReferenceKind,
        target: std::sync::Arc<ReferenceTarget>,
        addend: Option<i64>,
        paired: Option<(u32, u64)>,
        known: bool,
    },
    Gap {
        start: u64,
        length: u64,
        reason: GapReason,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Taken,
    Fallthrough,
    Jump,
    Call,
    PossibleContinuation,
    Indirect,
    Return,
    Stop,
    Conflict,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    Unvisited,
    UnsupportedInstruction,
    ConflictingBoundary,
    DeclaredData,
}
pub trait FunctionSink {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolDefinition {
    Null,
    Undefined,
    Section,
    Absolute,
    Common,
    Other,
}
