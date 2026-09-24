//! Structural flow over explicit saved interpretations; never executable evidence.
use crate::*;
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowQuery {
    pub scope: NavigationScope,
    pub root: FunctionAnalysisId,
    pub goal: FlowGoal,
    /// Maximum inter-function edges from the root (0 includes the root only).
    pub max_depth: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FlowGoal {
    Function {
        analysis: FunctionAnalysisId,
    },
    Effects {
        profile: FlowEffectProfile,
        address: Option<u32>,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowEffectProfile {
    Calls,
    Memory,
    All,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowHop {
    pub caller: FunctionAnalysisId,
    pub record: u64,
    pub callee: FunctionAnalysisId,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPath {
    pub hops: Vec<FlowHop>,
    pub purpose: String,
    pub applicability: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowFrontier {
    PartialAnalysis,
    UnresolvedCall,
    AmbiguousCall,
    OutsideSelection,
    Depth,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum FlowRecord {
    /// Parent links form one deterministic shortest structural witness, not every possible path.
    Function {
        function: NavigationFunction,
        depth: u32,
        parent: Option<FlowHop>,
    },
    Frontier {
        analysis: FunctionAnalysisId,
        record: Option<u64>,
        reason: FlowFrontier,
    },
    Call {
        observation: Box<NavigationRecord>,
    },
    Effect {
        analysis: FunctionAnalysisId,
        record: u64,
        fact: Box<FunctionRecord>,
        address_match: Option<bool>,
    },
    Unavailable {
        publication: PublicationId,
        member: Box<InvestigationMember>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSummary {
    pub schema: u32,
    pub request: FlowQuery,
    pub selected_analyses: u64,
    pub reached_analyses: u64,
    pub facts_passes: u64,
    pub effects: u64,
    pub frontiers: u64,
    pub unavailable_entries: u64,
    pub target_reached: Option<bool>,
}
impl ReviewedPath {
    pub fn allocated_bytes(&self) -> u64 {
        (self.hops.capacity() * std::mem::size_of::<FlowHop>()) as u64
            + self.purpose.capacity() as u64
            + self.applicability.capacity() as u64
            + self
                .hops
                .iter()
                .map(|h| h.caller.allocated_bytes() + h.callee.allocated_bytes())
                .sum::<u64>()
    }
}

impl FlowQuery {
    pub fn allocated_bytes(&self) -> u64 {
        self.root.allocated_bytes()
            + self.scope.revision.allocated_bytes()
            + (self.scope.publications.capacity() * std::mem::size_of::<PublicationId>()) as u64
            + self
                .scope
                .publications
                .iter()
                .map(PublicationId::allocated_bytes)
                .sum::<u64>()
            + (self.scope.analyses.capacity() * std::mem::size_of::<FunctionAnalysisId>()) as u64
            + self
                .scope
                .analyses
                .iter()
                .map(FunctionAnalysisId::allocated_bytes)
                .sum::<u64>()
            + self
                .scope
                .knowledge
                .as_ref()
                .map_or(0, KnowledgeRevisionId::allocated_bytes)
            + match &self.goal {
                FlowGoal::Function { analysis } => analysis.allocated_bytes(),
                _ => 0,
            }
    }
}
