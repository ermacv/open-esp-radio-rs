//! Immutable investigation selection and publication records. No scheduling or I/O.
use crate::*;
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationRequest {
    pub revision: Option<RevisionId>,
    /// Prepared image selection excludes input selection and source-object review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PreparedImageId>,
    /// None selects every input in the frozen revision; an empty explicit list is invalid.
    pub inputs: Option<Vec<u64>>,
    #[serde(default)]
    pub extents: Vec<FunctionExtent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewed_extents: Vec<ReviewedExtent>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionExtent {
    pub source: FunctionSource,
    pub symbol: SymbolId,
    pub extent: CodeRange,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionProducer {
    pub decoder: String,
    pub semantics: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationRecipe {
    pub schema: u32,
    pub policy: u32,
    pub project: ProjectId,
    pub request: InvestigationRequest,
    pub producer: FunctionProducer,
    /// Digest of canonical, newline-delimited PlanEntry records.
    pub entries: ArtifactId,
    pub entry_count: u64,
    pub functions: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationPlan {
    pub id: InvestigationPlanId,
    pub recipe: InvestigationRecipe,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PlanEntry {
    Image {
        image: PreparedImageId,
        payload: ArtifactId,
        synthetic: bool,
    },
    ImageGap {
        image: PreparedImageId,
        reason: String,
    },
    Input {
        input: u64,
        role: String,
    },
    Object {
        input: u64,
        object: ObjectId,
        payload: Option<ArtifactId>,
        supported: bool,
    },
    Function {
        request: FunctionRequest,
        name: Option<Vec<u8>>,
        payload: ArtifactId,
        address_space: CodeAddressSpace,
        declared_extent: CodeRange,
    },
    Gap {
        input: u64,
        object: Option<ObjectId>,
        reason: String,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InvestigationOutcome {
    Recorded,
    Blocked {
        error: Error,
    },
    Analyzed {
        analysis: FunctionAnalysisId,
        complete: bool,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationMember {
    pub entry: PlanEntry,
    pub outcome: InvestigationOutcome,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationCoverage {
    pub images: u64,
    pub inputs: u64,
    pub objects: u64,
    pub functions: u64,
    pub analyzed: u64,
    pub complete_functions: u64,
    pub blocked: u64,
    pub gaps: u64,
}
impl InvestigationCoverage {
    pub fn complete(self) -> bool {
        self.blocked == 0 && self.gaps == 0 && self.functions == self.complete_functions
    }
    pub fn include(&mut self, member: &InvestigationMember) {
        match member.entry {
            PlanEntry::Image { .. } => self.images += 1,
            PlanEntry::ImageGap { .. } => self.gaps += 1,
            PlanEntry::Input { .. } => self.inputs += 1,
            PlanEntry::Object { .. } => self.objects += 1,
            PlanEntry::Function { .. } => self.functions += 1,
            PlanEntry::Gap { .. } => self.gaps += 1,
        }
        match member.outcome {
            InvestigationOutcome::Recorded => {}
            InvestigationOutcome::Blocked { .. } => self.blocked += 1,
            InvestigationOutcome::Analyzed { complete, .. } => {
                self.analyzed += 1;
                self.complete_functions += u64::from(complete);
            }
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationManifest {
    pub schema: u32,
    pub plan: InvestigationPlan,
    pub members: ArtifactId,
    pub coverage: InvestigationCoverage,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvestigationStatus {
    pub revision: Option<RevisionId>,
    pub publication: Option<PublicationId>,
    pub publication_revision: Option<RevisionId>,
    pub current: bool,
    #[serde(default)]
    pub knowledge: Option<KnowledgeRevisionId>,
    pub coverage: Option<InvestigationCoverage>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InvestigationFilter {
    #[default]
    Members,
    Functions {
        name: Option<String>,
        address: Option<u32>,
    },
    Calls {
        caller: Option<u32>,
        callee: Option<u32>,
        unresolved_only: bool,
    },
    Accesses {
        address: Option<u32>,
        symbol: Option<SymbolId>,
        unknown_only: bool,
    },
    References {
        symbol: Option<SymbolId>,
        address: Option<u32>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InvestigationFinding {
    pub publication: PublicationId,
    pub request: FunctionRequest,
    pub analysis: FunctionAnalysisId,
    pub record: FunctionRecord,
}
