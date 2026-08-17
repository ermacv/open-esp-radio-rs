use serde::Serialize;

pub(crate) const MAX_VISITED_NODES: usize = 4_096;
pub(crate) const MAX_EXAMINED_EDGES: usize = 32_768;
pub(crate) const MAX_LOADED_FUNCTIONS: usize = 128;

#[derive(Clone, Debug)]
pub(crate) enum FlowInvestigationRequest<'a> {
    Target(TargetFlowRequest<'a>),
    Effects(EffectFlowRequest<'a>),
    EventRoute(EventRouteFlowRequest<'a>),
}

#[derive(Clone, Debug)]
pub(crate) enum FlowTargetRequest<'a> {
    Function(&'a str),
    Register(&'a str),
    Address(u32),
}

#[derive(Clone, Debug)]
pub(crate) struct TargetFlowRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) root_symbol: &'a str,
    pub(crate) target: FlowTargetRequest<'a>,
    pub(crate) max_depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FlowEffectKind {
    Delay,
    Timer,
    Event,
    Call,
    Queue,
    Mmio,
    Memory,
    All,
}

impl FlowEffectKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Delay => "delay",
            Self::Timer => "timer",
            Self::Event => "event",
            Self::Call => "call",
            Self::Queue => "queue",
            Self::Mmio => "mmio",
            Self::Memory => "memory",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EffectFlowRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) root_symbol: &'a str,
    pub(crate) kind: FlowEffectKind,
    pub(crate) max_depth: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct EventRouteFlowRequest<'a> {
    pub(crate) route: &'a str,
    pub(crate) max_depth: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum FlowStatus {
    Complete,
    Incomplete,
    NotReached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EvidenceLevel {
    Observed,
    Reviewed,
    Modeled,
    Executed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct FlowClaims {
    pub(crate) structural_navigation: bool,
    pub(crate) path_feasibility: bool,
    pub(crate) event_delivery: bool,
    pub(crate) executable_equivalence: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowLimits {
    pub(crate) max_depth: usize,
    pub(crate) max_visited_nodes: usize,
    pub(crate) max_examined_edges: usize,
    pub(crate) max_loaded_functions: usize,
    pub(crate) visited_nodes: usize,
    pub(crate) examined_edges: usize,
    pub(crate) loaded_functions: usize,
    pub(crate) reached: Option<String>,
}

impl FlowLimits {
    pub(crate) const fn new(max_depth: usize) -> Self {
        Self {
            max_depth,
            max_visited_nodes: MAX_VISITED_NODES,
            max_examined_edges: MAX_EXAMINED_EDGES,
            max_loaded_functions: MAX_LOADED_FUNCTIONS,
            visited_nodes: 0,
            examined_edges: 0,
            loaded_functions: 0,
            reached: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowInvestigationReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) status: FlowStatus,
    pub(crate) profile: String,
    pub(crate) linked_ir: String,
    pub(crate) root: String,
    pub(crate) target_kind: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) route: Option<String>,
    pub(crate) claims: FlowClaims,
    pub(crate) limits: FlowLimits,
    pub(crate) steps: Vec<FlowStepEvidence>,
    pub(crate) effects: Vec<FlowEffectEvidence>,
    pub(crate) rust_boundaries: Vec<FlowRustBoundaryEvidence>,
    pub(crate) blockers: Vec<FlowBlocker>,
}

impl FlowInvestigationReport {
    pub(crate) fn reached(&self) -> bool {
        self.status != FlowStatus::NotReached
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowStepEvidence {
    pub(crate) ordinal: usize,
    pub(crate) evidence: EvidenceLevel,
    pub(crate) context: String,
    pub(crate) caller: String,
    pub(crate) callee: String,
    pub(crate) site: Option<u32>,
    pub(crate) kind: String,
    pub(crate) tail: bool,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<FlowArgumentEvidence>,
    pub(crate) guards: Vec<String>,
    pub(crate) origin: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowArgumentEvidence {
    pub(crate) position: usize,
    pub(crate) local: String,
    pub(crate) resolved: String,
    pub(crate) constants: Vec<u32>,
    pub(crate) provenance: &'static str,
    pub(crate) pointee: Vec<FlowPointeeEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowPointeeEvidence {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) local: String,
    pub(crate) resolved: String,
    pub(crate) constants: Vec<u32>,
    pub(crate) provenance: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowEffectEvidence {
    pub(crate) kind: String,
    pub(crate) evidence: EvidenceLevel,
    pub(crate) function: String,
    pub(crate) site: Option<u32>,
    pub(crate) operation: Option<String>,
    pub(crate) detail: String,
    pub(crate) constant: Option<u64>,
    pub(crate) access: Option<String>,
    pub(crate) width: Option<u8>,
    pub(crate) address: Option<u32>,
    pub(crate) register: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) origin_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowRustBoundaryEvidence {
    pub(crate) vendor_source: String,
    pub(crate) vendor_symbol: String,
    pub(crate) association: String,
    pub(crate) reviewed: bool,
    pub(crate) status: String,
    pub(crate) production_component: Option<String>,
    pub(crate) verification_probes: Vec<String>,
    pub(crate) report: String,
    pub(crate) report_complete_project_run: bool,
    pub(crate) report_passed: bool,
    pub(crate) freshness_claim: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowBlocker {
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) next_action: String,
}

impl FlowBlocker {
    pub(crate) fn new(
        kind: impl Into<String>,
        message: impl Into<String>,
        next_action: impl Into<String>,
    ) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            next_action: next_action.into(),
        }
    }
}
