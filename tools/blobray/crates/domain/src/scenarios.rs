//! Concrete user actions; no programmable workflow or nested jobs.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InvestigationInput {
    Plan {
        plan: InvestigationPlan,
    },
    Automatic {
        request: InvestigationRequest,
        producer: FunctionProducer,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchRequest {
    pub publication: PublicationId,
    pub name: Option<String>,
    pub address: Option<u32>,
    pub options: ResearchOptions,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterProposalRequest {
    pub analysis: FunctionAnalysisId,
    pub subject: SubjectId,
    pub register: MmioRegister,
    pub expected_base: Option<KnowledgeRevisionId>,
    pub actor: String,
    pub reason: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ScenarioRequest {
    Investigate {
        request: InvestigationRequest,
        producer: FunctionProducer,
    },
    Research {
        request: ResearchRequest,
    },
    ProposeRegister {
        request: RegisterProposalRequest,
    },
    Replay {
        execution: ArtifactId,
        producer: ExecutionProducer,
    },
}
