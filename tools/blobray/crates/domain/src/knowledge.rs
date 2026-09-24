//! Reviewed knowledge is separate from observations and imported source revisions.
use crate::*;

/// Caller-assigned stable semantic key, scoped by ProjectId. Never a physical selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SubjectId(String);
impl TryFrom<String> for SubjectId {
    type Error = Error;
    fn try_from(value: String) -> Result<Self> {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "subject key must contain 1..256 bytes without control characters",
            ));
        }
        Ok(Self(value))
    }
}
impl From<SubjectId> for String {
    fn from(value: SubjectId) -> Self {
        value.0
    }
}
impl SubjectId {
    pub fn allocated_bytes(&self) -> u64 {
        self.0.capacity() as u64
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeOccurrence {
    pub revision: RevisionId,
    pub source: FunctionSource,
    pub object: ObjectId,
    pub symbol: Option<SymbolId>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum KnowledgeClaim {
    CallPair {
        correspondence: Box<CallCorrespondence>,
    },
    EventRoute {
        route: Box<ReviewedEventRoute>,
    },
    Path {
        path: Box<ReviewedPath>,
    },
    Function {
        contract: Box<FunctionContract>,
    },
    Interface {
        contract: Box<InterfaceContract>,
    },
    PointerTable {
        selector: DataSelector,
        layout: PointerTable,
        purpose: String,
        applicability: String,
    },
    IntegerTable {
        selector: DataSelector,
        layout: IntegerTable,
        purpose: String,
        applicability: String,
    },
    Constant {
        analysis: FunctionAnalysisId,
        record: u64,
        operand: ConstantOperand,
        value: u32,
        purpose: String,
        applicability: String,
    },
    MmioRegion {
        region: MmioRegion,
    },
    MmioRegister {
        register: MmioRegister,
    },
    Name {
        name: String,
    },
    Binding,
    FunctionExtent {
        extent: CodeRange,
    },
    /// Reviewed explicit code bytes independent of symbol metadata.
    ExecutableRange {
        section: u32,
        extent: CodeRange,
    },
    Hypothesis {
        text: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum EvidenceRef {
    /// Preserved review/provenance bytes. Does not assert semantic correctness.
    Document {
        payload: ArtifactId,
    },
    /// A byte range in the selected occurrence's captured object payload.
    Source {
        payload: ArtifactId,
        range: CodeRange,
    },
    /// An optional zero-based record ordinal in a retained analysis of this occurrence.
    Analysis {
        analysis: FunctionAnalysisId,
        record: Option<u64>,
    },
    Publication {
        publication: PublicationId,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeProposal {
    pub subject: SubjectId,
    pub occurrence: KnowledgeOccurrence,
    pub claim: KnowledgeClaim,
    pub evidence: Vec<EvidenceRef>,
    pub note: Option<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewDecision {
    Accept,
    Reject,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
// One bounded admission message, not a resident collection of actions.
#[allow(clippy::large_enum_variant)]
pub enum KnowledgeAction {
    Propose {
        proposal: KnowledgeProposal,
    },
    Review {
        assertion: AssertionId,
        decision: ReviewDecision,
        supersedes: Option<AssertionId>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeChange {
    /// None means an empty knowledge history, not "whatever is current".
    pub expected_base: Option<KnowledgeRevisionId>,
    pub actor: String,
    pub reason: String,
    pub action: KnowledgeAction,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeManifest {
    pub schema: u32,
    pub project: ProjectId,
    pub change: KnowledgeChange,
    /// Identity of the proposed assertion, or the reviewed existing assertion.
    pub assertion: AssertionId,
    /// Durable roots needed to interpret the decision; not a disposable cache.
    pub evidence_roots: Vec<ArtifactId>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssertionState {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEntry {
    pub id: AssertionId,
    pub proposal: KnowledgeProposal,
    pub proposed_in: KnowledgeRevisionId,
    pub last_revision: KnowledgeRevisionId,
    pub state: AssertionState,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeStatus {
    pub revision: Option<KnowledgeRevisionId>,
    pub proposed: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub superseded: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedExtent {
    pub revision: KnowledgeRevisionId,
    pub assertion: AssertionId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEvent {
    pub revision: KnowledgeRevisionId,
    pub manifest: KnowledgeManifest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LegacyOutcome {
    Converted { revision: KnowledgeRevisionId },
    PreservedUnresolved { reason: String },
    Unsupported { reason: String },
    MissingPayload { reason: String },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRecord {
    pub origin: OriginPath,
    pub selector: String,
    pub capture: Capture,
    pub outcome: LegacyOutcome,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyManifest {
    pub schema: u32,
    pub project: ProjectId,
    pub source: OriginPath,
    pub records: ArtifactId,
    pub record_count: u64,
    pub converted: u64,
    pub missing: u64,
    pub unsupported: u64,
}
