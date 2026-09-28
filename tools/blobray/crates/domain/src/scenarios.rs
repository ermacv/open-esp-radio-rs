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
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
// One bounded admission message, not a resident collection of requests.
#[allow(clippy::large_enum_variant)]
pub enum ScenarioRequest {
    ProposeEffectContract {
        request: EffectProposalRequest,
    },
    ProposeProjection {
        request: ProjectionProposalRequest,
    },
    ProposeCallPair {
        request: CallPairProposalRequest,
    },
    Investigate {
        request: InvestigationRequest,
        producer: FunctionProducer,
    },
    Replay {
        execution: ArtifactId,
        producer: ExecutionProducer,
    },
}
