//! Persisted journal and publication receipt encodings. Platform interpretation belongs to the host.
use blobray_domain::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RunOperation {
    #[default]
    Import,
    PrepareImage {
        revision: RevisionId,
        plan: LinkPlanId,
    },
    AnalyzeFunction {
        request: FunctionRequest,
    },
    Investigate {
        revision: RevisionId,
        plan: InvestigationPlanId,
    },
    Knowledge {
        change: KnowledgeChange,
    },
    Execute {
        request: ExecutionRequest,
        producer: ExecutionProducer,
    },
    Query,
}

/// Linux process identity includes boot and start time, never PID alone.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub schema: u32,
    #[serde(default)]
    pub operation: RunOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PreparedImageId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub analysis: Option<FunctionAnalysisId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<PublicationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<KnowledgeRevisionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ArtifactId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<ComparisonVerdict>,
    pub id: RunId,
    pub state: RunState,
    pub owner: OwnerIdentity,
    pub budget: ResourceBudget,
    pub base: Option<RevisionId>,
    pub revision: Option<RevisionId>,
    pub complete: Option<bool>,
    pub error: Option<Error>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<RunDiagnostics>,
}

/// Receipt produced only after a worker validates its manifest and payload closure.
/// Large inventory data stays in staging, never in a supervisor message.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImport {
    pub schema: u32,
    pub project: ProjectId,
    pub parent: Option<RevisionId>,
    pub revision: RevisionId,
    pub closure: ArtifactId,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImageReceipt {
    pub schema: u32,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub plan: LinkPlanId,
    pub image: PreparedImageId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFunctionReceipt {
    pub schema: u32,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub analysis: FunctionAnalysisId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedInvestigationReceipt {
    pub schema: u32,
    pub project: ProjectId,
    pub revision: RevisionId,
    pub plan: InvestigationPlanId,
    pub publication: PublicationId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedKnowledgeReceipt {
    pub schema: u32,
    pub project: ProjectId,
    pub revision: KnowledgeRevisionId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedExecutionReceipt {
    pub schema: u32,
    pub project: ProjectId,
    pub execution: ArtifactId,
}
