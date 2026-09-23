//! Revision-local selection. Names never select an occurrence implicitly.
use crate::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InspectionScope {
    Revision,
    Input { input: u64 },
    Object { input: u64, object: ObjectId },
    Symbol { input: u64, symbol: SymbolId },
}
impl InspectionScope {
    pub fn input(&self) -> Option<u64> {
        match self {
            Self::Revision => None,
            Self::Input { input } | Self::Object { input, .. } | Self::Symbol { input, .. } => {
                Some(*input)
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionKind {
    Object,
    Symbol,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRequest {
    pub kind: SelectionKind,
    pub name: Vec<u8>,
    pub input: Option<u64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionCandidate {
    pub revision: RevisionId,
    pub scope: InspectionScope,
    pub name: Vec<u8>,
    pub payload: Option<ArtifactId>,
}
/// Header borrowed before the first input callback; no input graph is materialized.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionHeader {
    pub project: ProjectId,
    pub parent: Option<RevisionId>,
    pub target: Target,
    pub inventory_producer: String,
}
