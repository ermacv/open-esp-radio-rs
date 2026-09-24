//! Read-only navigation over explicitly selected retained research.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionLocation {
    pub source: FunctionSource,
    pub selector: FunctionSelector,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationScope {
    pub revision: RevisionId,
    pub publications: Vec<PublicationId>,
    pub analyses: Vec<FunctionAnalysisId>,
    /// None means no reviewed declarations, never the current head.
    pub knowledge: Option<KnowledgeRevisionId>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallDirection {
    Callers,
    Callees,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NavigationFilter {
    Functions {
        function: Option<FunctionLocation>,
    },
    /// None selects all transfers; a focus is an exact physical function.
    Calls {
        function: Option<FunctionLocation>,
        direction: CallDirection,
    },
    Object {
        occurrence: Box<KnowledgeOccurrence>,
        selector: DataSelector,
        access: Option<AccessDirection>,
    },
    Context {
        assertion: AssertionId,
        field: Option<ContextFieldKey>,
        access: Option<AccessDirection>,
        /// Required for unknown signatures; supplied known mappings must agree with the signature.
        arguments: Vec<ArgumentWord>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationQuery {
    pub scope: NavigationScope,
    pub filter: NavigationFilter,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationFunction {
    pub analysis: FunctionAnalysisId,
    pub location: FunctionLocation,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NavigationIssue {
    UnresolvedTarget,
    AmbiguousTarget,
    OutsideSelection,
    UnknownAddress,
    AmbiguousAddress,
    IndirectPath,
    MissingReference,
    ForeignOccurrence,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationMatch {
    /// Zero-based alternative in the observed value/path, never an arbitrarily chosen value.
    pub alternative: u8,
    /// Relative to the selected object span or field start.
    pub offset: i64,
    pub partial_overlap: bool,
    pub field: Option<String>,
    pub argument: Option<u8>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum NavigationRecord {
    Function {
        function: NavigationFunction,
        extent: CodeRange,
        address_space: CodeAddressSpace,
        coverage: FunctionCoverage,
        semantic_complete: Option<bool>,
    },
    Declaration {
        function: NavigationFunction,
        assertion: AssertionId,
        state: AssertionState,
    },
    Call {
        caller: NavigationFunction,
        record: u64,
        offset: u64,
        call: bool,
        target: AbstractValue,
        saved_resolution: Option<FunctionAnalysisId>,
        candidates: Vec<NavigationFunction>,
        focus_match: Option<bool>,
        issue: Option<NavigationIssue>,
    },
    Access {
        function: NavigationFunction,
        record: u64,
        origin: Option<FunctionAnalysisId>,
        offset: u64,
        access: MemoryKind,
        width: u8,
        address: AbstractValue,
        value: Option<AbstractValue>,
        matches: Vec<LocationMatch>,
        issue: Option<NavigationIssue>,
        path_issue: Option<AccessIssue>,
    },
    Unavailable {
        publication: PublicationId,
        member: Box<InvestigationMember>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationSummary {
    pub schema: u32,
    pub request: NavigationQuery,
    pub selected_analyses: u64,
    pub analyses_read: u64,
    pub unavailable_entries: u64,
    pub partial_analyses: u64,
    pub observations: u64,
    pub unresolved: u64,
    pub ambiguous: u64,
    pub data: Option<DataLocation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentWord {
    pub argument: u8,
    /// ABI word position: 0..7 are a0..a7, 8 is the first incoming stack word.
    pub word: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessDirection {
    Readers,
    Writers,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextFieldKey {
    pub argument: u8,
    pub name: String,
}
impl FunctionLocation {
    pub fn allocated_bytes(&self) -> u64 {
        self.source.allocated_bytes() + self.selector.object().artifact.allocated_bytes()
    }
}
impl NavigationFunction {
    pub fn allocated_bytes(&self) -> u64 {
        self.analysis.allocated_bytes() + self.location.allocated_bytes()
    }
}
