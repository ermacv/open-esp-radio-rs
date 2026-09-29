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
        occurrence: Box<Occurrence>,
        selector: DataSelector,
        access: Option<AccessDirection>,
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
    UnknownAddress,
    AmbiguousAddress,
    IndirectPath,
    MissingReference,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationMatch {
    /// Zero-based alternative in the observed value/path, never an arbitrarily chosen value.
    pub alternative: u8,
    /// Relative to the selected object span or field start.
    pub offset: i64,
    pub partial_overlap: bool,
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
    Call {
        caller: NavigationFunction,
        record: u64,
        offset: u64,
        call: bool,
        target: AbstractValue,
        candidates: Vec<NavigationFunction>,
        focus_match: Option<bool>,
        issue: Option<NavigationIssue>,
    },
    Access {
        function: NavigationFunction,
        record: u64,
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
#[serde(rename_all = "kebab-case")]
pub enum AccessDirection {
    Readers,
    Writers,
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
