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
pub enum ScenarioRequest {
    Investigate {
        request: InvestigationRequest,
        producer: FunctionProducer,
    },
}
