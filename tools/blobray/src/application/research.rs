//! Explainable, scope-aware prioritization of the next research action.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::Write,
    path::{Path, PathBuf},
};

use open_radio_vendor_contracts::SemanticEntityId;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ProjectSession;
use super::{ExecutableAction, ProjectContextRequirement};
use crate::{
    Result,
    registers::{
        ProjectRegisterWorkspace, RegisterPublicationOwnership, classify_register_publication,
        reviewed_register_identities,
    },
    review_scopes::{ReviewScopeReport, ReviewScopesDocument},
};

pub(crate) const RESEARCH_SCHEMA: u32 = 18;
const MAX_RESEARCH_EVENT_ROUTES: usize = 256;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchRankingStrategy {
    #[default]
    Impact,
    QuickWins,
    Frontier,
}

impl ResearchRankingStrategy {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Impact => "impact",
            Self::QuickWins => "quick-wins",
            Self::Frontier => "frontier",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchFocus {
    #[default]
    All,
    HardwareAccess,
}

impl ResearchFocus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::HardwareAccess => "hardware-access",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResearchNextOptions<'a> {
    pub(crate) scope: Option<&'a str>,
    pub(crate) protocol: Option<&'a str>,
    pub(crate) finding: Option<&'a str>,
    pub(crate) strategy: ResearchRankingStrategy,
    pub(crate) focus: ResearchFocus,
    pub(crate) budget: Option<u64>,
    pub(crate) limit: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchFindingQueryState {
    All,
    Open,
    ConditionSatisfied,
    InputNotObserved,
    FilteredOut,
    NotPresent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchRegisterResolutionSubject {
    pub(crate) chip: String,
    pub(crate) address_space: String,
    pub(crate) address: u32,
    pub(crate) width: u8,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ResearchRegisterObservationArtifact {
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchRegisterPublicationOwnership {
    Owned,
    External,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ResearchRegisterObservationSite {
    pub(crate) function: String,
    pub(crate) pc: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchRegisterObservationEvidence {
    pub(crate) analysis_artifacts: Vec<ResearchRegisterObservationArtifact>,
    pub(crate) range: String,
    pub(crate) publication_ownership: ResearchRegisterPublicationOwnership,
    pub(crate) read_functions: Vec<String>,
    pub(crate) write_functions: Vec<String>,
    pub(crate) read_sites: Vec<ResearchRegisterObservationSite>,
    pub(crate) write_sites: Vec<ResearchRegisterObservationSite>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum ResearchFindingResolutionEvidence {
    RegisterWorkspaceAbsent {
        address: u32,
        width: u8,
    },
    AbsentRegisterModel {
        subject: ResearchRegisterResolutionSubject,
        current_observation: Option<ResearchRegisterObservationEvidence>,
        current_identity: Option<String>,
        matching_scopes: Vec<String>,
        applied_assertions: Vec<open_radio_vendor_review::EffectiveAssertion>,
        model_sources: Vec<String>,
    },
    UnknownHardwareWriteSemantics {
        assertion_id: String,
        effective_write_semantics: String,
        subject: ResearchRegisterResolutionSubject,
        current_observation: Option<ResearchRegisterObservationEvidence>,
        current_identity: Option<String>,
        matching_scopes: Vec<String>,
        applied_assertions: Vec<open_radio_vendor_review::EffectiveAssertion>,
        model_sources: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchFindingQuery {
    pub(crate) state: ResearchFindingQueryState,
    pub(crate) finding_id: Option<String>,
    /// Looking up a queue identity never proves the underlying research done.
    pub(crate) completion_claim: bool,
    /// Exact lookup is not evidence about a prior occurrence of this ID.
    pub(crate) historical_finding_claim: bool,
    pub(crate) interpretation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resolution_evidence: Option<ResearchFindingResolutionEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchScoreBreakdown {
    pub(crate) guaranteed_weight: u64,
    pub(crate) optimistic_weight: u64,
    pub(crate) marginal_weight: u64,
    pub(crate) root_weight: u64,
    pub(crate) capability_weight: u64,
    pub(crate) verification_weight: u64,
    pub(crate) publication_weight: u64,
    pub(crate) cost_penalty: u64,
    pub(crate) co_blocker_penalty: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchScoreExplanation {
    pub(crate) benefit_points: u64,
    pub(crate) effort_points: u64,
    pub(crate) estimated_cost_units: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ResearchSubject {
    AnalysisRoot {
        root_id: String,
    },
    EventRouteBlocker {
        route_id: String,
        blocker_kind: String,
    },
    MmioRegister {
        address_space: String,
        address: u32,
        width: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        assertion: Option<String>,
    },
    InterfaceObservation {
        observation: String,
        contract: String,
        source: String,
        offset: i32,
        width: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        selector: Option<String>,
        call_sites: Vec<u32>,
    },
    PublicSymbolFamily {
        surface: String,
        protocols: Vec<String>,
        source: String,
        symbol_prefix: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
        state: ResearchAnalysisSurfaceState,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchAnalysisSurfaceState {
    MissingVendorArtifact,
    MissingSymbolInventory,
    StaleSymbolFamily,
    MissingProfileDefinition,
    MissingProfileOutput,
    InvalidProfileOutput,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchConsumerResolution {
    Ready,
    NeedsAnchor,
    NeedsDestination,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchActionability {
    Ready,
    NeedsAnchor,
    NeedsDestination,
    /// Reserved for a producer-supplied typed coverage cause. Current
    /// diagnostics must not manufacture this state from message text.
    #[allow(dead_code)]
    CoverageBlocked,
    InspectionOnly,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchActionabilityGroup {
    pub(crate) count: usize,
    pub(crate) finding_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchActionabilitySummary {
    pub(crate) ready: ResearchActionabilityGroup,
    pub(crate) needs_anchor: ResearchActionabilityGroup,
    pub(crate) needs_destination: ResearchActionabilityGroup,
    pub(crate) coverage_blocked: ResearchActionabilityGroup,
    pub(crate) inspection_only: ResearchActionabilityGroup,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchPrerequisiteKind {
    AcquireRequiredAnalysisSurface,
    ConfigureInterfaceDestination,
    CreateInterfaceAnchor,
    SelectReviewedKnowledgeDestination,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchPrerequisiteAction {
    pub(crate) rank: usize,
    pub(crate) id: String,
    pub(crate) kind: ResearchPrerequisiteKind,
    pub(crate) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<PathBuf>,
    pub(crate) subject: String,
    pub(crate) manual_action: String,
    pub(crate) satisfies_finding_ids: Vec<String>,
    pub(crate) blocked_action_ids: Vec<String>,
    pub(crate) guaranteed_unlock: usize,
    pub(crate) optimistic_unlock: usize,
    pub(crate) affected_scope_roots: Vec<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) benefit_points: u64,
    pub(crate) estimated_cost_units: u64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum ResearchConsumer {
    ReviewedKnowledgeAssertions {
        resolution: ResearchConsumerResolution,
        configured_paths: Vec<PathBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        selected_path: Option<PathBuf>,
        assertion_kinds: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic: Option<String>,
    },
    InterfacePackSlot {
        resolution: ResearchConsumerResolution,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<PathBuf>,
        contract: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        anchor: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        offset: i32,
        width: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        diagnostic: Option<String>,
    },
    RequiredAnalysisSurface {
        state: ResearchAnalysisSurfaceState,
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        profile: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<PathBuf>,
        project_manifest: PathBuf,
        working_directory: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_spec_override: Option<PathBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_spec_override: Option<PathBuf>,
        svd_overrides: Vec<PathBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_spec: Option<PathBuf>,
        diagnostic: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchLinkRelation {
    ExistingEvidenceContext,
    ReviewScopeContext,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ResearchCapabilityLink {
    pub(crate) rule: String,
    pub(crate) status: String,
    pub(crate) requirement_kind: String,
    pub(crate) requirement: String,
    pub(crate) function: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_site: Option<u32>,
    pub(crate) relation: ResearchLinkRelation,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct ResearchVerificationLink {
    pub(crate) surface: String,
    pub(crate) surface_kind: String,
    pub(crate) review_scope: String,
    pub(crate) closed: bool,
    pub(crate) relation: ResearchLinkRelation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchFinding {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) severity: String,
    pub(crate) subject: ResearchSubject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reviewed_memory_access: Option<crate::ReviewedMemoryAccessClassification>,
    pub(crate) consumers: Vec<ResearchConsumer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) blocker_resolution_route: Option<crate::BlockerResolutionRoute>,
    pub(crate) resolution_owner: crate::BlockerResolutionOwner,
    pub(crate) actionability: ResearchActionability,
    pub(crate) prerequisite_ids: Vec<String>,
    pub(crate) evidence_sites: Vec<u32>,
    pub(crate) evidence_channels: Vec<String>,
    /// Functions that contain the causal evidence and should be inspected.
    pub(crate) inspection_function_ids: Vec<String>,
    /// Functions whose analysis is directly affected by this finding.
    pub(crate) direct_function_ids: Vec<String>,
    pub(crate) guaranteed_function_ids: Vec<String>,
    pub(crate) optimistic_function_ids: Vec<String>,
    pub(crate) marginal_function_ids: Vec<String>,
    pub(crate) co_blocker_ids: Vec<String>,
    pub(crate) affected_scope_roots: Vec<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) capability_links: Vec<ResearchCapabilityLink>,
    pub(crate) verification_links: Vec<ResearchVerificationLink>,
    pub(crate) publication_scopes: Vec<String>,
    pub(crate) knowledge_required: String,
    pub(crate) evidence_required: Vec<String>,
    pub(crate) revalidation_actions: Vec<ExecutableAction>,
    pub(crate) requery_action: ExecutableAction,
    pub(crate) summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchAction {
    pub(crate) rank: usize,
    pub(crate) id: String,
    pub(crate) kinds: Vec<String>,
    pub(crate) score: u64,
    pub(crate) inspection_function_ids: Vec<String>,
    pub(crate) direct_functions: usize,
    pub(crate) direct_function_ids: Vec<String>,
    pub(crate) guaranteed_unlock: usize,
    pub(crate) guaranteed_function_ids: Vec<String>,
    pub(crate) optimistic_unlock: usize,
    pub(crate) optimistic_function_ids: Vec<String>,
    pub(crate) marginal_unlock_after_co_blockers: usize,
    pub(crate) marginal_function_ids: Vec<String>,
    pub(crate) co_blockers: usize,
    pub(crate) co_blocker_ids: Vec<String>,
    pub(crate) affected_scope_roots: Vec<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) capability_links: Vec<ResearchCapabilityLink>,
    pub(crate) verification_links: Vec<ResearchVerificationLink>,
    pub(crate) publication_scopes: Vec<String>,
    pub(crate) estimated_cost: String,
    pub(crate) confidence: String,
    pub(crate) next_action: ExecutableAction,
    pub(crate) actionability: ResearchActionabilitySummary,
    pub(crate) prerequisite_ids: Vec<String>,
    pub(crate) findings: Vec<ResearchFinding>,
    pub(crate) score_breakdown: ResearchScoreBreakdown,
    pub(crate) score_explanation: ResearchScoreExplanation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchActionCatalogEntry {
    pub(crate) id: String,
    pub(crate) kinds: Vec<String>,
    pub(crate) score: u64,
    pub(crate) next_action: ExecutableAction,
    pub(crate) estimated_cost: String,
    pub(crate) confidence: String,
    pub(crate) resolution_owner: crate::BlockerResolutionOwner,
    pub(crate) required_model: String,
    pub(crate) score_breakdown: ResearchScoreBreakdown,
    pub(crate) score_explanation: ResearchScoreExplanation,
    pub(crate) finding_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchPrerequisiteCatalogEntry {
    pub(crate) id: String,
    pub(crate) kind: ResearchPrerequisiteKind,
    pub(crate) reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<PathBuf>,
    pub(crate) subject: String,
    pub(crate) manual_action: String,
    pub(crate) satisfies_finding_ids: Vec<String>,
    pub(crate) blocked_action_ids: Vec<String>,
    pub(crate) guaranteed_unlock: usize,
    pub(crate) optimistic_unlock: usize,
    pub(crate) affected_scope_roots: Vec<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) benefit_points: u64,
    pub(crate) estimated_cost_units: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchInventory {
    pub(crate) sha256: String,
    pub(crate) findings: Vec<ResearchFinding>,
    pub(crate) actions: Vec<ResearchActionCatalogEntry>,
    pub(crate) prerequisites: Vec<ResearchPrerequisiteCatalogEntry>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResearchStepKind {
    Prerequisite,
    Action,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchStepRef {
    pub(crate) kind: ResearchStepKind,
    pub(crate) id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchSelection {
    pub(crate) strategy: ResearchRankingStrategy,
    pub(crate) limit: usize,
    pub(crate) budget: Option<u64>,
    pub(crate) consumed_budget: u64,
    pub(crate) eligible_prerequisites: usize,
    pub(crate) eligible_actions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) diagnostic: Option<String>,
    pub(crate) steps: Vec<ResearchStepRef>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchReviewedFunction {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchNextReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) project: String,
    pub(crate) focus: ResearchFocus,
    pub(crate) protocol: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) analyzed_scopes: Vec<String>,
    pub(crate) finding_query: ResearchFindingQuery,
    /// Research prioritization never proves that the investigation is complete.
    pub(crate) completion_claim: bool,
    pub(crate) capability_diagnostic: Option<String>,
    pub(crate) verification_diagnostic: Option<String>,
    pub(crate) reviewed_functions: Vec<ResearchReviewedFunction>,
    pub(crate) inventory: ResearchInventory,
    pub(crate) selection: ResearchSelection,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphNode {
    source: String,
    symbol: String,
    dependencies: BTreeSet<String>,
    direct_diagnostic_roots: BTreeSet<String>,
    complete: bool,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ScopeGraph {
    nodes: BTreeMap<String, GraphNode>,
    outgoing: BTreeMap<String, BTreeSet<String>>,
    incoming: BTreeMap<String, BTreeSet<String>>,
}

type DirectDiagnosticOwners = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug)]
struct Accumulator {
    id: String,
    kind: String,
    severity: String,
    message: String,
    subject: ResearchSubject,
    reviewed_memory_access: Option<crate::ReviewedMemoryAccessClassification>,
    consumers: Vec<ResearchConsumer>,
    blocker_resolution_route: Option<crate::BlockerResolutionRoute>,
    evidence_sites: BTreeSet<u32>,
    evidence_channels: BTreeSet<String>,
    inspection: BTreeSet<String>,
    direct: BTreeSet<String>,
    guaranteed: BTreeSet<String>,
    optimistic: BTreeSet<String>,
    marginal: BTreeSet<String>,
    co_blockers: BTreeSet<String>,
    roots: BTreeSet<String>,
    scopes: BTreeSet<String>,
    publication_scopes: BTreeSet<String>,
}

struct Seed {
    id: String,
    kind: String,
    severity: String,
    message: String,
    subject: ResearchSubject,
    reviewed_memory_access: Option<crate::ReviewedMemoryAccessClassification>,
    consumers: Vec<ResearchConsumer>,
    blocker_resolution_route: Option<crate::BlockerResolutionRoute>,
    evidence_sites: BTreeSet<u32>,
    evidence_channels: BTreeSet<String>,
    inspection: BTreeSet<String>,
    direct: BTreeSet<String>,
    guaranteed: BTreeSet<String>,
    optimistic: BTreeSet<String>,
    marginal: BTreeSet<String>,
    co_blockers: BTreeSet<String>,
    roots: BTreeSet<String>,
}

type CapabilityContexts = BTreeMap<String, BTreeSet<ResearchCapabilityLink>>;
type VerificationContexts = BTreeMap<String, BTreeSet<ResearchVerificationLink>>;

pub(crate) fn next(
    session: &ProjectSession,
    options: ResearchNextOptions<'_>,
) -> Result<ResearchNextReport> {
    if options.limit == 0 {
        return Err(crate::Error::invalid(
            "research next limit must be non-zero",
        ));
    }
    if options.budget == Some(0) {
        return Err(crate::Error::invalid(
            "research next budget must be non-zero",
        ));
    }
    let document = crate::review_scopes::load_for_project(&session.project)?;
    let selected_protocol = options
        .protocol
        .map(normalize_protocol_filter)
        .transpose()?;
    let scopes = select_scopes(&document, options.scope, selected_protocol)?;
    let mut analyzed_scopes = scopes
        .iter()
        .map(|scope| scope.id.clone())
        .collect::<Vec<_>>();
    analyzed_scopes.sort();
    analyzed_scopes.dedup();
    let configured_scopes = document.scopes.iter().collect::<Vec<_>>();
    let graph_scopes = if options.finding.is_some() {
        configured_scopes.clone()
    } else {
        scopes.clone()
    };
    let (direct_diagnostic_owners, graphs) = load_research_graphs(session, &graph_scopes)?;
    let (interface_context, capability_diagnostic) = interface_research_context(session);
    let reviewed_memory_accesses =
        crate::harnesses::reviewed_memory_accesses(session.project.analysis_provider.as_deref())?;
    let mut candidates = BTreeMap::new();
    for scope in &scopes {
        add_blockers(
            &session.project,
            scope,
            &graphs[&scope.id],
            &direct_diagnostic_owners,
            reviewed_memory_accesses,
            &session.project.review_context,
            &mut candidates,
        )?;
    }
    add_incomplete_event_route_blockers(session, &scopes, &graphs, &mut candidates)?;
    if options.scope.is_none() {
        add_required_analysis_surface_findings(session, selected_protocol, &mut candidates)?;
    }
    let exact_resolution = if let Some(paths) = session.project.registers.as_ref() {
        // These adjacent producers share the heavyweight MMIO facts, but the
        // workspace must be dropped before interface ranking and rendering.
        let workspace = ProjectRegisterWorkspace::load(paths)?;
        let knowledge = selected_review_knowledge(session)?;
        add_registers(
            session,
            paths,
            &workspace,
            &scopes,
            &graphs,
            &mut candidates,
        )?;
        add_unknown_semantics(
            session,
            paths,
            &workspace,
            &knowledge,
            &scopes,
            &graphs,
            &mut candidates,
        )?;
        resolve_exact_register_finding(
            options.finding,
            &ExactRegisterResolutionContext {
                paths,
                workspace: &workspace,
                knowledge: &knowledge,
                configured_scopes: &configured_scopes,
                selected_scopes: &scopes,
                graphs: &graphs,
            },
        )?
    } else {
        resolve_exact_without_register_workspace(options.finding)
    };
    if !exact_register_or_semantic_lookup(options.finding)
        && let Some(context) = interface_context.as_ref()
    {
        add_interfaces(
            session,
            &context.observations,
            &scopes,
            &graphs,
            &mut candidates,
        )?;
    }
    attach_candidate_co_blockers(&mut candidates);
    let finding_query = apply_finding_query(&mut candidates, options.finding, exact_resolution)?;
    let capabilities = interface_context
        .as_ref()
        .map_or_else(CapabilityContexts::new, |context| {
            capability_contexts(&context.links)
        });
    let (surfaces, verification_diagnostic) = verification_contexts(&session.project);
    let context = session.context();
    let ranked = candidates
        .into_values()
        .map(|candidate| {
            let requery_action = context.follow_up_action(
                [
                    "project".to_owned(),
                    "research".to_owned(),
                    "next".to_owned(),
                    "--finding".to_owned(),
                    candidate.id.clone(),
                ],
                ProjectContextRequirement::Analysis,
            )?;
            let next_action = context.follow_up_action(
                next_action_tokens(&candidate),
                ProjectContextRequirement::Analysis,
            )?;
            let revalidation_action = context
                .follow_up_action(["project", "analyze"], ProjectContextRequirement::Analysis)?;
            Ok(finalize(
                candidate,
                &capabilities,
                &surfaces,
                next_action,
                revalidation_action,
                requery_action,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let actions = coalesce_actions(ranked);
    let prerequisites = build_prerequisites(&actions);
    let prerequisite_indices = ranked_prerequisite_indices_for_focus(
        &prerequisites,
        &actions,
        options.strategy,
        options.focus,
    );
    let action_indices = ranked_action_indices_for_focus(&actions, options.strategy, options.focus);
    let minimum_cost = prerequisite_indices
        .iter()
        .map(|index| prerequisites[*index].estimated_cost_units)
        .chain(
            action_indices
                .iter()
                .map(|index| actions[*index].score_explanation.estimated_cost_units),
        )
        .min();
    let (steps, consumed_budget) = select_ranked_steps(
        &prerequisites,
        &prerequisite_indices,
        &actions,
        &action_indices,
        options.limit,
        options.budget,
    );
    let selection_diagnostic = if steps.is_empty()
        && (!prerequisite_indices.is_empty() || !action_indices.is_empty())
        && let (Some(budget), Some(minimum_cost)) = (options.budget, minimum_cost)
    {
        Some(format!(
            "no {} research step fits budget {budget}; the minimum estimated step cost is {minimum_cost}",
            options.strategy.label()
        ))
    } else {
        None
    };
    let inventory = build_inventory(
        &session.project.id,
        &analyzed_scopes,
        actions,
        prerequisites,
    )?;
    let reviewed_functions = research_reviewed_functions(session, &inventory)?;
    let report = ResearchNextReport {
        schema_version: RESEARCH_SCHEMA,
        command: "research next".to_owned(),
        project: session.project.id.clone(),
        focus: options.focus,
        protocol: selected_protocol.map(str::to_owned),
        scope: options.scope.map(str::to_owned),
        analyzed_scopes,
        finding_query,
        completion_claim: false,
        capability_diagnostic,
        verification_diagnostic,
        reviewed_functions,
        inventory,
        selection: ResearchSelection {
            strategy: options.strategy,
            limit: options.limit,
            budget: options.budget,
            consumed_budget,
            eligible_prerequisites: prerequisite_indices.len(),
            eligible_actions: action_indices.len(),
            diagnostic: selection_diagnostic,
            steps,
        },
    };
    validate_report(&report)?;
    Ok(report)
}

fn research_reviewed_functions(
    session: &ProjectSession,
    inventory: &ResearchInventory,
) -> Result<Vec<ResearchReviewedFunction>> {
    let referenced = inventory
        .findings
        .iter()
        .flat_map(|finding| {
            finding
                .inspection_function_ids
                .iter()
                .chain(&finding.direct_function_ids)
                .chain(&finding.guaranteed_function_ids)
                .chain(&finding.optimistic_function_ids)
                .chain(&finding.marginal_function_ids)
                .chain(&finding.affected_scope_roots)
        })
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let Some(workspace) = session.function_workspace()? else {
        return Ok(Vec::new());
    };
    let mut reviewed = workspace
        .pack
        .functions
        .iter()
        .filter(|function| referenced.contains(function.identity.as_str()))
        .filter_map(|function| {
            function.name.as_ref().map(|name| ResearchReviewedFunction {
                profile: function.profile.clone(),
                source: function.source.clone(),
                identity: function.identity.clone(),
                name: name.clone(),
                role: function.role.clone(),
                summary: function.summary.clone(),
            })
        })
        .collect::<Vec<_>>();
    reviewed.sort_by(|left, right| {
        (&left.identity, &left.profile).cmp(&(&right.identity, &right.profile))
    });
    Ok(reviewed)
}

fn build_inventory(
    project: &str,
    analyzed_scopes: &[String],
    actions: Vec<ResearchAction>,
    prerequisites: Vec<ResearchPrerequisiteAction>,
) -> Result<ResearchInventory> {
    let mut findings = Vec::new();
    let mut action_catalog = Vec::with_capacity(actions.len());
    for mut action in actions {
        let action_findings = std::mem::take(&mut action.findings);
        let resolution = finding_action_resolution_key(
            action_findings
                .first()
                .expect("every research action owns one or more findings"),
        );
        let mut finding_ids = action_findings
            .iter()
            .map(|finding| finding.id.clone())
            .collect::<Vec<_>>();
        finding_ids.sort();
        finding_ids.dedup();
        findings.extend(action_findings);
        action_catalog.push(ResearchActionCatalogEntry {
            id: action.id,
            kinds: action.kinds,
            score: action.score,
            next_action: action.next_action,
            estimated_cost: action.estimated_cost,
            confidence: action.confidence,
            resolution_owner: resolution.owner,
            required_model: resolution.required_model,
            score_breakdown: action.score_breakdown,
            score_explanation: action.score_explanation,
            finding_ids,
        });
    }
    findings.sort_by(|left, right| left.id.cmp(&right.id));
    action_catalog.sort_by(|left, right| left.id.cmp(&right.id));
    let mut prerequisite_catalog = prerequisites
        .into_iter()
        .map(|prerequisite| ResearchPrerequisiteCatalogEntry {
            id: prerequisite.id,
            kind: prerequisite.kind,
            reason: prerequisite.reason,
            path: prerequisite.path,
            subject: prerequisite.subject,
            manual_action: prerequisite.manual_action,
            satisfies_finding_ids: prerequisite.satisfies_finding_ids,
            blocked_action_ids: prerequisite.blocked_action_ids,
            guaranteed_unlock: prerequisite.guaranteed_unlock,
            optimistic_unlock: prerequisite.optimistic_unlock,
            affected_scope_roots: prerequisite.affected_scope_roots,
            scopes: prerequisite.scopes,
            benefit_points: prerequisite.benefit_points,
            estimated_cost_units: prerequisite.estimated_cost_units,
        })
        .collect::<Vec<_>>();
    prerequisite_catalog.sort_by(|left, right| left.id.cmp(&right.id));
    let sha256 = inventory_sha256(
        project,
        analyzed_scopes,
        &findings,
        &action_catalog,
        &prerequisite_catalog,
    )?;
    Ok(ResearchInventory {
        sha256,
        findings,
        actions: action_catalog,
        prerequisites: prerequisite_catalog,
    })
}

#[derive(Serialize)]
struct ResearchInventoryDigestInput<'a> {
    project: &'a str,
    analyzed_scopes: &'a [String],
    findings: &'a [ResearchFinding],
    actions: &'a [ResearchActionCatalogEntry],
    prerequisites: &'a [ResearchPrerequisiteCatalogEntry],
}

struct Sha256Writer(Sha256);

impl Write for Sha256Writer {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.update(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn inventory_sha256(
    project: &str,
    analyzed_scopes: &[String],
    findings: &[ResearchFinding],
    actions: &[ResearchActionCatalogEntry],
    prerequisites: &[ResearchPrerequisiteCatalogEntry],
) -> Result<String> {
    let mut writer = Sha256Writer(Sha256::new());
    writer.write_all(b"blobray-research-inventory-v1\0")?;
    serde_json::to_writer(
        &mut writer,
        &ResearchInventoryDigestInput {
            project,
            analyzed_scopes,
            findings,
            actions,
            prerequisites,
        },
    )?;
    Ok(format!("{:x}", writer.0.finalize()))
}

fn validate_report(report: &ResearchNextReport) -> Result<()> {
    if report.schema_version != RESEARCH_SCHEMA {
        return Err(crate::Error::invalid(format!(
            "research report schema {} is not current schema {RESEARCH_SCHEMA}",
            report.schema_version
        )));
    }
    if report.protocol.as_deref().is_some_and(|protocol| {
        crate::project::canonical_review_protocol(protocol) != Some(protocol)
    }) {
        return Err(crate::Error::invalid(
            "research report protocol must be a canonical supported protocol",
        ));
    }
    if report.completion_claim
        || report.finding_query.completion_claim
        || report.finding_query.historical_finding_claim
    {
        return Err(crate::Error::invalid(
            "research finding lookup cannot claim completion or historical occurrence",
        ));
    }
    let inventory = &report.inventory;
    let mut previous_reviewed = None;
    for function in &report.reviewed_functions {
        if function.profile.is_empty()
            || function.source.is_empty()
            || function.name.is_empty()
            || function
                .identity
                .strip_prefix(&function.source)
                .and_then(|identity| identity.strip_prefix("::"))
                .is_none_or(str::is_empty)
        {
            return Err(crate::Error::invalid(
                "research reviewed-function labels must have a profile, source, identity and name",
            ));
        }
        let key = (function.identity.as_str(), function.profile.as_str());
        if previous_reviewed.is_some_and(|previous| previous >= key) {
            return Err(crate::Error::invalid(format!(
                "research reviewed-function labels are not unique and strictly sorted at {:?}",
                function.identity
            )));
        }
        previous_reviewed = Some(key);
    }
    validate_sorted_unique_ids(
        "finding",
        inventory.findings.iter().map(|finding| finding.id.as_str()),
    )?;
    validate_sorted_unique_ids(
        "action",
        inventory.actions.iter().map(|action| action.id.as_str()),
    )?;
    validate_sorted_unique_ids(
        "prerequisite",
        inventory
            .prerequisites
            .iter()
            .map(|prerequisite| prerequisite.id.as_str()),
    )?;
    let finding_ids = inventory
        .findings
        .iter()
        .map(|finding| finding.id.as_str())
        .collect::<BTreeSet<_>>();
    match (
        report.finding_query.state,
        report.finding_query.finding_id.as_deref(),
    ) {
        (ResearchFindingQueryState::All, None)
            if report.finding_query.resolution_evidence.is_none() => {}
        (ResearchFindingQueryState::Open, Some(id)) if finding_ids == [id].into() => {}
        (
            ResearchFindingQueryState::ConditionSatisfied
            | ResearchFindingQueryState::InputNotObserved
            | ResearchFindingQueryState::FilteredOut
            | ResearchFindingQueryState::NotPresent,
            Some(_),
        ) if finding_ids.is_empty() => {}
        _ => {
            return Err(crate::Error::invalid(
                "research finding query state does not match the exact inventory",
            ));
        }
    }
    validate_finding_resolution(report)?;
    if matches!(
        report.finding_query.state,
        ResearchFindingQueryState::ConditionSatisfied
            | ResearchFindingQueryState::InputNotObserved
            | ResearchFindingQueryState::FilteredOut
            | ResearchFindingQueryState::NotPresent
    ) && (!report.selection.steps.is_empty()
        || report.selection.eligible_actions != 0
        || report.selection.eligible_prerequisites != 0
        || report.selection.consumed_budget != 0)
    {
        return Err(crate::Error::invalid(
            "terminal exact finding resolution must have an empty selection",
        ));
    }
    let action_ids = inventory
        .actions
        .iter()
        .map(|action| action.id.as_str())
        .collect::<BTreeSet<_>>();
    let prerequisite_ids = inventory
        .prerequisites
        .iter()
        .map(|prerequisite| prerequisite.id.as_str())
        .collect::<BTreeSet<_>>();
    let prerequisites_by_id = inventory
        .prerequisites
        .iter()
        .map(|prerequisite| (prerequisite.id.as_str(), prerequisite))
        .collect::<BTreeMap<_, _>>();
    for prerequisite in &inventory.prerequisites {
        validate_sorted_unique_ids(
            "prerequisite finding reference",
            prerequisite
                .satisfies_finding_ids
                .iter()
                .map(String::as_str),
        )?;
        validate_sorted_unique_ids(
            "prerequisite action reference",
            prerequisite.blocked_action_ids.iter().map(String::as_str),
        )?;
    }
    for finding in &inventory.findings {
        validate_blocker_resolution_route(finding)?;
        validate_finding_resolution_identity(finding)?;
        validate_executable_action("finding requery", &finding.requery_action)?;
        for action in &finding.revalidation_actions {
            validate_executable_action("finding revalidation", action)?;
        }
        validate_finding_follow_up_actions(finding)?;
        validate_sorted_unique_ids(
            "finding prerequisite reference",
            finding.prerequisite_ids.iter().map(String::as_str),
        )?;
        let expected_seeds =
            finding_prerequisites(&finding.id, &finding.subject, &finding.consumers);
        let expected_ids = expected_seeds
            .iter()
            .map(|seed| seed.id.clone())
            .collect::<Vec<_>>();
        if finding.prerequisite_ids != expected_ids {
            return Err(crate::Error::invalid(format!(
                "research finding {:?} prerequisite set does not match its typed consumers",
                finding.id
            )));
        }
        for seed in &expected_seeds {
            let Some(prerequisite) = prerequisites_by_id.get(seed.id.as_str()) else {
                return Err(crate::Error::invalid(format!(
                    "research finding {:?} references missing prerequisite {:?}",
                    finding.id, seed.id
                )));
            };
            validate_prerequisite_seed(finding, prerequisite, seed)?;
            if prerequisite
                .satisfies_finding_ids
                .binary_search(&finding.id)
                .is_err()
            {
                return Err(crate::Error::invalid(format!(
                    "research finding {:?} and prerequisite {:?} are not reciprocal",
                    finding.id, seed.id
                )));
            }
        }
        validate_analysis_surface_finding(finding, report.protocol.as_deref())?;
    }
    let mut referenced_findings = BTreeSet::new();
    let mut action_by_finding = BTreeMap::new();
    for action in &inventory.actions {
        validate_executable_action("catalog next", &action.next_action)?;
        validate_sorted_unique_ids(
            "action finding reference",
            action.finding_ids.iter().map(String::as_str),
        )?;
        if action.finding_ids.is_empty() {
            return Err(crate::Error::invalid(format!(
                "research action {:?} is not owned by any finding",
                action.id
            )));
        }
        let mut action_resolution = None;
        for finding in &action.finding_ids {
            let finding_entry = inventory
                .findings
                .binary_search_by(|entry| entry.id.cmp(finding))
                .ok()
                .map(|index| &inventory.findings[index])
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "research action {:?} references missing finding {finding:?}",
                        action.id
                    ))
                })?;
            let resolution = finding_action_resolution_key(finding_entry);
            if action_resolution
                .as_ref()
                .is_some_and(|existing| existing != &resolution)
            {
                return Err(crate::Error::invalid(format!(
                    "research action {:?} coalesces findings with different resolution owners or models",
                    action.id
                )));
            }
            action_resolution = Some(resolution);
        }
        let action_resolution = action_resolution.expect("non-empty action checked above");
        if action.resolution_owner != action_resolution.owner
            || action.required_model != action_resolution.required_model
        {
            return Err(crate::Error::invalid(format!(
                "research action {:?} does not publish its findings' exact resolution owner and model",
                action.id
            )));
        }
        if action.id
            != stable_id(
                "action",
                &action_canonical_identity(&action.next_action, &action_resolution),
            )
        {
            return Err(crate::Error::invalid(format!(
                "research action {:?} does not match its canonical executable identity and resolution model",
                action.id
            )));
        }
        for finding in &action.finding_ids {
            if !referenced_findings.insert(finding.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research finding {finding:?} belongs to more than one action"
                )));
            }
            let finding_entry = inventory
                .findings
                .binary_search_by(|entry| entry.id.cmp(finding))
                .ok()
                .map(|index| &inventory.findings[index])
                .expect("finding membership checked above");
            validate_analysis_surface_next_action(finding_entry, &action.next_action)?;
            validate_event_route_next_action(finding_entry, &action.next_action)?;
            action_by_finding.insert(finding.as_str(), action.id.as_str());
        }
    }
    if referenced_findings != finding_ids {
        return Err(crate::Error::invalid(
            "research inventory contains a finding that belongs to no action",
        ));
    }
    for prerequisite in &inventory.prerequisites {
        if prerequisite.satisfies_finding_ids.is_empty() {
            return Err(crate::Error::invalid(format!(
                "research prerequisite {:?} is not owned by any finding",
                prerequisite.id
            )));
        }
        for finding in &prerequisite.satisfies_finding_ids {
            if !finding_ids.contains(finding.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research prerequisite {:?} references missing finding {finding:?}",
                    prerequisite.id
                )));
            }
            let finding_entry = inventory
                .findings
                .binary_search_by(|entry| entry.id.cmp(finding))
                .ok()
                .map(|index| &inventory.findings[index])
                .expect("finding membership checked above");
            if finding_entry
                .prerequisite_ids
                .binary_search(&prerequisite.id)
                .is_err()
            {
                return Err(crate::Error::invalid(format!(
                    "research prerequisite {:?} and finding {finding:?} are not reciprocal",
                    prerequisite.id
                )));
            }
        }
        let expected_blocked_actions = prerequisite
            .satisfies_finding_ids
            .iter()
            .map(|finding| {
                action_by_finding
                    .get(finding.as_str())
                    .expect("every finding has one action owner")
                    .to_string()
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if prerequisite.blocked_action_ids != expected_blocked_actions {
            return Err(crate::Error::invalid(format!(
                "research prerequisite {:?} blocked action set does not match its finding owners",
                prerequisite.id
            )));
        }
    }
    if report.selection.steps.len() > report.selection.limit {
        return Err(crate::Error::invalid(
            "research selection exceeds its declared limit",
        ));
    }
    let mut selected = BTreeSet::new();
    for step in &report.selection.steps {
        let exists = match step.kind {
            ResearchStepKind::Prerequisite => prerequisite_ids.contains(step.id.as_str()),
            ResearchStepKind::Action => action_ids.contains(step.id.as_str()),
        };
        if !exists {
            return Err(crate::Error::invalid(format!(
                "research selection references missing {:?} {:?}",
                step.kind, step.id
            )));
        }
        if !selected.insert((step.kind, step.id.as_str())) {
            return Err(crate::Error::invalid(format!(
                "research selection repeats {:?} {:?}",
                step.kind, step.id
            )));
        }
    }
    let expected_sha256 = inventory_sha256(
        &report.project,
        &report.analyzed_scopes,
        &inventory.findings,
        &inventory.actions,
        &inventory.prerequisites,
    )?;
    if inventory.sha256 != expected_sha256 {
        return Err(crate::Error::invalid(format!(
            "research inventory digest mismatch: stored {}, computed {expected_sha256}",
            inventory.sha256
        )));
    }
    Ok(())
}

fn validate_finding_resolution_identity(finding: &ResearchFinding) -> Result<()> {
    if matches!(&finding.subject, ResearchSubject::EventRouteBlocker { .. })
        && (!finding.direct_function_ids.is_empty()
            || !finding.guaranteed_function_ids.is_empty()
            || !finding.optimistic_function_ids.is_empty()
            || !finding.marginal_function_ids.is_empty()
            || !finding.affected_scope_roots.is_empty()
            || !finding.publication_scopes.is_empty())
    {
        return Err(crate::Error::invalid(format!(
            "event-route finding {:?} publishes impact without typed affected-function evidence",
            finding.id
        )));
    }
    let expected_owner = finding.blocker_resolution_route.as_ref().map_or_else(
        || subject_resolution_owner(&finding.subject),
        |route| route.owner,
    );
    if finding.resolution_owner != expected_owner {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} does not publish its typed resolution owner",
            finding.id
        )));
    }
    if finding.blocker_resolution_route.is_none()
        && finding.knowledge_required != knowledge_required(&finding.kind)
    {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} does not publish its exact required model",
            finding.id
        )));
    }
    Ok(())
}

fn validate_blocker_resolution_route(finding: &ResearchFinding) -> Result<()> {
    let route = match (&finding.subject, &finding.blocker_resolution_route) {
        (ResearchSubject::AnalysisRoot { root_id }, Some(route)) => {
            route.validate(root_id)?;
            route
        }
        (ResearchSubject::AnalysisRoot { .. }, None) => {
            return Err(crate::Error::invalid(format!(
                "research blocker {:?} has no typed resolution route",
                finding.id
            )));
        }
        (
            ResearchSubject::EventRouteBlocker {
                route_id,
                blocker_kind,
            },
            Some(route),
        ) => {
            route.validate_event_route(route_id, blocker_kind)?;
            if finding.id != event_route_finding_id(route_id, blocker_kind)
                || finding.kind != *blocker_kind
            {
                return Err(crate::Error::invalid(format!(
                    "event-route research blocker {:?} does not match its typed subject",
                    finding.id
                )));
            }
            route
        }
        (ResearchSubject::EventRouteBlocker { .. }, None) => {
            return Err(crate::Error::invalid(format!(
                "event-route research blocker {:?} has no typed resolution route",
                finding.id
            )));
        }
        (_, Some(_)) => {
            return Err(crate::Error::invalid(format!(
                "non-blocker research finding {:?} must not carry a blocker resolution route",
                finding.id
            )));
        }
        (_, None) => return Ok(()),
    };
    if route.required_model != finding.knowledge_required {
        return Err(crate::Error::invalid(format!(
            "research blocker {:?} knowledge requirement diverges from its typed route",
            finding.id
        )));
    }
    if route.evidence_required != finding.evidence_required {
        return Err(crate::Error::invalid(format!(
            "research blocker {:?} evidence requirement diverges from its typed route",
            finding.id
        )));
    }
    Ok(())
}

fn validate_event_route_next_action(
    finding: &ResearchFinding,
    action: &ExecutableAction,
) -> Result<()> {
    let ResearchSubject::EventRouteBlocker { route_id, .. } = &finding.subject else {
        return Ok(());
    };
    let prefix = ["blobray", "inspect", "flow", "--event-route", route_id];
    let Some(suffix) = action.argv.get(prefix.len()..) else {
        return Err(crate::Error::invalid(format!(
            "event-route finding {:?} has an incomplete inspect action",
            finding.id
        )));
    };
    if action.argv[..prefix.len()] != prefix
        || !valid_analysis_context_suffix(suffix)
        || action.context != ProjectContextRequirement::Analysis
    {
        return Err(crate::Error::invalid(format!(
            "event-route finding {:?} does not use its exact typed inspect action",
            finding.id
        )));
    }
    Ok(())
}

fn validate_prerequisite_seed(
    finding: &ResearchFinding,
    prerequisite: &ResearchPrerequisiteCatalogEntry,
    seed: &PrerequisiteSeed,
) -> Result<()> {
    if prerequisite.id != seed.id
        || prerequisite.kind != seed.kind
        || prerequisite.path != seed.path
        || prerequisite.subject != seed.subject
        || prerequisite.manual_action != seed.manual_action
        || prerequisite.estimated_cost_units != seed.estimated_cost_units
        || (seed.kind == ResearchPrerequisiteKind::AcquireRequiredAnalysisSurface
            && prerequisite.reason != seed.reason)
    {
        return Err(crate::Error::invalid(format!(
            "research prerequisite {:?} does not match the typed consumer of finding {:?}",
            prerequisite.id, finding.id
        )));
    }
    Ok(())
}

fn validate_executable_action(label: &str, action: &ExecutableAction) -> Result<()> {
    ExecutableAction::new(
        action.argv.clone(),
        action.working_directory.clone(),
        action.context,
    )
    .map(|_| ())
    .map_err(|error| crate::Error::invalid(format!("invalid {label} action: {error}")))
}

fn valid_analysis_context_suffix(argv: &[String]) -> bool {
    fn consume(argv: &[String], index: &mut usize, option: &str) -> bool {
        if argv.get(*index).map(String::as_str) != Some(option) {
            return false;
        }
        let Some(value) = argv.get(*index + 1) else {
            return false;
        };
        if value.starts_with('-') {
            return false;
        }
        *index += 2;
        true
    }

    let mut index = 0;
    if !consume(argv, &mut index, "--project") {
        return false;
    }
    if argv.get(index).map(String::as_str) == Some("--target-spec")
        && !consume(argv, &mut index, "--target-spec")
    {
        return false;
    }
    if argv.get(index).map(String::as_str) == Some("--run-spec")
        && !consume(argv, &mut index, "--run-spec")
    {
        return false;
    }
    while argv.get(index).map(String::as_str) == Some("--svd") {
        if !consume(argv, &mut index, "--svd") {
            return false;
        }
    }
    index == argv.len()
}

fn validate_finding_follow_up_actions(finding: &ResearchFinding) -> Result<()> {
    if finding.revalidation_actions.len() != 1 {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} must have exactly one revalidation action",
            finding.id
        )));
    }
    let requery = &finding.requery_action;
    let revalidation = &finding.revalidation_actions[0];
    let requery_prefix = [
        "blobray",
        "project",
        "research",
        "next",
        "--finding",
        finding.id.as_str(),
    ];
    let revalidation_prefix = ["blobray", "project", "analyze"];
    let Some(requery_suffix) = requery.argv.get(requery_prefix.len()..) else {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} has an invalid exact requery action",
            finding.id
        )));
    };
    let Some(revalidation_suffix) = revalidation.argv.get(revalidation_prefix.len()..) else {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} has an invalid revalidation action",
            finding.id
        )));
    };
    if requery.argv[..requery_prefix.len()] != requery_prefix
        || revalidation.argv[..revalidation_prefix.len()] != revalidation_prefix
        || requery_suffix != revalidation_suffix
        || !valid_analysis_context_suffix(requery_suffix)
        || requery.context != ProjectContextRequirement::Analysis
        || revalidation.context != ProjectContextRequirement::Analysis
        || requery.working_directory != revalidation.working_directory
    {
        return Err(crate::Error::invalid(format!(
            "research finding {:?} follow-up actions do not share one exact analysis context",
            finding.id
        )));
    }
    Ok(())
}

fn validate_analysis_surface_finding(
    finding: &ResearchFinding,
    selected_protocol: Option<&str>,
) -> Result<()> {
    let subject = match &finding.subject {
        ResearchSubject::PublicSymbolFamily {
            surface,
            protocols,
            source,
            symbol_prefix,
            profile,
            state,
        } => Some((surface, protocols, source, symbol_prefix, profile, state)),
        _ => None,
    };
    let required_consumers = finding
        .consumers
        .iter()
        .filter_map(|consumer| match consumer {
            ResearchConsumer::RequiredAnalysisSurface {
                source,
                profile,
                state,
                output,
                project_manifest,
                working_directory,
                diagnostic,
                ..
            } => Some((
                source,
                profile,
                state,
                output,
                project_manifest,
                working_directory,
                diagnostic,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    let is_surface =
        finding.kind == "analysis-surface" || subject.is_some() || !required_consumers.is_empty();
    if !is_surface {
        return Ok(());
    }
    let Some((surface, protocols, subject_source, symbol_prefix, subject_profile, subject_state)) =
        subject
    else {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has no public-symbol-family subject",
            finding.id
        )));
    };
    if finding.kind != "analysis-surface"
        || finding.consumers.len() != 1
        || required_consumers.len() != 1
    {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has an inconsistent kind, actionability, or consumer set",
            finding.id
        )));
    }
    let (
        consumer_source,
        consumer_profile,
        consumer_state,
        output,
        project_manifest,
        working_directory,
        diagnostic,
    ) = required_consumers[0];
    if subject_source != consumer_source
        || subject_profile != consumer_profile
        || subject_state != consumer_state
    {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has inconsistent subject and consumer state",
            finding.id
        )));
    }
    let profile = subject_profile.as_deref();
    let protocols_are_valid = !protocols.is_empty()
        && protocols.iter().collect::<BTreeSet<_>>().len() == protocols.len()
        && protocols.iter().all(|protocol| {
            crate::project::canonical_review_protocol(protocol)
                .is_some_and(|canonical| canonical == protocol)
        });
    if surface.is_empty()
        || subject_source.is_empty()
        || symbol_prefix.is_empty()
        || profile.is_none_or(str::is_empty)
        || project_manifest.as_os_str().is_empty()
        || !working_directory.is_absolute()
        || working_directory.to_str().is_none()
        || diagnostic.is_empty()
        || !protocols_are_valid
        || selected_protocol.is_some_and(|protocol| !protocols.iter().any(|item| item == protocol))
    {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has an invalid typed subject or consumer payload",
            finding.id
        )));
    }
    if finding.id != stable_id("analysis-surface", surface) {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} does not match its canonical surface identity",
            finding.id
        )));
    }
    let automatic = matches!(
        subject_state,
        ResearchAnalysisSurfaceState::MissingSymbolInventory
            | ResearchAnalysisSurfaceState::MissingProfileOutput
            | ResearchAnalysisSurfaceState::InvalidProfileOutput
    );
    let expected_actionability = if automatic {
        ResearchActionability::Ready
    } else {
        ResearchActionability::CoverageBlocked
    };
    let expected_prerequisites = usize::from(!automatic);
    if finding.actionability != expected_actionability
        || finding.prerequisite_ids.len() != expected_prerequisites
    {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} actionability or prerequisite count does not match its state",
            finding.id
        )));
    }
    let output_is_valid = match subject_state {
        ResearchAnalysisSurfaceState::MissingProfileDefinition => output.is_none(),
        ResearchAnalysisSurfaceState::MissingProfileOutput
        | ResearchAnalysisSurfaceState::InvalidProfileOutput => output
            .as_ref()
            .is_some_and(|path| !path.as_os_str().is_empty()),
        ResearchAnalysisSurfaceState::MissingVendorArtifact
        | ResearchAnalysisSurfaceState::MissingSymbolInventory
        | ResearchAnalysisSurfaceState::StaleSymbolFamily => true,
    };
    if !output_is_valid {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} profile output does not match its state",
            finding.id
        )));
    }
    Ok(())
}

fn validate_analysis_surface_next_action(
    finding: &ResearchFinding,
    action: &ExecutableAction,
) -> Result<()> {
    let ResearchSubject::PublicSymbolFamily { profile, state, .. } = &finding.subject else {
        return Ok(());
    };
    let Some(ResearchConsumer::RequiredAnalysisSurface {
        project_manifest,
        working_directory,
        target_spec_override,
        run_spec_override,
        svd_overrides,
        ..
    }) = finding.consumers.first()
    else {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has no action context consumer",
            finding.id
        )));
    };
    fn push_path(argv: &mut Vec<String>, option: &str, path: &Path) -> Result<()> {
        let value = path.to_str().ok_or_else(|| {
            crate::Error::invalid(format!(
                "research analysis-surface action path is not valid UTF-8: {}",
                path.display()
            ))
        })?;
        if value.is_empty() {
            return Err(crate::Error::invalid(
                "research analysis-surface action path must not be empty",
            ));
        }
        argv.push(option.to_owned());
        argv.push(value.to_owned());
        Ok(())
    }
    let mut context_suffix = Vec::new();
    push_path(&mut context_suffix, "--project", project_manifest)?;
    if let Some(path) = target_spec_override {
        push_path(&mut context_suffix, "--target-spec", path)?;
    }
    if let Some(path) = run_spec_override {
        push_path(&mut context_suffix, "--run-spec", path)?;
    }
    for path in svd_overrides {
        push_path(&mut context_suffix, "--svd", path)?;
    }
    let mut expected = vec!["blobray".to_owned()];
    expected.extend(analysis_surface_next_action_tokens(
        *state,
        profile.as_deref(),
    ));
    expected.extend(context_suffix.iter().cloned());
    let mut expected_requery = vec![
        "blobray".to_owned(),
        "project".to_owned(),
        "research".to_owned(),
        "next".to_owned(),
        "--finding".to_owned(),
        finding.id.clone(),
    ];
    expected_requery.extend(context_suffix.iter().cloned());
    let mut expected_revalidation = vec![
        "blobray".to_owned(),
        "project".to_owned(),
        "analyze".to_owned(),
    ];
    expected_revalidation.extend(context_suffix);
    let revalidation = &finding.revalidation_actions[0];
    if action.context != ProjectContextRequirement::Analysis
        || action.argv != expected
        || action.working_directory != *working_directory
        || finding.requery_action.argv != expected_requery
        || finding.requery_action.working_directory != *working_directory
        || revalidation.argv != expected_revalidation
        || revalidation.working_directory != *working_directory
    {
        return Err(crate::Error::invalid(format!(
            "research analysis-surface finding {:?} has a next action that does not match its state",
            finding.id
        )));
    }
    Ok(())
}

fn validate_finding_resolution(report: &ResearchNextReport) -> Result<()> {
    let state = report.finding_query.state;
    let evidence = report.finding_query.resolution_evidence.as_ref();
    if let Some(evidence) = evidence {
        let Some(finding_id) = report.finding_query.finding_id.as_deref() else {
            return Err(crate::Error::invalid(
                "typed finding resolution evidence requires an exact finding ID",
            ));
        };
        if !resolution_evidence_matches_finding(finding_id, evidence) {
            return Err(crate::Error::invalid(
                "research finding resolution evidence does not match the exact finding ID and subject",
            ));
        }
    }
    let selected_scopes = report
        .analyzed_scopes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let intersects_selected = |matching_scopes: &[String]| {
        matching_scopes
            .iter()
            .any(|scope| selected_scopes.contains(scope.as_str()))
    };
    let valid = match (state, evidence) {
        (ResearchFindingQueryState::All, None) => true,
        (ResearchFindingQueryState::Open, None) => true,
        (
            ResearchFindingQueryState::Open,
            Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
                current_observation: Some(_),
                current_identity: None,
                matching_scopes,
                applied_assertions,
                ..
            }),
        ) => {
            !matching_scopes.is_empty()
                && intersects_selected(matching_scopes)
                && applied_assertions.is_empty()
        }
        (
            ResearchFindingQueryState::Open,
            Some(ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
                assertion_id,
                effective_write_semantics,
                current_observation: Some(_),
                matching_scopes,
                applied_assertions,
                ..
            }),
        ) => {
            effective_write_semantics == "unknown"
                && !matching_scopes.is_empty()
                && intersects_selected(matching_scopes)
                && semantic_assertion_matches(
                    assertion_id,
                    effective_write_semantics,
                    applied_assertions,
                )
        }
        (
            ResearchFindingQueryState::ConditionSatisfied,
            Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
                current_observation: Some(observation),
                current_identity: Some(current_identity),
                matching_scopes,
                applied_assertions,
                ..
            }),
        ) => {
            observation.publication_ownership == ResearchRegisterPublicationOwnership::Owned
                && !matching_scopes.is_empty()
                && intersects_selected(matching_scopes)
                && register_identity_assertion_matches(current_identity, applied_assertions)
        }
        (
            ResearchFindingQueryState::ConditionSatisfied,
            Some(ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
                assertion_id,
                effective_write_semantics,
                current_observation: Some(observation),
                current_identity: Some(_),
                matching_scopes,
                applied_assertions,
                ..
            }),
        ) => {
            observation.publication_ownership == ResearchRegisterPublicationOwnership::Owned
                && effective_write_semantics != "unknown"
                && !matching_scopes.is_empty()
                && intersects_selected(matching_scopes)
                && semantic_assertion_matches(
                    assertion_id,
                    effective_write_semantics,
                    applied_assertions,
                )
        }
        (
            ResearchFindingQueryState::InputNotObserved,
            Some(ResearchFindingResolutionEvidence::RegisterWorkspaceAbsent { .. }),
        ) => true,
        (
            ResearchFindingQueryState::InputNotObserved,
            Some(
                ResearchFindingResolutionEvidence::AbsentRegisterModel {
                    current_identity: None,
                    matching_scopes,
                    applied_assertions,
                    model_sources,
                    ..
                }
                | ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
                    current_identity: None,
                    matching_scopes,
                    applied_assertions,
                    model_sources,
                    ..
                },
            ),
        ) => {
            matching_scopes.is_empty() && applied_assertions.is_empty() && model_sources.is_empty()
        }
        (
            ResearchFindingQueryState::FilteredOut,
            Some(
                ResearchFindingResolutionEvidence::AbsentRegisterModel {
                    current_observation: Some(_),
                    current_identity: None,
                    matching_scopes,
                    applied_assertions,
                    model_sources,
                    ..
                }
                | ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
                    current_observation: Some(_),
                    current_identity: None,
                    matching_scopes,
                    applied_assertions,
                    model_sources,
                    ..
                },
            ),
        ) => {
            !matching_scopes.is_empty()
                && !intersects_selected(matching_scopes)
                && applied_assertions.is_empty()
                && model_sources.is_empty()
        }
        (ResearchFindingQueryState::NotPresent, None) => true,
        (
            ResearchFindingQueryState::NotPresent,
            Some(
                ResearchFindingResolutionEvidence::AbsentRegisterModel {
                    current_observation: Some(_),
                    matching_scopes,
                    ..
                }
                | ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
                    current_observation: Some(_),
                    matching_scopes,
                    ..
                },
            ),
        ) => !matching_scopes.is_empty() && intersects_selected(matching_scopes),
        _ => false,
    };
    if !valid {
        return Err(crate::Error::invalid(format!(
            "research finding resolution evidence does not satisfy state {state:?}"
        )));
    }
    Ok(())
}

fn resolution_evidence_matches_finding(
    finding_id: &str,
    evidence: &ResearchFindingResolutionEvidence,
) -> bool {
    match evidence {
        ResearchFindingResolutionEvidence::RegisterWorkspaceAbsent { address, width } => {
            register_finding_id(*address, *width) == finding_id
        }
        ResearchFindingResolutionEvidence::AbsentRegisterModel {
            subject,
            applied_assertions,
            ..
        } => {
            register_finding_id(subject.address, subject.width) == finding_id
                && assertions_match_register_subject(applied_assertions, subject)
        }
        ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
            assertion_id,
            subject,
            applied_assertions,
            ..
        } => {
            format!("semantic-{assertion_id}") == finding_id
                && assertions_match_register_subject(applied_assertions, subject)
        }
    }
}

fn register_finding_id(address: u32, width: u8) -> String {
    format!("register-{address:#010x}-{width}")
}

fn assertions_match_register_subject(
    assertions: &[open_radio_vendor_review::EffectiveAssertion],
    subject: &ResearchRegisterResolutionSubject,
) -> bool {
    let expected = SemanticEntityId::register(
        subject.chip.clone(),
        subject.address_space.clone(),
        u64::from(subject.address),
        u32::from(subject.width),
    )
    .expect("persisted research register subjects are canonical");
    assertions
        .iter()
        .all(|assertion| assertion.subject == expected)
}

fn semantic_assertion_matches(
    assertion_id: &str,
    effective_write_semantics: &str,
    applied_assertions: &[open_radio_vendor_review::EffectiveAssertion],
) -> bool {
    applied_assertions.len() == 1
        && applied_assertions.first().is_some_and(|assertion| {
            assertion.id == assertion_id
                && assertion.kind == "hardware-write-semantics"
                && !assertion.metadata.evidence.is_empty()
                && normalize_write_semantics(&assertion.value).as_deref()
                    == Some(effective_write_semantics)
        })
}

fn register_identity_assertion_matches(
    current_identity: &str,
    applied_assertions: &[open_radio_vendor_review::EffectiveAssertion],
) -> bool {
    applied_assertions.len() == 1
        && applied_assertions.first().is_some_and(|assertion| {
            assertion.kind == "register-identity"
                && !assertion.metadata.evidence.is_empty()
                && matches!(
                    &assertion.value,
                    open_radio_vendor_review::AssertionValue::String(value)
                        if value == current_identity
                )
        })
}

fn validate_sorted_unique_ids<'a>(kind: &str, ids: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut previous = None;
    for id in ids {
        if previous.is_some_and(|previous| previous >= id) {
            return Err(crate::Error::invalid(format!(
                "research {kind} IDs are not unique and strictly sorted at {id:?}"
            )));
        }
        previous = Some(id);
    }
    Ok(())
}

fn select_scopes<'a>(
    document: &'a ReviewScopesDocument,
    selected_scope: Option<&str>,
    selected_protocol: Option<&str>,
) -> Result<Vec<&'a ReviewScopeReport>> {
    let configured_scopes = document
        .scopes
        .iter()
        .map(|scope| (scope.id.clone(), scope.protocols.clone()))
        .collect::<BTreeMap<_, _>>();
    let selected_ids = select_scope_ids(&configured_scopes, selected_scope, selected_protocol)?;
    Ok(document
        .scopes
        .iter()
        .filter(|scope| selected_ids.contains(&scope.id))
        .collect())
}

fn select_scope_ids(
    configured_scopes: &BTreeMap<String, Vec<String>>,
    selected_scope: Option<&str>,
    selected_protocol: Option<&str>,
) -> Result<BTreeSet<String>> {
    if let Some(selected) = selected_scope
        && !configured_scopes.contains_key(selected)
    {
        let available = configured_scopes
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(crate::Error::invalid(format!(
            "unknown review scope {selected:?}; configured scopes: {available}"
        )));
    }
    let protocols = configured_scopes
        .values()
        .flatten()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if let Some(selected) = selected_protocol
        && !protocols.contains(selected)
    {
        return Err(crate::Error::invalid(format!(
            "research protocol {selected:?} has no configured review scopes; configured protocols: {}",
            protocols.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let selected = configured_scopes
        .iter()
        .filter(|(scope, _)| selected_scope.is_none_or(|selected| scope.as_str() == selected))
        .filter(|(_, protocols)| {
            selected_protocol.is_none_or(|selected| protocols.iter().any(|value| value == selected))
        })
        .map(|(scope, _)| scope.clone())
        .collect::<BTreeSet<_>>();
    if selected.is_empty()
        && let (Some(scope), Some(protocol)) = (selected_scope, selected_protocol)
    {
        return Err(crate::Error::invalid(format!(
            "review scope {scope:?} is not tagged with protocol {protocol:?}; configured scope protocols: {}",
            configured_scopes[scope].join(", ")
        )));
    }
    Ok(selected)
}

fn normalize_protocol_filter(value: &str) -> Result<&'static str> {
    crate::project::normalize_review_protocol_alias(value).ok_or_else(|| {
        crate::Error::invalid(format!(
            "unknown research protocol {value:?}; supported protocols: wifi, bluetooth (alias bt), ble, ieee802154 (aliases 802.15.4 and 802154), coex, shared"
        ))
    })
}

fn ranked_action_indices_for_focus(
    candidates: &[ResearchAction],
    strategy: ResearchRankingStrategy,
    focus: ResearchFocus,
) -> Vec<usize> {
    let mut indices = (0..candidates.len())
        .filter(|index| {
            action_matches_focus(&candidates[*index], focus)
                && (strategy != ResearchRankingStrategy::Frontier
                    || !candidates.iter().enumerate().any(|(other_index, other)| {
                        other_index != *index
                            && action_matches_focus(other, focus)
                            && actionability_lane(other) == actionability_lane(&candidates[*index])
                            && dominates(other, &candidates[*index])
                    }))
        })
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        let left = &candidates[*left];
        let right = &candidates[*right];
        actionability_lane(left)
            .cmp(&actionability_lane(right))
            .then_with(|| match strategy {
                ResearchRankingStrategy::Impact | ResearchRankingStrategy::Frontier => {
                    impact_order(left, right)
                }
                ResearchRankingStrategy::QuickWins => left
                    .score_explanation
                    .estimated_cost_units
                    .cmp(&right.score_explanation.estimated_cost_units)
                    .then_with(|| left.co_blockers.cmp(&right.co_blockers))
                    .then_with(|| impact_order(left, right)),
            })
    });
    indices
}

#[cfg(test)]
fn ranked_action_indices(
    candidates: &[ResearchAction],
    strategy: ResearchRankingStrategy,
) -> Vec<usize> {
    ranked_action_indices_for_focus(candidates, strategy, ResearchFocus::All)
}

fn action_matches_focus(action: &ResearchAction, focus: ResearchFocus) -> bool {
    focus == ResearchFocus::All
        || action
            .findings
            .iter()
            .any(|finding| finding_matches_focus(finding, focus))
}

fn finding_matches_focus(finding: &ResearchFinding, focus: ResearchFocus) -> bool {
    match focus {
        ResearchFocus::All => true,
        ResearchFocus::HardwareAccess => {
            matches!(&finding.subject, ResearchSubject::MmioRegister { .. })
                || finding
                    .reviewed_memory_access
                    .is_some_and(|classification| {
                        classification.role == crate::ReviewedMemoryAccessRole::HardwareShared
                    })
        }
    }
}

fn actionability_lane(action: &ResearchAction) -> u8 {
    if action.kinds.iter().any(|kind| kind == "analysis-surface")
        && action.actionability.ready.count != 0
    {
        0
    } else if action.actionability.ready.count != 0 {
        1
    } else if action.actionability.inspection_only.count != 0 {
        2
    } else {
        3
    }
}

fn impact_order(left: &ResearchAction, right: &ResearchAction) -> Ordering {
    right
        .score
        .cmp(&left.score)
        .then_with(|| right.guaranteed_unlock.cmp(&left.guaranteed_unlock))
        .then_with(|| right.optimistic_unlock.cmp(&left.optimistic_unlock))
        .then_with(|| left.id.cmp(&right.id))
}

fn dominates(left: &ResearchAction, right: &ResearchAction) -> bool {
    let left_score = &left.score_explanation;
    let right_score = &right.score_explanation;
    left_score.benefit_points >= right_score.benefit_points
        && left_score.effort_points <= right_score.effort_points
        && (left_score.benefit_points > right_score.benefit_points
            || left_score.effort_points < right_score.effort_points)
}

fn select_ranked_steps(
    prerequisites: &[ResearchPrerequisiteAction],
    prerequisite_indices: &[usize],
    actions: &[ResearchAction],
    action_indices: &[usize],
    limit: usize,
    budget: Option<u64>,
) -> (Vec<ResearchStepRef>, u64) {
    let mut selected = Vec::new();
    let mut consumed_budget = 0_u64;
    let mut prerequisite_lane = prerequisite_indices
        .iter()
        .map(|index| {
            let prerequisite = &prerequisites[*index];
            (
                ResearchStepRef {
                    kind: ResearchStepKind::Prerequisite,
                    id: prerequisite.id.clone(),
                },
                prerequisite.estimated_cost_units,
            )
        })
        .collect::<VecDeque<_>>();
    let mut action_lane = action_indices
        .iter()
        .map(|index| {
            let action = &actions[*index];
            (
                ResearchStepRef {
                    kind: ResearchStepKind::Action,
                    id: action.id.clone(),
                },
                action.score_explanation.estimated_cost_units,
            )
        })
        .collect::<VecDeque<_>>();
    let mut prerequisite_turn = true;
    while selected.len() < limit && (!prerequisite_lane.is_empty() || !action_lane.is_empty()) {
        let selected_preferred = if prerequisite_turn {
            take_fitting_step(
                &mut prerequisite_lane,
                &mut selected,
                &mut consumed_budget,
                budget,
            )
        } else {
            take_fitting_step(
                &mut action_lane,
                &mut selected,
                &mut consumed_budget,
                budget,
            )
        };
        let selected_alternate = selected_preferred
            || if prerequisite_turn {
                take_fitting_step(
                    &mut action_lane,
                    &mut selected,
                    &mut consumed_budget,
                    budget,
                )
            } else {
                take_fitting_step(
                    &mut prerequisite_lane,
                    &mut selected,
                    &mut consumed_budget,
                    budget,
                )
            };
        if !selected_alternate {
            break;
        }
        prerequisite_turn = !prerequisite_turn;
    }
    (selected, consumed_budget)
}

fn take_fitting_step(
    lane: &mut VecDeque<(ResearchStepRef, u64)>,
    selected: &mut Vec<ResearchStepRef>,
    consumed_budget: &mut u64,
    budget: Option<u64>,
) -> bool {
    while let Some((step, cost)) = lane.pop_front() {
        if budget.is_some_and(|budget| consumed_budget.saturating_add(cost) > budget) {
            continue;
        }
        *consumed_budget = consumed_budget.saturating_add(cost);
        selected.push(step);
        return true;
    }
    false
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResearchGraphFunction {
    identity: String,
    node: GraphNode,
}

#[derive(Clone, Copy)]
struct ResearchProfileInput<'a> {
    id: &'a str,
    output: &'a Path,
}

struct ResearchScopeInput<'a> {
    id: &'a str,
    profiles: &'a [String],
    function_identities: BTreeSet<&'a str>,
}

fn load_research_graphs(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
) -> Result<(DirectDiagnosticOwners, BTreeMap<String, ScopeGraph>)> {
    let profiles = session
        .project
        .ir_profiles
        .iter()
        .map(|profile| ResearchProfileInput {
            id: &profile.id,
            output: &profile.output,
        })
        .collect::<Vec<_>>();
    let scopes = scopes
        .iter()
        .map(|scope| ResearchScopeInput {
            id: &scope.id,
            profiles: &scope.profiles,
            function_identities: scope
                .function_identities
                .iter()
                .map(String::as_str)
                .collect(),
        })
        .collect::<Vec<_>>();
    load_research_graphs_with(&profiles, &scopes, |output, visit| {
        let projection = session.linked_ir(output)?.read_review_projection()?;
        for function in projection.functions {
            let direct_diagnostic_roots = function
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.channel == "direct")
                .map(|diagnostic| diagnostic.root_id.clone())
                .collect();
            let complete = function.completeness.body_complete
                && function.completeness.call_targets_complete
                && function.completeness.transitive_effects_complete
                && function.completeness.executable_complete
                && function.diagnostics.is_empty()
                && function.decode_blockers.is_empty();
            visit(ResearchGraphFunction {
                identity: function.identity,
                node: GraphNode {
                    source: function.source,
                    symbol: function.symbol,
                    dependencies: function.dependencies.into_iter().collect(),
                    direct_diagnostic_roots,
                    complete,
                },
            })?;
        }
        Ok(())
    })
}

fn load_research_graphs_with(
    profiles: &[ResearchProfileInput<'_>],
    scopes: &[ResearchScopeInput<'_>],
    mut load: impl FnMut(&Path, &mut dyn FnMut(ResearchGraphFunction) -> Result<()>) -> Result<()>,
) -> Result<(DirectDiagnosticOwners, BTreeMap<String, ScopeGraph>)> {
    let mut profile_outputs = BTreeMap::new();
    let mut scopes_by_output = BTreeMap::<&Path, BTreeSet<usize>>::new();
    for profile in profiles {
        if profile_outputs.insert(profile.id, profile.output).is_some() {
            return Err(crate::Error::invalid(format!(
                "duplicate IR profile {:?} in research graph inputs",
                profile.id
            )));
        }
        // Every project profile contributes exact direct-diagnostic ownership,
        // even when no selected scope contains one of its functions.
        scopes_by_output.entry(profile.output).or_default();
    }

    let mut graphs = BTreeMap::new();
    for (scope_index, scope) in scopes.iter().enumerate() {
        if graphs
            .insert(scope.id.to_owned(), ScopeGraph::default())
            .is_some()
        {
            return Err(crate::Error::invalid(format!(
                "duplicate research scope {:?}",
                scope.id
            )));
        }
        for profile_id in scope.profiles {
            let output = profile_outputs.get(profile_id.as_str()).ok_or_else(|| {
                crate::Error::invalid(format!("unknown IR profile {profile_id:?}"))
            })?;
            scopes_by_output
                .entry(*output)
                .or_default()
                .insert(scope_index);
        }
    }

    let mut owners = DirectDiagnosticOwners::new();
    for (output, scope_indices) in scopes_by_output {
        load(output, &mut |function| {
            for root_id in &function.node.direct_diagnostic_roots {
                owners
                    .entry(root_id.clone())
                    .or_default()
                    .insert(function.identity.clone());
            }
            for scope_index in &scope_indices {
                let scope = &scopes[*scope_index];
                if !scope
                    .function_identities
                    .contains(function.identity.as_str())
                {
                    continue;
                }
                let graph = graphs
                    .get_mut(scope.id)
                    .expect("every research scope has an initialized graph");
                merge_graph_node(
                    scope.id,
                    &mut graph.nodes,
                    &function.identity,
                    &function.node,
                )?;
            }
            Ok(())
        })?;
    }

    for graph in graphs.values_mut() {
        build_graph_edges(graph);
    }
    Ok((owners, graphs))
}

fn merge_graph_node(
    scope_id: &str,
    nodes: &mut BTreeMap<String, GraphNode>,
    identity: &str,
    node: &GraphNode,
) -> Result<()> {
    match nodes.entry(identity.to_owned()) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(node.clone());
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            let existing = entry.get_mut();
            if existing.source != node.source
                || existing.symbol != node.symbol
                || existing.dependencies != node.dependencies
                || existing.direct_diagnostic_roots != node.direct_diagnostic_roots
            {
                return Err(crate::Error::invalid(format!(
                    "scope {scope_id:?} has inconsistent projections for {identity:?}"
                )));
            }
            existing.complete &= node.complete;
        }
    }
    Ok(())
}

fn build_graph_edges(graph: &mut ScopeGraph) {
    for identity in graph.nodes.keys().cloned().collect::<Vec<_>>() {
        for dependency in graph.nodes[&identity].dependencies.clone() {
            if let Some(target) = resolve_function(graph, &dependency) {
                graph
                    .outgoing
                    .entry(identity.clone())
                    .or_default()
                    .insert(target.clone());
                graph
                    .incoming
                    .entry(target)
                    .or_default()
                    .insert(identity.clone());
            }
        }
    }
}

fn resolve_function(graph: &ScopeGraph, selector: &str) -> Option<String> {
    if graph.nodes.contains_key(selector) {
        return Some(selector.to_owned());
    }
    let matches = graph
        .nodes
        .iter()
        .filter(|(identity, node)| {
            node.symbol == selector
                || format!("{}:{}", node.source, node.symbol) == selector
                || identity.ends_with(&format!("::{selector}"))
        })
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0].clone())
}

fn reverse_reachable(graph: &ScopeGraph, starts: &BTreeSet<String>) -> BTreeSet<String> {
    let mut reached = starts.clone();
    let mut queue = VecDeque::from_iter(starts.iter().cloned());
    while let Some(function) = queue.pop_front() {
        for caller in graph.incoming.get(&function).into_iter().flatten() {
            if reached.insert(caller.clone()) {
                queue.push_back(caller.clone());
            }
        }
    }
    reached
}

fn blocker_inspection_targets(
    graph: &ScopeGraph,
    direct_diagnostic_owners: &DirectDiagnosticOwners,
    item: &crate::review_scopes::ReviewQueueItem,
) -> BTreeSet<String> {
    let mut inspection = direct_diagnostic_owners
        .get(&item.id)
        .cloned()
        .unwrap_or_default();
    inspection.extend(
        graph
            .nodes
            .iter()
            .filter(|(_, node)| node.direct_diagnostic_roots.contains(&item.id))
            .map(|(identity, _)| identity.clone()),
    );
    // Decode, unresolved-call and replacement findings are synthesized
    // outside the diagnostic-root channel. Their owning functions remain
    // the safest available inspection targets. A reference diagnostic whose
    // direct owner is absent from the configured inventory instead fails
    // closed: an impacted caller is not evidence of the causal location.
    if inspection.is_empty() && !item.channels.iter().any(|channel| channel == "reference") {
        inspection.extend(item.functions.iter().cloned());
    }
    inspection
}

fn add_blockers(
    project: &crate::ProjectSpec,
    scope: &ReviewScopeReport,
    graph: &ScopeGraph,
    direct_diagnostic_owners: &DirectDiagnosticOwners,
    reviewed_memory_accesses: &[crate::ReviewedMemoryAccessClassification],
    review_context: &open_radio_vendor_contracts::ApplicabilityContext,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let mut by_function = BTreeMap::<String, BTreeSet<String>>::new();
    for item in &scope.review_queue {
        for function in &item.functions {
            by_function
                .entry(function.clone())
                .or_default()
                .insert(item.id.clone());
        }
    }
    for item in &scope.review_queue {
        let reviewed_memory_access =
            classify_reviewed_memory_access(item, reviewed_memory_accesses, review_context);
        let blocker_resolution_route = crate::blocker_resolution::blocker_resolution_route(
            project,
            &item.id,
            &item.kind,
            &item.message,
        );
        let direct = item.functions.iter().cloned().collect::<BTreeSet<_>>();
        let inspection = blocker_inspection_targets(graph, direct_diagnostic_owners, item);
        let optimistic = reverse_reachable(graph, &direct);
        let co_blockers = direct
            .iter()
            .flat_map(|function| by_function.get(function).into_iter().flatten())
            .filter(|id| *id != &item.id)
            .cloned()
            .collect();
        let guaranteed = direct
            .iter()
            .filter(|function| {
                by_function
                    .get(*function)
                    .is_some_and(|blockers| blockers.len() == 1 && blockers.contains(&item.id))
                    && graph
                        .outgoing
                        .get(*function)
                        .into_iter()
                        .flatten()
                        .all(|dependency| graph.nodes[dependency].complete)
            })
            .cloned()
            .collect();
        merge(
            candidates,
            Seed {
                id: item.id.clone(),
                kind: item.kind.clone(),
                severity: item.severity.clone(),
                message: item.message.clone(),
                subject: ResearchSubject::AnalysisRoot {
                    root_id: item.id.clone(),
                },
                reviewed_memory_access,
                consumers: Vec::new(),
                blocker_resolution_route: Some(blocker_resolution_route),
                evidence_sites: item.sites.iter().copied().collect(),
                evidence_channels: item.channels.iter().cloned().collect(),
                inspection,
                guaranteed,
                optimistic,
                marginal: direct.clone(),
                direct,
                co_blockers,
                roots: item.affected_scope_roots.iter().cloned().collect(),
            },
            scope,
        )?;
    }
    Ok(())
}

fn classify_reviewed_memory_access(
    item: &crate::review_scopes::ReviewQueueItem,
    facts: &[crate::ReviewedMemoryAccessClassification],
    review_context: &open_radio_vendor_contracts::ApplicabilityContext,
) -> Option<crate::ReviewedMemoryAccessClassification> {
    let operation = match item.kind.as_str() {
        "memory-load" => crate::ReviewedMemoryAccessOperation::Load,
        "memory-store" => crate::ReviewedMemoryAccessOperation::Store,
        _ => return None,
    };
    let [function] = item.functions.as_slice() else {
        return None;
    };
    let [site] = item.sites.as_slice() else {
        return None;
    };
    facts.iter().copied().find(|fact| {
        fact.occurrence.function == function
            && fact.occurrence.site == *site
            && fact.occurrence.operation == operation
            && review_context.artifacts.iter().any(|artifact| {
                artifact.source() == fact.occurrence.artifact_source
                    && artifact.sha256() == fact.occurrence.artifact_sha256
            })
    })
}

fn add_incomplete_event_route_blockers(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let Some(workspace) = session.function_workspace()? else {
        return Ok(());
    };
    let route_ids = workspace
        .pack
        .event_routes
        .iter()
        .map(|route| route.id().to_owned())
        .collect::<Vec<_>>();
    if route_ids.len() > MAX_RESEARCH_EVENT_ROUTES {
        return Err(crate::Error::invalid(format!(
            "research event-route inventory contains {} routes, exceeding the bounded limit {MAX_RESEARCH_EVENT_ROUTES}",
            route_ids.len()
        )));
    }
    let reports = crate::flow_investigation::investigate_event_routes_with_workspace(
        &route_ids,
        12,
        &session.project,
        workspace,
    )?;
    for report in reports {
        add_incomplete_event_route_report(&report, scopes, graphs, candidates)?;
    }
    Ok(())
}

fn add_incomplete_event_route_report(
    report: &crate::flow_investigation::FlowInvestigationReport,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    if report.status != crate::flow_investigation::FlowStatus::Incomplete {
        return Ok(());
    }
    let route_id = report.route.as_deref().ok_or_else(|| {
        crate::Error::invalid("incomplete event-route report has no typed route identity")
    })?;
    let route_functions = event_route_report_functions(report);
    let evidence_sites = report
        .steps
        .iter()
        .filter_map(|step| step.site)
        .chain(report.effects.iter().filter_map(|effect| effect.site))
        .collect::<BTreeSet<_>>();
    let blocker_ids = report
        .blockers
        .iter()
        .map(|blocker| event_route_finding_id(route_id, &blocker.kind))
        .collect::<BTreeSet<_>>();
    for scope in scopes {
        let graph = &graphs[&scope.id];
        let Some(inspection) = event_route_scope_inspection(&route_functions, graph) else {
            continue;
        };
        for blocker in &report.blockers {
            let id = event_route_finding_id(route_id, &blocker.kind);
            merge(
                candidates,
                Seed {
                    id: id.clone(),
                    kind: blocker.kind.clone(),
                    severity: "warning".to_owned(),
                    message: format!(
                        "event route {route_id:?} remains incomplete: {}",
                        blocker.message
                    ),
                    subject: ResearchSubject::EventRouteBlocker {
                        route_id: route_id.to_owned(),
                        blocker_kind: blocker.kind.clone(),
                    },
                    reviewed_memory_access: None,
                    consumers: Vec::new(),
                    blocker_resolution_route: Some(
                        crate::blocker_resolution::event_route_blocker_resolution_route(
                            route_id,
                            &blocker.kind,
                        ),
                    ),
                    evidence_sites: evidence_sites.clone(),
                    evidence_channels: ["event-route".to_owned()].into(),
                    inspection: inspection.clone(),
                    direct: BTreeSet::new(),
                    guaranteed: BTreeSet::new(),
                    optimistic: BTreeSet::new(),
                    marginal: BTreeSet::new(),
                    co_blockers: blocker_ids
                        .iter()
                        .filter(|other| *other != &id)
                        .cloned()
                        .collect(),
                    roots: BTreeSet::new(),
                },
                scope,
            )?;
        }
    }
    Ok(())
}

fn event_route_scope_inspection(
    route_functions: &BTreeSet<String>,
    graph: &ScopeGraph,
) -> Option<BTreeSet<String>> {
    let inspection = route_functions
        .iter()
        .filter_map(|function| resolve_function(graph, function))
        .collect::<BTreeSet<_>>();
    (!inspection.is_empty()).then_some(inspection)
}

fn event_route_report_functions(
    report: &crate::flow_investigation::FlowInvestigationReport,
) -> BTreeSet<String> {
    std::iter::once(report.root.clone())
        .chain(report.target.iter().cloned())
        .chain(
            report
                .steps
                .iter()
                .flat_map(|step| [step.caller.clone(), step.callee.clone()]),
        )
        .chain(report.effects.iter().map(|effect| effect.function.clone()))
        .collect()
}

fn event_route_finding_id(route_id: &str, blocker_kind: &str) -> String {
    stable_id(
        "event-route-blocker",
        &crate::blocker_resolution::event_route_blocker_root(route_id, blocker_kind),
    )
}

fn required_analysis_surface_state(
    source_available: bool,
    inventory_available: bool,
    family_matched: bool,
    profile_configured: bool,
    profile_output_present: bool,
    profile_output_valid: bool,
) -> Option<ResearchAnalysisSurfaceState> {
    if !source_available {
        Some(ResearchAnalysisSurfaceState::MissingVendorArtifact)
    } else if !inventory_available {
        Some(ResearchAnalysisSurfaceState::MissingSymbolInventory)
    } else if !family_matched {
        Some(ResearchAnalysisSurfaceState::StaleSymbolFamily)
    } else if !profile_configured {
        Some(ResearchAnalysisSurfaceState::MissingProfileDefinition)
    } else if !profile_output_present {
        Some(ResearchAnalysisSurfaceState::MissingProfileOutput)
    } else if !profile_output_valid {
        Some(ResearchAnalysisSurfaceState::InvalidProfileOutput)
    } else {
        None
    }
}

fn add_required_analysis_surface_findings(
    session: &ProjectSession,
    selected_protocol: Option<&str>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let available_sources = session
        .run_spec
        .iter()
        .flat_map(crate::run_spec::RunSpec::inputs)
        .filter(|input| input.path.is_file())
        .filter_map(|input| match &input.role {
            crate::run_spec::InputRole::SourceArtifact(source) => Some(source.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let inventory = session
        .project
        .symbol_inventory
        .as_ref()
        .filter(|spec| spec.output.is_file())
        .map(|spec| {
            std::fs::read_to_string(&spec.output)
                .map_err(|error| error.to_string())
                .and_then(|input| {
                    crate::artifacts::parse_symbol_inventory(&input)
                        .map_err(|error| error.to_string())
                })
        })
        .transpose();
    let inventory_error = inventory.as_ref().err().cloned();
    let inventory = inventory.ok().flatten();
    for family in &session.project.analysis_symbol_families {
        if family.disposition != crate::project::AnalysisSymbolFamilyDisposition::Required
            || selected_protocol
                .is_some_and(|protocol| !family.protocols.iter().any(|item| item == protocol))
        {
            continue;
        }
        let source_available = available_sources.contains(family.source.as_str());
        let profile = family.profile.as_ref().and_then(|expected| {
            session
                .project
                .ir_profiles
                .iter()
                .find(|profile| &profile.id == expected)
        });
        let matched = inventory.as_ref().map(|inventory| {
            let artifact_sources = inventory
                .artifacts
                .iter()
                .map(|artifact| {
                    (
                        artifact.index,
                        artifact.sources.iter().cloned().collect::<BTreeSet<_>>(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let symbols = inventory
                .symbols
                .iter()
                .map(|symbol| (symbol.artifact, symbol.name.clone()))
                .collect::<Vec<_>>();
            super::status::matching_symbol_identities(
                &family.source,
                &family.symbol_prefix,
                &artifact_sources,
                &symbols,
            )
        });
        let profile_output_present = profile.is_some_and(|profile| profile.output.is_dir());
        let profile_inspection = profile
            .filter(|_| profile_output_present)
            .map(|profile| crate::artifacts::inspect_linked_ir(&profile.output));
        let profile_output_valid = matches!(
            profile_inspection.as_ref(),
            Some(Ok(summary)) if summary.functions != 0
        );
        let Some(state) = required_analysis_surface_state(
            source_available,
            inventory.is_some(),
            matched.as_ref().is_some_and(|matched| !matched.is_empty()),
            profile.is_some(),
            profile_output_present,
            profile_output_valid,
        ) else {
            continue;
        };
        let diagnostic = match state {
            ResearchAnalysisSurfaceState::MissingVendorArtifact => format!(
                "required public symbol family {:?} is blocked: source-artifact:{} is not bound to an existing file",
                family.id, family.source
            ),
            ResearchAnalysisSurfaceState::MissingSymbolInventory => inventory_error
                .clone()
                .unwrap_or_else(|| {
                    format!(
                        "required public symbol family {:?} cannot be checked because the generated symbol inventory is absent",
                        family.id
                    )
                }),
            ResearchAnalysisSurfaceState::StaleSymbolFamily => format!(
                "required public symbol family {:?} prefix {:?} matched no symbols from source {:?}",
                family.id, family.symbol_prefix, family.source
            ),
            ResearchAnalysisSurfaceState::MissingProfileDefinition => format!(
                "required public symbol family {:?} has no configured linked-IR profile {:?}",
                family.id,
                family.profile.as_deref().unwrap_or("<not-configured>")
            ),
            ResearchAnalysisSurfaceState::MissingProfileOutput => format!(
                "required public symbol family {:?} linked-IR profile {:?} has no generated output",
                family.id,
                profile.expect("state requires a configured profile").id
            ),
            ResearchAnalysisSurfaceState::InvalidProfileOutput => match profile_inspection.as_ref() {
                Some(Ok(_)) => format!(
                    "required public symbol family {:?} linked-IR profile {:?} contains zero functions",
                    family.id,
                    profile.expect("state requires a configured profile").id
                ),
                Some(Err(error)) => format!(
                    "required public symbol family {:?} linked-IR profile {:?} is invalid: {error}",
                    family.id,
                    profile.expect("state requires a configured profile").id
                ),
                None => unreachable!("invalid profile state requires inspected output"),
            },
        };
        let id = stable_id("analysis-surface", &family.id);
        let subject = ResearchSubject::PublicSymbolFamily {
            surface: family.id.clone(),
            protocols: family.protocols.clone(),
            source: family.source.clone(),
            symbol_prefix: family.symbol_prefix.clone(),
            profile: family.profile.clone(),
            state,
        };
        let consumer = ResearchConsumer::RequiredAnalysisSurface {
            state,
            source: family.source.clone(),
            profile: family.profile.clone(),
            output: profile.map(|profile| profile.output.clone()),
            project_manifest: session.manifest.clone(),
            working_directory: session.invocation_directory.clone(),
            target_spec_override: session.explicit_context.target_spec.clone(),
            run_spec_override: session.explicit_context.run_spec.clone(),
            svd_overrides: session.explicit_context.svd_paths.clone(),
            run_spec: session.run_spec_path.clone(),
            diagnostic: diagnostic.clone(),
        };
        if candidates
            .insert(
                id.clone(),
                Accumulator {
                    id,
                    kind: "analysis-surface".to_owned(),
                    severity: "error".to_owned(),
                    message: diagnostic,
                    subject,
                    reviewed_memory_access: None,
                    consumers: vec![consumer],
                    blocker_resolution_route: None,
                    evidence_sites: BTreeSet::new(),
                    evidence_channels: ["project-analysis-surface".to_owned()].into(),
                    inspection: BTreeSet::new(),
                    direct: BTreeSet::new(),
                    guaranteed: BTreeSet::new(),
                    optimistic: BTreeSet::new(),
                    marginal: BTreeSet::new(),
                    co_blockers: BTreeSet::new(),
                    roots: BTreeSet::new(),
                    scopes: BTreeSet::new(),
                    publication_scopes: BTreeSet::new(),
                },
            )
            .is_some()
        {
            return Err(crate::Error::invalid(format!(
                "required analysis surface {:?} collides with another research finding identity",
                family.id
            )));
        }
    }
    Ok(())
}

fn merge(
    candidates: &mut BTreeMap<String, Accumulator>,
    seed: Seed,
    scope: &ReviewScopeReport,
) -> Result<()> {
    if let Some(existing) = candidates.get(&seed.id)
        && (existing.kind != seed.kind
            || existing.severity != seed.severity
            || existing.subject != seed.subject
            || existing.reviewed_memory_access != seed.reviewed_memory_access
            || existing.consumers != seed.consumers
            || existing.blocker_resolution_route != seed.blocker_resolution_route)
    {
        return Err(crate::Error::invalid(format!(
            "research finding id {:?} resolves to conflicting typed subjects or consumers",
            seed.id
        )));
    }
    let message = seed.message.clone();
    let item = candidates
        .entry(seed.id.clone())
        .or_insert_with(|| Accumulator {
            id: seed.id,
            kind: seed.kind,
            severity: seed.severity,
            message: seed.message,
            subject: seed.subject,
            reviewed_memory_access: seed.reviewed_memory_access,
            consumers: seed.consumers,
            blocker_resolution_route: seed.blocker_resolution_route,
            evidence_sites: BTreeSet::new(),
            evidence_channels: BTreeSet::new(),
            inspection: BTreeSet::new(),
            direct: BTreeSet::new(),
            guaranteed: BTreeSet::new(),
            optimistic: BTreeSet::new(),
            marginal: BTreeSet::new(),
            co_blockers: BTreeSet::new(),
            roots: BTreeSet::new(),
            scopes: BTreeSet::new(),
            publication_scopes: BTreeSet::new(),
        });
    if message < item.message {
        item.message = message;
    }
    item.evidence_sites.extend(seed.evidence_sites);
    item.evidence_channels.extend(seed.evidence_channels);
    item.inspection.extend(seed.inspection);
    item.direct.extend(seed.direct);
    item.guaranteed.extend(seed.guaranteed);
    item.optimistic.extend(seed.optimistic);
    item.marginal.extend(seed.marginal);
    item.co_blockers.extend(seed.co_blockers);
    item.roots.extend(seed.roots);
    item.scopes.insert(scope.id.clone());
    if scope.publication && !matches!(&item.subject, ResearchSubject::EventRouteBlocker { .. }) {
        item.publication_scopes.insert(scope.id.clone());
    }
    Ok(())
}

fn add_registers(
    session: &ProjectSession,
    paths: &crate::project::RegisterWorkspacePaths,
    workspace: &ProjectRegisterWorkspace,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let facts = workspace.required_facts()?;
    let model = workspace.model();
    let address_space = model.address_space().to_owned();
    let reviewed_identities = reviewed_register_identities(model)?;
    for fact in &facts.registers {
        if reviewed_identities.contains_key(&(u64::from(fact.address), u32::from(fact.width))) {
            continue;
        }
        let ownership =
            classify_register_publication(facts, &paths.owned_ranges, fact.address, fact.width)?;
        let message = match ownership {
            RegisterPublicationOwnership::Owned(_) => format!(
                "name and review MMIO {:#010x}/{} before publication",
                fact.address, fact.width
            ),
            RegisterPublicationOwnership::External(range) => format!(
                "inspect external MMIO {:#010x}/{} in range {:?}; the range is outside [registers].owned-ranges and cannot be published to project default reviewed facts",
                fact.address, fact.width, range.name
            ),
        };
        add_register_seed(
            scopes,
            graphs,
            candidates,
            fact,
            SeedTemplate {
                id: format!("register-{:#010x}-{}", fact.address, fact.width),
                kind: "register-model".to_owned(),
                severity: "warning".to_owned(),
                message,
                subject: ResearchSubject::MmioRegister {
                    address_space: address_space.clone(),
                    address: fact.address,
                    width: fact.width,
                    assertion: None,
                },
                consumers: register_publication_consumers(ownership, || {
                    reviewed_knowledge_consumer(session, &["register-identity"])
                }),
            },
        )?;
    }
    Ok(())
}

struct SeedTemplate {
    id: String,
    kind: String,
    severity: String,
    message: String,
    subject: ResearchSubject,
    consumers: Vec<ResearchConsumer>,
}

fn reviewed_knowledge_consumer(
    session: &ProjectSession,
    assertion_kinds: &[&str],
) -> ResearchConsumer {
    let mut configured_paths = session.project.reviewed_knowledge.clone();
    configured_paths.sort();
    let (resolution, selected_path, diagnostic) = if let Some(path) =
        session.project.reviewed_knowledge_default.as_ref()
    {
        (ResearchConsumerResolution::Ready, Some(path.clone()), None)
    } else {
        (
            ResearchConsumerResolution::NeedsDestination,
            None,
            Some(if configured_paths.is_empty() {
                "project has no project-local reviewed-knowledge destination".to_owned()
            } else {
                "select [reviewed-knowledge].default-pack explicitly; pack count is never a routing rule"
                        .to_owned()
            }),
        )
    };
    ResearchConsumer::ReviewedKnowledgeAssertions {
        resolution,
        configured_paths,
        selected_path,
        assertion_kinds: assertion_kinds
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect(),
        diagnostic,
    }
}

fn register_publication_consumers(
    ownership: RegisterPublicationOwnership<'_>,
    consumer: impl FnOnce() -> ResearchConsumer,
) -> Vec<ResearchConsumer> {
    if ownership.is_owned() {
        vec![consumer()]
    } else {
        Vec::new()
    }
}

fn add_register_seed(
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
    fact: &crate::registers::RegisterFact,
    template: SeedTemplate,
) -> Result<()> {
    for scope in scopes {
        let graph = &graphs[&scope.id];
        let direct = register_fact_functions_in_scope(fact, graph);
        if direct.is_empty() {
            continue;
        }
        merge(
            candidates,
            Seed {
                id: template.id.clone(),
                kind: template.kind.clone(),
                severity: template.severity.clone(),
                message: template.message.clone(),
                subject: template.subject.clone(),
                reviewed_memory_access: None,
                consumers: template.consumers.clone(),
                blocker_resolution_route: None,
                evidence_sites: fact
                    .read_sites
                    .iter()
                    .chain(&fact.write_sites)
                    .map(|site| site.pc)
                    .collect(),
                evidence_channels: ["mmio".to_owned()].into(),
                inspection: direct.clone(),
                guaranteed: BTreeSet::new(),
                optimistic: reverse_reachable(graph, &direct),
                marginal: direct.clone(),
                direct,
                co_blockers: BTreeSet::new(),
                roots: BTreeSet::new(),
            },
            scope,
        )?;
    }
    Ok(())
}

fn register_fact_functions_in_scope(
    fact: &crate::registers::RegisterFact,
    graph: &ScopeGraph,
) -> BTreeSet<String> {
    fact.read_functions
        .iter()
        .chain(&fact.write_functions)
        .filter_map(|function| resolve_function(graph, function))
        .collect()
}

fn add_unknown_semantics(
    session: &ProjectSession,
    paths: &crate::project::RegisterWorkspacePaths,
    workspace: &ProjectRegisterWorkspace,
    knowledge: &open_radio_vendor_review::ReviewKnowledge,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let facts = workspace.required_facts()?;
    let model_address_space = workspace.model().address_space().to_owned();
    for assertion in knowledge.assertions().values().filter(|assertion| {
        assertion.kind == "hardware-write-semantics"
            && matches!(&assertion.value, open_radio_vendor_review::AssertionValue::String(value) if value == "unknown")
    }) {
        let Some((chip, address_space, address, width)) = register_entity(&assertion.subject) else {
            continue;
        };
        if chip != workspace.model().chip() || address_space != model_address_space {
            continue;
        }
        let Some(fact) = facts
            .registers
            .iter()
            .find(|fact| fact.address == address && fact.width == width)
        else {
            continue;
        };
        let ownership = classify_register_publication(
            facts,
            &paths.owned_ranges,
            fact.address,
            fact.width,
        )?;
        let message = match ownership {
            RegisterPublicationOwnership::Owned(_) => format!(
                "prove write semantics for {address:#010x}/{width}; software access cannot prove W1C/self-clear"
            ),
            RegisterPublicationOwnership::External(range) => format!(
                "inspect unknown write semantics for external MMIO {address:#010x}/{width} in range {:?}; the range is outside [registers].owned-ranges and cannot update project default reviewed facts",
                range.name
            ),
        };
        add_register_seed(
            scopes,
            graphs,
            candidates,
            fact,
            SeedTemplate {
                id: format!("semantic-{}", assertion.id),
                kind: "register-write-semantics".to_owned(),
                severity: "warning".to_owned(),
                message,
                subject: ResearchSubject::MmioRegister {
                    address_space: address_space.to_owned(),
                    address,
                    width,
                    assertion: Some(assertion.id.clone()),
                },
                consumers: register_publication_consumers(ownership, || {
                    reviewed_knowledge_consumer(session, &["hardware-write-semantics"])
                }),
            },
        )?;
    }
    Ok(())
}

fn selected_review_knowledge(
    session: &ProjectSession,
) -> Result<open_radio_vendor_review::ReviewKnowledge> {
    open_radio_vendor_review::ReviewKnowledge::load_all(&session.project.reviewed_knowledge)
        .and_then(|knowledge| knowledge.select_for(&session.project.review_context))
        .map_err(|error| {
            crate::Error::invalid(format!("cannot prioritize reviewed knowledge: {error}"))
        })
}

fn register_entity(subject: &SemanticEntityId) -> Option<(&str, &str, u32, u8)> {
    let SemanticEntityId::Register {
        chip,
        address_space,
        address,
        width,
    } = subject
    else {
        return None;
    };
    Some((
        chip,
        address_space,
        (*address).try_into().ok()?,
        (*width).try_into().ok()?,
    ))
}

fn add_interfaces(
    session: &ProjectSession,
    observations: &[crate::application::capability_context::InterfaceResearchObservation],
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    for observation in observations {
        let id = interface_finding_id(&observation.id);
        for scope in scopes {
            let graph = &graphs[&scope.id];
            let direct = observation
                .functions
                .iter()
                .filter_map(|function| resolve_function(graph, function))
                .collect::<BTreeSet<_>>();
            if direct.is_empty() {
                continue;
            }
            merge(
                candidates,
                Seed {
                    id: id.clone(),
                    kind: "interface-layout".to_owned(),
                    severity: "warning".to_owned(),
                    message: format!(
                        "review interface slot {} at {:+#x}/{}",
                        observation.contract, observation.offset, observation.width
                    ),
                    subject: ResearchSubject::InterfaceObservation {
                        observation: observation.id.clone(),
                        contract: observation.contract.clone(),
                        source: observation.source.clone(),
                        offset: observation.offset,
                        width: observation.width,
                        selector: observation.selector.clone(),
                        call_sites: observation.call_sites.clone(),
                    },
                    reviewed_memory_access: None,
                    consumers: vec![interface_consumer(session, observation)],
                    blocker_resolution_route: None,
                    evidence_sites: observation.call_sites.iter().copied().collect(),
                    evidence_channels: ["interface".to_owned()].into(),
                    inspection: direct.clone(),
                    guaranteed: BTreeSet::new(),
                    optimistic: reverse_reachable(graph, &direct),
                    marginal: direct.clone(),
                    direct,
                    co_blockers: BTreeSet::new(),
                    roots: BTreeSet::new(),
                },
                scope,
            )?;
        }
    }
    Ok(())
}

fn interface_finding_id(observation_id: &str) -> String {
    stable_id("interface", observation_id)
}

fn interface_consumer(
    session: &ProjectSession,
    observation: &crate::application::capability_context::InterfaceResearchObservation,
) -> ResearchConsumer {
    let path = session
        .project
        .interfaces
        .as_ref()
        .and_then(|paths| paths.pack.clone());
    interface_consumer_with_path(path, observation)
}

fn interface_consumer_with_path(
    path: Option<PathBuf>,
    observation: &crate::application::capability_context::InterfaceResearchObservation,
) -> ResearchConsumer {
    let resolution = match observation.resolution {
        crate::application::capability_context::InterfaceObservationResolution::Ready => {
            ResearchConsumerResolution::Ready
        }
        crate::application::capability_context::InterfaceObservationResolution::NeedsAnchor => {
            ResearchConsumerResolution::NeedsAnchor
        }
    };
    ResearchConsumer::InterfacePackSlot {
        resolution,
        path,
        contract: observation.contract.clone(),
        anchor: observation.anchor.clone(),
        template: observation.template.clone(),
        offset: observation.offset,
        width: observation.width,
        diagnostic: observation.diagnostic.clone(),
    }
}

fn stable_id(kind: &str, identity: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in kind
        .bytes()
        .chain(std::iter::once(0))
        .chain(identity.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("research-{hash:016x}")
}

fn attach_candidate_co_blockers(candidates: &mut BTreeMap<String, Accumulator>) {
    let mut by_function = BTreeMap::<(&'static str, String), BTreeSet<String>>::new();
    for candidate in candidates.values() {
        let domain = candidate_domain(&candidate.kind);
        for function in &candidate.direct {
            by_function
                .entry((domain, function.clone()))
                .or_default()
                .insert(candidate.id.clone());
        }
    }
    for candidate in candidates.values_mut() {
        let domain = candidate_domain(&candidate.kind);
        for function in &candidate.direct {
            candidate.co_blockers.extend(
                by_function[&(domain, function.clone())]
                    .iter()
                    .filter(|id| *id != &candidate.id)
                    .cloned(),
            );
        }
    }
}

struct ExactFindingResolution {
    state: ResearchFindingQueryState,
    interpretation: String,
    evidence: Option<ResearchFindingResolutionEvidence>,
}

struct ExactRegisterResolutionContext<'a> {
    paths: &'a crate::project::RegisterWorkspacePaths,
    workspace: &'a ProjectRegisterWorkspace,
    knowledge: &'a open_radio_vendor_review::ReviewKnowledge,
    configured_scopes: &'a [&'a ReviewScopeReport],
    selected_scopes: &'a [&'a ReviewScopeReport],
    graphs: &'a BTreeMap<String, ScopeGraph>,
}

fn resolve_exact_without_register_workspace(
    requested: Option<&str>,
) -> Option<ExactFindingResolution> {
    let (address, width) = parse_register_finding_id(requested?)?;
    Some(ExactFindingResolution {
        state: ResearchFindingQueryState::InputNotObserved,
        interpretation: "the register finding cannot be resolved because the project has no current MMIO discovery workspace".to_owned(),
        evidence: Some(ResearchFindingResolutionEvidence::RegisterWorkspaceAbsent {
            address,
            width,
        }),
    })
}

fn resolve_exact_register_finding(
    requested: Option<&str>,
    context: &ExactRegisterResolutionContext<'_>,
) -> Result<Option<ExactFindingResolution>> {
    let ExactRegisterResolutionContext {
        paths,
        workspace,
        knowledge,
        configured_scopes,
        selected_scopes,
        graphs,
    } = context;
    let Some(requested) = requested else {
        return Ok(None);
    };
    if let Some((address, width)) = parse_register_finding_id(requested) {
        return resolve_register_model_finding(context, address, width).map(Some);
    }
    let Some(assertion_id) = requested.strip_prefix("semantic-") else {
        return Ok(None);
    };
    let Some(assertion) = knowledge.assertions().get(assertion_id) else {
        return Ok(None);
    };
    if assertion.kind != "hardware-write-semantics" || assertion.metadata.evidence.is_empty() {
        return Ok(None);
    }
    let Some(effective_write_semantics) = normalize_write_semantics(&assertion.value) else {
        return Ok(None);
    };
    let Some((chip, address_space, address, width)) = register_entity(&assertion.subject) else {
        return Ok(None);
    };
    if chip != workspace.model().chip() || address_space != workspace.model().address_space() {
        return Ok(None);
    }
    let subject = ResearchRegisterResolutionSubject {
        chip: chip.to_owned(),
        address_space: address_space.to_owned(),
        address,
        width,
    };
    let facts = workspace.required_facts()?;
    let Some(fact) = facts
        .registers
        .iter()
        .find(|fact| fact.address == address && fact.width == width)
    else {
        return Ok(Some(register_resolution(
            ExactRegisterPredicate::UnknownHardwareWriteSemantics {
                assertion_id: assertion.id.clone(),
                effective_write_semantics: effective_write_semantics.clone(),
            },
            ResearchFindingQueryState::InputNotObserved,
            ExactRegisterEvidence {
                subject,
                current_observation: None,
                current_identity: None,
                matching_scopes: Vec::new(),
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the exact semantic assertion remains configured, but its physical register is not observed in current MMIO discovery facts",
        )));
    };
    let observation = register_observation_evidence(facts, paths, fact)?;
    let matching_scopes = matching_register_scopes(fact, configured_scopes, graphs);
    if matching_scopes.is_empty() {
        return Ok(Some(register_resolution(
            ExactRegisterPredicate::UnknownHardwareWriteSemantics {
                assertion_id: assertion.id.clone(),
                effective_write_semantics: effective_write_semantics.clone(),
            },
            ResearchFindingQueryState::InputNotObserved,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation.clone()),
                current_identity: None,
                matching_scopes: Vec::new(),
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the physical register is present in discovery facts but no configured review scope observes it through the current function graphs",
        )));
    }
    if !selected_scope_intersects(&matching_scopes, selected_scopes) {
        return Ok(Some(register_resolution(
            ExactRegisterPredicate::UnknownHardwareWriteSemantics {
                assertion_id: assertion.id.clone(),
                effective_write_semantics: effective_write_semantics.clone(),
            },
            ResearchFindingQueryState::FilteredOut,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation.clone()),
                current_identity: None,
                matching_scopes,
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the physical register is observed by configured scopes, but none intersects the selected research filter",
        )));
    }
    let identities = workspace.model().register_identities()?;
    let current_identity = identities
        .get(&(u64::from(address), u32::from(width)))
        .cloned();
    let model_sources = model_sources(workspace, address, width)?;
    let is_unknown = effective_write_semantics == "unknown";
    let state = semantic_resolution_state(
        &effective_write_semantics,
        observation.publication_ownership,
        current_identity.as_deref(),
    );
    Ok(Some(register_resolution(
        ExactRegisterPredicate::UnknownHardwareWriteSemantics {
            assertion_id: assertion.id.clone(),
            effective_write_semantics,
        },
        state,
        ExactRegisterEvidence {
            subject,
            current_observation: Some(observation),
            current_identity,
            matching_scopes,
            applied_assertions: vec![assertion.clone()],
            model_sources,
        },
        if is_unknown {
            "the exact selected semantic assertion still records unknown hardware write semantics"
        } else if state == ResearchFindingQueryState::ConditionSatisfied {
            "the current producer predicate is false: the exact selected assertion records supported non-unknown write semantics with retained evidence for a project-owned modeled register"
        } else {
            "the finding is not current, but a non-unknown assertion is not satisfaction proof without a project-owned current effective register identity"
        },
    )))
}

fn resolve_register_model_finding(
    context: &ExactRegisterResolutionContext<'_>,
    address: u32,
    width: u8,
) -> Result<ExactFindingResolution> {
    let ExactRegisterResolutionContext {
        paths,
        workspace,
        knowledge,
        configured_scopes,
        selected_scopes,
        graphs,
    } = context;
    let subject = ResearchRegisterResolutionSubject {
        chip: workspace.model().chip().to_owned(),
        address_space: workspace.model().address_space().to_owned(),
        address,
        width,
    };
    let facts = workspace.required_facts()?;
    let Some(fact) = facts
        .registers
        .iter()
        .find(|fact| fact.address == address && fact.width == width)
    else {
        return Ok(register_resolution(
            ExactRegisterPredicate::AbsentRegisterModel,
            ResearchFindingQueryState::InputNotObserved,
            ExactRegisterEvidence {
                subject,
                current_observation: None,
                current_identity: None,
                matching_scopes: Vec::new(),
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the exact register is absent from current MMIO discovery facts",
        ));
    };
    let observation = register_observation_evidence(facts, paths, fact)?;
    let matching_scopes = matching_register_scopes(fact, configured_scopes, graphs);
    if matching_scopes.is_empty() {
        return Ok(register_resolution(
            ExactRegisterPredicate::AbsentRegisterModel,
            ResearchFindingQueryState::InputNotObserved,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation.clone()),
                current_identity: None,
                matching_scopes: Vec::new(),
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the register is present in discovery facts but no configured review scope observes it through the current function graphs",
        ));
    }
    if !selected_scope_intersects(&matching_scopes, selected_scopes) {
        return Ok(register_resolution(
            ExactRegisterPredicate::AbsentRegisterModel,
            ResearchFindingQueryState::FilteredOut,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation.clone()),
                current_identity: None,
                matching_scopes,
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            },
            "the register is observed by configured scopes, but none intersects the selected research filter",
        ));
    }
    let identities = workspace.model().register_identities()?;
    let current_identity = identities
        .get(&(u64::from(address), u32::from(width)))
        .cloned();
    let model_sources = model_sources(workspace, address, width)?;
    if current_identity.is_none() {
        return Ok(register_resolution(
            ExactRegisterPredicate::AbsentRegisterModel,
            ResearchFindingQueryState::Open,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation),
                current_identity: None,
                matching_scopes,
                applied_assertions: Vec::new(),
                model_sources,
            },
            "the exact observed and relevant register still has no current effective register identity",
        ));
    }
    let retained = workspace
        .model()
        .reviewed_register_facts()
        .iter()
        .find(|assertion| {
            assertion.kind == "register-identity"
                && register_entity(&assertion.subject).is_some_and(
                    |(chip, address_space, assertion_address, assertion_width)| {
                        chip == workspace.model().chip()
                            && address_space == workspace.model().address_space()
                            && assertion_address == address
                            && assertion_width == width
                    },
                )
                && matches!(
                    &assertion.value,
                    open_radio_vendor_review::AssertionValue::String(identity)
                        if current_identity.as_deref() == Some(identity)
                )
        })
        .and_then(|retained| {
            let configured = knowledge.assertions().get(&retained.id)?;
            (configured == retained
                && register_identity_assertion_matches(
                    current_identity.as_deref()?,
                    std::slice::from_ref(configured),
                ))
            .then(|| vec![configured.clone()])
        });
    let state =
        modeled_register_resolution_state(observation.publication_ownership, retained.is_some());
    if state == ResearchFindingQueryState::ConditionSatisfied {
        let applied_assertions = retained.expect("satisfied register identity is retained");
        return Ok(register_resolution(
            ExactRegisterPredicate::AbsentRegisterModel,
            state,
            ExactRegisterEvidence {
                subject,
                current_observation: Some(observation),
                current_identity,
                matching_scopes,
                applied_assertions,
                model_sources,
            },
            "the current producer predicate is false: the observed relevant project-owned register has one exact retained reviewed identity assertion",
        ));
    }
    Ok(register_resolution(
        ExactRegisterPredicate::AbsentRegisterModel,
        ResearchFindingQueryState::NotPresent,
        ExactRegisterEvidence {
            subject,
            current_observation: Some(observation),
            current_identity,
            matching_scopes,
            applied_assertions: Vec::new(),
            model_sources,
        },
        "the finding is not current, but no exact retained reviewed identity assertion attributes that absence; a base-model identity alone is not satisfaction proof",
    ))
}

fn semantic_resolution_state(
    effective_write_semantics: &str,
    ownership: ResearchRegisterPublicationOwnership,
    current_identity: Option<&str>,
) -> ResearchFindingQueryState {
    if effective_write_semantics == "unknown" {
        ResearchFindingQueryState::Open
    } else if ownership == ResearchRegisterPublicationOwnership::Owned && current_identity.is_some()
    {
        ResearchFindingQueryState::ConditionSatisfied
    } else {
        ResearchFindingQueryState::NotPresent
    }
}

fn modeled_register_resolution_state(
    ownership: ResearchRegisterPublicationOwnership,
    retained_identity: bool,
) -> ResearchFindingQueryState {
    if ownership == ResearchRegisterPublicationOwnership::Owned && retained_identity {
        ResearchFindingQueryState::ConditionSatisfied
    } else {
        ResearchFindingQueryState::NotPresent
    }
}

fn matching_register_scopes(
    fact: &crate::registers::RegisterFact,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
) -> Vec<String> {
    scopes
        .iter()
        .filter(|scope| !register_fact_functions_in_scope(fact, &graphs[&scope.id]).is_empty())
        .map(|scope| scope.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn selected_scope_intersects(
    matching_scopes: &[String],
    selected_scopes: &[&ReviewScopeReport],
) -> bool {
    let selected = selected_scopes
        .iter()
        .map(|scope| scope.id.as_str())
        .collect::<BTreeSet<_>>();
    matching_scopes
        .iter()
        .any(|scope| selected.contains(scope.as_str()))
}

fn model_sources(
    workspace: &ProjectRegisterWorkspace,
    address: u32,
    width: u8,
) -> Result<Vec<String>> {
    Ok(workspace
        .model()
        .register_review_annotations()?
        .get(&(u64::from(address), u32::from(width)))
        .into_iter()
        .flat_map(|annotation| annotation.sources.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn register_observation_evidence(
    facts: &crate::registers::RegisterFacts,
    paths: &crate::project::RegisterWorkspacePaths,
    fact: &crate::registers::RegisterFact,
) -> Result<ResearchRegisterObservationEvidence> {
    let ownership =
        classify_register_publication(facts, &paths.owned_ranges, fact.address, fact.width)?;
    let (range, publication_ownership) = match ownership {
        RegisterPublicationOwnership::Owned(range) => {
            (range, ResearchRegisterPublicationOwnership::Owned)
        }
        RegisterPublicationOwnership::External(range) => {
            (range, ResearchRegisterPublicationOwnership::External)
        }
    };
    let analysis_artifacts = facts
        .artifacts
        .iter()
        .map(|artifact| ResearchRegisterObservationArtifact {
            source: artifact.source.clone(),
            sha256: artifact.sha256.clone(),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ResearchRegisterObservationEvidence {
        analysis_artifacts,
        range: range.name.clone(),
        publication_ownership,
        read_functions: fact.read_functions.iter().cloned().collect(),
        write_functions: fact.write_functions.iter().cloned().collect(),
        read_sites: fact
            .read_sites
            .iter()
            .map(|site| ResearchRegisterObservationSite {
                function: site.function.clone(),
                pc: site.pc,
            })
            .collect(),
        write_sites: fact
            .write_sites
            .iter()
            .map(|site| ResearchRegisterObservationSite {
                function: site.function.clone(),
                pc: site.pc,
            })
            .collect(),
    })
}

enum ExactRegisterPredicate {
    AbsentRegisterModel,
    UnknownHardwareWriteSemantics {
        assertion_id: String,
        effective_write_semantics: String,
    },
}

struct ExactRegisterEvidence {
    subject: ResearchRegisterResolutionSubject,
    current_observation: Option<ResearchRegisterObservationEvidence>,
    current_identity: Option<String>,
    matching_scopes: Vec<String>,
    applied_assertions: Vec<open_radio_vendor_review::EffectiveAssertion>,
    model_sources: Vec<String>,
}

fn register_resolution(
    predicate: ExactRegisterPredicate,
    state: ResearchFindingQueryState,
    evidence: ExactRegisterEvidence,
    interpretation: &str,
) -> ExactFindingResolution {
    let ExactRegisterEvidence {
        subject,
        current_observation,
        current_identity,
        matching_scopes,
        applied_assertions,
        model_sources,
    } = evidence;
    let evidence = match predicate {
        ExactRegisterPredicate::AbsentRegisterModel => {
            ResearchFindingResolutionEvidence::AbsentRegisterModel {
                subject,
                current_observation,
                current_identity,
                matching_scopes,
                applied_assertions,
                model_sources,
            }
        }
        ExactRegisterPredicate::UnknownHardwareWriteSemantics {
            assertion_id,
            effective_write_semantics,
        } => ResearchFindingResolutionEvidence::UnknownHardwareWriteSemantics {
            assertion_id,
            effective_write_semantics,
            subject,
            current_observation,
            current_identity,
            matching_scopes,
            applied_assertions,
            model_sources,
        },
    };
    ExactFindingResolution {
        state,
        interpretation: interpretation.to_owned(),
        evidence: Some(evidence),
    }
}

fn normalize_write_semantics(value: &open_radio_vendor_review::AssertionValue) -> Option<String> {
    let open_radio_vendor_review::AssertionValue::String(value) = value else {
        return None;
    };
    Some(
        match value.as_str() {
            "unknown" => "unknown",
            "w1c" | "one-to-clear" => "one-to-clear",
            "w1s" | "one-to-set" => "one-to-set",
            "one-to-toggle" => "one-to-toggle",
            "zero-to-clear" => "zero-to-clear",
            "zero-to-set" => "zero-to-set",
            "zero-to-toggle" => "zero-to-toggle",
            "clear" => "clear",
            "set" => "set",
            "modify" => "modify",
            _ => return None,
        }
        .to_owned(),
    )
}

fn parse_register_finding_id(id: &str) -> Option<(u32, u8)> {
    let (address, width) = id.strip_prefix("register-")?.rsplit_once('-')?;
    let address = u32::from_str_radix(address.strip_prefix("0x")?, 16).ok()?;
    let width = width.parse::<u8>().ok()?;
    (matches!(width, 8 | 16 | 32) && format!("register-{address:#010x}-{width}") == id)
        .then_some((address, width))
}

fn apply_finding_query(
    candidates: &mut BTreeMap<String, Accumulator>,
    requested: Option<&str>,
    exact_resolution: Option<ExactFindingResolution>,
) -> Result<ResearchFindingQuery> {
    let Some(requested) = requested else {
        return Ok(ResearchFindingQuery {
            state: ResearchFindingQueryState::All,
            finding_id: None,
            completion_claim: false,
            historical_finding_claim: false,
            interpretation: "no exact finding was requested; the inventory contains every finding in the selected current scopes"
                .to_owned(),
            resolution_evidence: None,
        });
    };
    if requested.is_empty() {
        return Err(crate::Error::invalid(
            "research finding ID must not be empty",
        ));
    }
    let present = candidates.contains_key(requested);
    let (state, interpretation, resolution_evidence) = if let Some(resolution) = exact_resolution {
        if present != (resolution.state == ResearchFindingQueryState::Open) {
            return Err(crate::Error::invalid(
                "exact register resolution disagrees with the generated research inventory",
            ));
        }
        (
            resolution.state,
            resolution.interpretation,
            resolution.evidence,
        )
    } else if present {
        (
            ResearchFindingQueryState::Open,
            "the exact finding is open in the selected current analyzed inputs; this is not a completion claim".to_owned(),
            None,
        )
    } else {
        (
            ResearchFindingQueryState::NotPresent,
            "the exact finding ID is absent from the selected current analyzed inputs without typed resolution evidence; absence is not proof of correctness or completion".to_owned(),
            None,
        )
    };
    candidates.retain(|id, _| state == ResearchFindingQueryState::Open && id == requested);
    Ok(ResearchFindingQuery {
        state,
        finding_id: Some(requested.to_owned()),
        completion_claim: false,
        historical_finding_claim: false,
        interpretation,
        resolution_evidence,
    })
}

fn candidate_domain(kind: &str) -> &'static str {
    match kind {
        "register-model" | "register-write-semantics" => "register",
        "interface-layout" => "interface",
        kind if kind.starts_with("replacement-") => "replacement",
        _ => "analysis",
    }
}

fn interface_research_context(
    session: &ProjectSession,
) -> (
    Option<crate::application::capability_context::InterfaceResearchContext>,
    Option<String>,
) {
    let Some(paths) = session.project.interfaces.as_ref() else {
        return (None, None);
    };
    if paths.pack.is_none() {
        return (None, None);
    }
    match crate::application::capability_context::load(session) {
        Ok(context) => (Some(context), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

fn exact_register_or_semantic_lookup(requested: Option<&str>) -> bool {
    requested.is_some_and(|finding| {
        parse_register_finding_id(finding).is_some() || finding.starts_with("semantic-")
    })
}

fn capability_contexts(
    links: &[crate::application::capability_context::CapabilityContextLink],
) -> CapabilityContexts {
    let mut by_function = CapabilityContexts::new();
    for link in links {
        by_function
            .entry(link.function.clone())
            .or_default()
            .insert(ResearchCapabilityLink {
                rule: link.rule.clone(),
                status: link.status.label().to_owned(),
                requirement_kind: link.requirement_kind.label().to_owned(),
                requirement: link.requirement.clone(),
                function: link.function.clone(),
                evidence_site: link.evidence_site,
                relation: ResearchLinkRelation::ExistingEvidenceContext,
            });
    }
    by_function
}

fn verification_contexts(project: &crate::ProjectSpec) -> (VerificationContexts, Option<String>) {
    match crate::verification::policy::evaluate(project) {
        Ok(Some(report)) => {
            let mut by_scope = VerificationContexts::new();
            for surface in report.surfaces {
                for scope in surface.review_scopes {
                    by_scope
                        .entry(scope.clone())
                        .or_default()
                        .insert(ResearchVerificationLink {
                            surface: surface.id.clone(),
                            surface_kind: surface.kind.as_str().to_owned(),
                            review_scope: scope,
                            closed: surface.closed,
                            relation: ResearchLinkRelation::ReviewScopeContext,
                        });
                }
            }
            (by_scope, None)
        }
        Ok(None) => (BTreeMap::new(), None),
        Err(error) => (BTreeMap::new(), Some(error.to_string())),
    }
}

#[derive(Clone, Debug)]
struct PrerequisiteSeed {
    id: String,
    kind: ResearchPrerequisiteKind,
    reason: String,
    path: Option<PathBuf>,
    subject: String,
    manual_action: String,
    estimated_cost_units: u64,
}

fn finding_actionability(consumers: &[ResearchConsumer]) -> ResearchActionability {
    if consumers.is_empty() {
        return ResearchActionability::InspectionOnly;
    }
    if consumers.iter().any(|consumer| {
        matches!(
            consumer,
            ResearchConsumer::RequiredAnalysisSurface {
                state: ResearchAnalysisSurfaceState::MissingVendorArtifact
                    | ResearchAnalysisSurfaceState::StaleSymbolFamily
                    | ResearchAnalysisSurfaceState::MissingProfileDefinition,
                ..
            }
        )
    }) {
        return ResearchActionability::CoverageBlocked;
    }
    if consumers.iter().any(|consumer| {
        consumer_resolution(consumer) == ResearchConsumerResolution::NeedsDestination
    }) {
        return ResearchActionability::NeedsDestination;
    }
    if consumers
        .iter()
        .any(|consumer| consumer_resolution(consumer) == ResearchConsumerResolution::NeedsAnchor)
    {
        return ResearchActionability::NeedsAnchor;
    }
    ResearchActionability::Ready
}

fn consumer_resolution(consumer: &ResearchConsumer) -> ResearchConsumerResolution {
    match consumer {
        ResearchConsumer::ReviewedKnowledgeAssertions { resolution, .. }
        | ResearchConsumer::InterfacePackSlot { resolution, .. } => *resolution,
        ResearchConsumer::RequiredAnalysisSurface { .. } => ResearchConsumerResolution::Ready,
    }
}

fn finding_prerequisites(
    finding_id: &str,
    subject: &ResearchSubject,
    consumers: &[ResearchConsumer],
) -> Vec<PrerequisiteSeed> {
    let mut prerequisites = consumers
        .iter()
        .filter_map(|consumer| prerequisite_for_consumer(finding_id, subject, consumer))
        .collect::<Vec<_>>();
    prerequisites.sort_by(|left, right| left.id.cmp(&right.id));
    prerequisites.dedup_by(|left, right| left.id == right.id);
    prerequisites
}

fn prerequisite_for_consumer(
    finding_id: &str,
    subject: &ResearchSubject,
    consumer: &ResearchConsumer,
) -> Option<PrerequisiteSeed> {
    match consumer {
        ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::NeedsDestination,
            configured_paths,
            diagnostic,
            ..
        } => {
            let subject = "project-reviewed-knowledge-default".to_owned();
            let id = stable_id("prerequisite-destination", &subject);
            Some(PrerequisiteSeed {
                id,
                kind: ResearchPrerequisiteKind::SelectReviewedKnowledgeDestination,
                reason: diagnostic.clone().unwrap_or_else(|| {
                    "reviewed knowledge has no explicit writable destination".to_owned()
                }),
                path: None,
                subject,
                manual_action: if configured_paths.is_empty() {
                    "Configure a project-local reviewed-knowledge pack and select it as default-pack"
                        .to_owned()
                } else {
                    format!(
                        "Select one owning [reviewed-knowledge].default-pack from: {}",
                        configured_paths
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
                estimated_cost_units: 1,
            })
        }
        ResearchConsumer::InterfacePackSlot {
            resolution: ResearchConsumerResolution::NeedsDestination,
            diagnostic,
            ..
        } => {
            let subject = "project-interface-pack".to_owned();
            Some(PrerequisiteSeed {
                id: stable_id("prerequisite-interface-pack", &subject),
                kind: ResearchPrerequisiteKind::ConfigureInterfaceDestination,
                reason: diagnostic
                    .clone()
                    .unwrap_or_else(|| "project has no reviewed interface pack".to_owned()),
                path: None,
                subject,
                manual_action: "Configure a project-local reviewed interface pack".to_owned(),
                estimated_cost_units: 1,
            })
        }
        ResearchConsumer::InterfacePackSlot {
            resolution: ResearchConsumerResolution::NeedsAnchor,
            path,
            contract,
            anchor,
            diagnostic,
            ..
        } => {
            let anchor_candidate = match subject {
                ResearchSubject::InterfaceObservation { observation, .. } => observation
                    .split_once('@')
                    .map_or(observation.as_str(), |(identity, _)| identity),
                _ => finding_id,
            };
            let subject = anchor.as_ref().map_or_else(
                || anchor_candidate.to_owned(),
                |anchor| format!("{contract}::{anchor}"),
            );
            let path = path.clone();
            Some(PrerequisiteSeed {
                id: stable_id("prerequisite-interface-anchor", &subject),
                kind: ResearchPrerequisiteKind::CreateInterfaceAnchor,
                reason: diagnostic.clone().unwrap_or_else(|| {
                    "interface observation has no writable project anchor".to_owned()
                }),
                path: path.clone(),
                subject: subject.clone(),
                manual_action: format!(
                    "Create a project-local non-templated anchor for {subject} in {}",
                    path.as_ref().map_or_else(
                        || "the reviewed interface pack".to_owned(),
                        |path| path.display().to_string()
                    )
                ),
                estimated_cost_units: 3,
            })
        }
        ResearchConsumer::RequiredAnalysisSurface {
            state,
            source,
            profile,
            project_manifest,
            run_spec,
            ..
        } => {
            let (id, path, prerequisite_subject, manual_action, estimated_cost_units) = match state
            {
                ResearchAnalysisSurfaceState::MissingVendorArtifact => (
                    stable_id("prerequisite-source-artifact", source),
                    run_spec.clone(),
                    format!("source-artifact:{source}"),
                    format!(
                        "Bind source-artifact:{source} to the exact vendor input and rerun project analyze"
                    ),
                    1,
                ),
                ResearchAnalysisSurfaceState::StaleSymbolFamily => {
                    let surface = match subject {
                        ResearchSubject::PublicSymbolFamily { surface, .. } => surface,
                        _ => finding_id,
                    };
                    (
                        stable_id("prerequisite-symbol-family", surface),
                        Some(project_manifest.clone()),
                        format!("public-symbol-family:{surface}"),
                        format!(
                            "Correct the source binding or reviewed symbol prefix for public family {surface}"
                        ),
                        3,
                    )
                }
                ResearchAnalysisSurfaceState::MissingProfileDefinition => {
                    let profile = profile.as_deref().unwrap_or("<not-configured>");
                    let surface = match subject {
                        ResearchSubject::PublicSymbolFamily { surface, .. } => surface,
                        _ => finding_id,
                    };
                    (
                        stable_id("prerequisite-analysis-profile-definition", surface),
                        Some(project_manifest.clone()),
                        format!("analysis-profile:{profile}"),
                        format!(
                            "Define a linked-IR profile {profile} for required source {source}"
                        ),
                        3,
                    )
                }
                ResearchAnalysisSurfaceState::MissingSymbolInventory
                | ResearchAnalysisSurfaceState::MissingProfileOutput
                | ResearchAnalysisSurfaceState::InvalidProfileOutput => return None,
            };
            Some(PrerequisiteSeed {
                id,
                kind: ResearchPrerequisiteKind::AcquireRequiredAnalysisSurface,
                reason: match state {
                    ResearchAnalysisSurfaceState::MissingVendorArtifact => {
                        format!(
                            "required source-artifact:{source} is not bound to an existing file"
                        )
                    }
                    ResearchAnalysisSurfaceState::StaleSymbolFamily => {
                        "the reviewed public symbol family matches no source-qualified symbol"
                            .to_owned()
                    }
                    ResearchAnalysisSurfaceState::MissingProfileDefinition => {
                        "the required public symbol family has no configured linked-IR profile"
                            .to_owned()
                    }
                    ResearchAnalysisSurfaceState::MissingSymbolInventory
                    | ResearchAnalysisSurfaceState::MissingProfileOutput
                    | ResearchAnalysisSurfaceState::InvalidProfileOutput => {
                        unreachable!("automatic analysis-surface states return above")
                    }
                },
                path,
                subject: prerequisite_subject,
                manual_action,
                estimated_cost_units,
            })
        }
        _ => None,
    }
}

#[derive(Debug)]
struct PrerequisiteAccumulator {
    seed: PrerequisiteSeed,
    finding_ids: BTreeSet<String>,
    action_ids: BTreeSet<String>,
    guaranteed: BTreeSet<String>,
    optimistic: BTreeSet<String>,
    roots: BTreeSet<String>,
    scopes: BTreeSet<String>,
    publication_scopes: BTreeSet<String>,
}

fn build_prerequisites(actions: &[ResearchAction]) -> Vec<ResearchPrerequisiteAction> {
    let mut grouped = BTreeMap::<String, PrerequisiteAccumulator>::new();
    for action in actions {
        for finding in &action.findings {
            for seed in finding_prerequisites(&finding.id, &finding.subject, &finding.consumers) {
                let prerequisite =
                    grouped
                        .entry(seed.id.clone())
                        .or_insert_with(|| PrerequisiteAccumulator {
                            seed,
                            finding_ids: BTreeSet::new(),
                            action_ids: BTreeSet::new(),
                            guaranteed: BTreeSet::new(),
                            optimistic: BTreeSet::new(),
                            roots: BTreeSet::new(),
                            scopes: BTreeSet::new(),
                            publication_scopes: BTreeSet::new(),
                        });
                prerequisite.finding_ids.insert(finding.id.clone());
                prerequisite.action_ids.insert(action.id.clone());
                prerequisite
                    .guaranteed
                    .extend(finding.guaranteed_function_ids.iter().cloned());
                prerequisite
                    .optimistic
                    .extend(finding.optimistic_function_ids.iter().cloned());
                prerequisite
                    .roots
                    .extend(finding.affected_scope_roots.iter().cloned());
                prerequisite.scopes.extend(finding.scopes.iter().cloned());
                prerequisite
                    .publication_scopes
                    .extend(finding.publication_scopes.iter().cloned());
            }
        }
    }
    grouped
        .into_values()
        .map(|value| {
            let benefit_points = value.guaranteed.len() as u64 * 20
                + value.optimistic.len() as u64 * 3
                + value.roots.len() as u64 * 10
                + value.publication_scopes.len() as u64 * 20;
            ResearchPrerequisiteAction {
                rank: 0,
                id: value.seed.id,
                kind: value.seed.kind,
                reason: value.seed.reason,
                path: value.seed.path,
                subject: value.seed.subject,
                manual_action: value.seed.manual_action,
                satisfies_finding_ids: value.finding_ids.into_iter().collect(),
                blocked_action_ids: value.action_ids.into_iter().collect(),
                guaranteed_unlock: value.guaranteed.len(),
                optimistic_unlock: value.optimistic.len(),
                affected_scope_roots: value.roots.into_iter().collect(),
                scopes: value.scopes.into_iter().collect(),
                benefit_points,
                estimated_cost_units: value.seed.estimated_cost_units,
            }
        })
        .collect()
}

fn ranked_prerequisite_indices_for_focus(
    prerequisites: &[ResearchPrerequisiteAction],
    actions: &[ResearchAction],
    strategy: ResearchRankingStrategy,
    focus: ResearchFocus,
) -> Vec<usize> {
    let eligible_findings = actions
        .iter()
        .flat_map(|action| &action.findings)
        .filter(|finding| finding_matches_focus(finding, focus))
        .map(|finding| finding.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut indices = (0..prerequisites.len())
        .filter(|index| {
            let prerequisite = &prerequisites[*index];
            prerequisite_matches_focus(prerequisite, &eligible_findings)
                && (strategy != ResearchRankingStrategy::Frontier
                    || !prerequisites
                        .iter()
                        .enumerate()
                        .any(|(other_index, other)| {
                            other_index != *index
                                && prerequisite_matches_focus(other, &eligible_findings)
                                && required_surface_lane(other)
                                    == required_surface_lane(prerequisite)
                                && other.benefit_points >= prerequisite.benefit_points
                                && other.estimated_cost_units <= prerequisite.estimated_cost_units
                                && (other.benefit_points > prerequisite.benefit_points
                                    || other.estimated_cost_units
                                        < prerequisite.estimated_cost_units)
                        }))
        })
        .collect::<Vec<_>>();
    sort_prerequisite_indices(&mut indices, prerequisites, strategy);
    indices
}

fn sort_prerequisite_indices(
    indices: &mut [usize],
    prerequisites: &[ResearchPrerequisiteAction],
    strategy: ResearchRankingStrategy,
) {
    indices.sort_by(|left, right| {
        let left = &prerequisites[*left];
        let right = &prerequisites[*right];
        required_surface_lane(left)
            .cmp(&required_surface_lane(right))
            .then_with(|| match strategy {
                ResearchRankingStrategy::Impact | ResearchRankingStrategy::Frontier => right
                    .benefit_points
                    .cmp(&left.benefit_points)
                    .then_with(|| left.estimated_cost_units.cmp(&right.estimated_cost_units))
                    .then_with(|| left.id.cmp(&right.id)),
                ResearchRankingStrategy::QuickWins => left
                    .estimated_cost_units
                    .cmp(&right.estimated_cost_units)
                    .then_with(|| right.benefit_points.cmp(&left.benefit_points))
                    .then_with(|| left.id.cmp(&right.id)),
            })
    });
}

fn prerequisite_matches_focus(
    prerequisite: &ResearchPrerequisiteAction,
    eligible_findings: &BTreeSet<&str>,
) -> bool {
    prerequisite
        .satisfies_finding_ids
        .iter()
        .any(|finding| eligible_findings.contains(finding.as_str()))
}

fn required_surface_lane(prerequisite: &ResearchPrerequisiteAction) -> u8 {
    u8::from(prerequisite.kind != ResearchPrerequisiteKind::AcquireRequiredAnalysisSurface)
}

fn summarize_actionability(findings: &[ResearchFinding]) -> ResearchActionabilitySummary {
    let mut summary = ResearchActionabilitySummary::default();
    for finding in findings {
        let group = match finding.actionability {
            ResearchActionability::Ready => &mut summary.ready,
            ResearchActionability::NeedsAnchor => &mut summary.needs_anchor,
            ResearchActionability::NeedsDestination => &mut summary.needs_destination,
            ResearchActionability::CoverageBlocked => &mut summary.coverage_blocked,
            ResearchActionability::InspectionOnly => &mut summary.inspection_only,
        };
        group.finding_ids.push(finding.id.clone());
    }
    for group in [
        &mut summary.ready,
        &mut summary.needs_anchor,
        &mut summary.needs_destination,
        &mut summary.coverage_blocked,
        &mut summary.inspection_only,
    ] {
        group.finding_ids.sort();
        group.finding_ids.dedup();
        group.count = group.finding_ids.len();
    }
    summary
}

fn collect_prerequisite_ids(findings: &[ResearchFinding]) -> Vec<String> {
    let mut prerequisite_ids = findings
        .iter()
        .flat_map(|finding| finding.prerequisite_ids.iter().cloned())
        .collect::<Vec<_>>();
    prerequisite_ids.sort();
    prerequisite_ids.dedup();
    prerequisite_ids
}

fn finalize(
    candidate: Accumulator,
    capabilities_by_function: &CapabilityContexts,
    surfaces_by_scope: &VerificationContexts,
    next_action: ExecutableAction,
    revalidation_action: ExecutableAction,
    requery_action: ExecutableAction,
) -> ResearchAction {
    let capability_links = candidate
        .direct
        .iter()
        .flat_map(|function| capabilities_by_function.get(function).into_iter().flatten())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let verification_links = candidate
        .scopes
        .iter()
        .flat_map(|scope| surfaces_by_scope.get(scope).into_iter().flatten())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let kind = candidate.kind.clone();
    let knowledge_required = candidate.blocker_resolution_route.as_ref().map_or_else(
        || knowledge_required(&kind).to_owned(),
        |route| route.required_model.clone(),
    );
    let resolution_owner = candidate.blocker_resolution_route.as_ref().map_or_else(
        || subject_resolution_owner(&candidate.subject),
        |route| route.owner,
    );
    let event_route = matches!(
        &candidate.subject,
        ResearchSubject::EventRouteBlocker { .. }
    );
    let evidence_required = candidate.blocker_resolution_route.as_ref().map_or_else(
        || evidence_required(&kind),
        |route| route.evidence_required.clone(),
    );
    let actionability = finding_actionability(&candidate.consumers);
    let prerequisite_ids =
        finding_prerequisites(&candidate.id, &candidate.subject, &candidate.consumers)
            .into_iter()
            .map(|prerequisite| prerequisite.id)
            .collect();
    let finding = ResearchFinding {
        id: candidate.id,
        kind: kind.clone(),
        severity: candidate.severity,
        subject: candidate.subject,
        reviewed_memory_access: candidate.reviewed_memory_access,
        consumers: candidate.consumers,
        blocker_resolution_route: candidate.blocker_resolution_route,
        resolution_owner,
        actionability,
        prerequisite_ids,
        evidence_sites: candidate.evidence_sites.into_iter().collect(),
        evidence_channels: candidate.evidence_channels.into_iter().collect(),
        inspection_function_ids: candidate.inspection.into_iter().collect(),
        direct_function_ids: candidate.direct.into_iter().collect(),
        guaranteed_function_ids: candidate.guaranteed.into_iter().collect(),
        optimistic_function_ids: candidate.optimistic.into_iter().collect(),
        marginal_function_ids: candidate.marginal.into_iter().collect(),
        co_blocker_ids: candidate.co_blockers.into_iter().collect(),
        affected_scope_roots: candidate.roots.into_iter().collect(),
        scopes: candidate.scopes.into_iter().collect(),
        capability_links,
        verification_links,
        publication_scopes: if event_route {
            Vec::new()
        } else {
            candidate.publication_scopes.into_iter().collect()
        },
        knowledge_required,
        evidence_required,
        revalidation_actions: vec![revalidation_action],
        requery_action,
        summary: candidate.message,
    };
    let action_resolution = finding_action_resolution_key(&finding);
    let mut result = ResearchAction {
        rank: 0,
        id: stable_id(
            "action",
            &action_canonical_identity(&next_action, &action_resolution),
        ),
        kinds: vec![kind],
        score: 0,
        inspection_function_ids: finding.inspection_function_ids.clone(),
        direct_functions: finding.direct_function_ids.len(),
        direct_function_ids: finding.direct_function_ids.clone(),
        guaranteed_unlock: finding.guaranteed_function_ids.len(),
        guaranteed_function_ids: finding.guaranteed_function_ids.clone(),
        optimistic_unlock: finding.optimistic_function_ids.len(),
        optimistic_function_ids: finding.optimistic_function_ids.clone(),
        marginal_unlock_after_co_blockers: finding.marginal_function_ids.len(),
        marginal_function_ids: finding.marginal_function_ids.clone(),
        co_blockers: finding.co_blocker_ids.len(),
        co_blocker_ids: finding.co_blocker_ids.clone(),
        affected_scope_roots: finding.affected_scope_roots.clone(),
        scopes: finding.scopes.clone(),
        capability_links: finding.capability_links.clone(),
        verification_links: finding.verification_links.clone(),
        publication_scopes: finding.publication_scopes.clone(),
        estimated_cost: String::new(),
        confidence: String::new(),
        next_action,
        actionability: summarize_actionability(std::slice::from_ref(&finding)),
        prerequisite_ids: finding.prerequisite_ids.clone(),
        findings: vec![finding],
        score_breakdown: ResearchScoreBreakdown {
            guaranteed_weight: 0,
            optimistic_weight: 0,
            marginal_weight: 0,
            root_weight: 0,
            capability_weight: 0,
            verification_weight: 0,
            publication_weight: 0,
            cost_penalty: 0,
            co_blocker_penalty: 0,
        },
        score_explanation: ResearchScoreExplanation {
            benefit_points: 0,
            effort_points: 0,
            estimated_cost_units: 0,
        },
    };
    refresh_action_score(&mut result);
    result
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActionResolutionKey {
    owner: crate::BlockerResolutionOwner,
    required_model: String,
    memory_access_role: Option<crate::ReviewedMemoryAccessRole>,
}

fn finding_action_resolution_key(finding: &ResearchFinding) -> ActionResolutionKey {
    ActionResolutionKey {
        owner: finding.resolution_owner,
        required_model: finding.knowledge_required.clone(),
        memory_access_role: finding
            .reviewed_memory_access
            .map(|classification| classification.role),
    }
}

fn subject_resolution_owner(subject: &ResearchSubject) -> crate::BlockerResolutionOwner {
    match subject {
        ResearchSubject::AnalysisRoot { .. } | ResearchSubject::EventRouteBlocker { .. } => {
            crate::BlockerResolutionOwner::Unsupported
        }
        ResearchSubject::MmioRegister { .. } => crate::BlockerResolutionOwner::ReviewedKnowledge,
        ResearchSubject::InterfaceObservation { .. } => {
            crate::BlockerResolutionOwner::InterfacePack
        }
        ResearchSubject::PublicSymbolFamily { .. } => {
            crate::BlockerResolutionOwner::ProjectComposition
        }
    }
}

fn action_canonical_identity(
    action: &ExecutableAction,
    resolution: &ActionResolutionKey,
) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        action.canonical_execution_key(),
        resolution.owner.label(),
        resolution.required_model,
        resolution.memory_access_role.map_or(
            "not-reviewed-memory",
            crate::ReviewedMemoryAccessRole::label
        )
    )
}

fn coalesce_actions(candidates: Vec<ResearchAction>) -> Vec<ResearchAction> {
    let mut actions = Vec::<ResearchAction>::new();
    let mut by_action = BTreeMap::<(ExecutableAction, ActionResolutionKey), usize>::new();
    for candidate in candidates {
        let resolution = finding_action_resolution_key(
            candidate
                .findings
                .first()
                .expect("every research action owns one or more findings"),
        );
        let key = (candidate.next_action.clone(), resolution);
        if let Some(index) = by_action.get(&key).copied() {
            let action = &mut actions[index];
            action.findings.extend(candidate.findings);
            action
                .findings
                .sort_by(|left, right| left.id.cmp(&right.id));
            merge_strings(&mut action.kinds, candidate.kinds);
            merge_strings(&mut action.scopes, candidate.scopes);
            merge_ordered(&mut action.capability_links, candidate.capability_links);
            merge_ordered(&mut action.verification_links, candidate.verification_links);
            merge_strings(&mut action.publication_scopes, candidate.publication_scopes);
            merge_strings(
                &mut action.affected_scope_roots,
                candidate.affected_scope_roots,
            );
            merge_strings(
                &mut action.inspection_function_ids,
                candidate.inspection_function_ids,
            );
            merge_strings(
                &mut action.direct_function_ids,
                candidate.direct_function_ids,
            );
            merge_strings(
                &mut action.guaranteed_function_ids,
                candidate.guaranteed_function_ids,
            );
            merge_strings(
                &mut action.optimistic_function_ids,
                candidate.optimistic_function_ids,
            );
            merge_strings(
                &mut action.marginal_function_ids,
                candidate.marginal_function_ids,
            );
            merge_strings(&mut action.co_blocker_ids, candidate.co_blocker_ids);
            let internal_findings = action
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<BTreeSet<_>>();
            action
                .co_blocker_ids
                .retain(|id| !internal_findings.contains(id.as_str()));
            action.actionability = summarize_actionability(&action.findings);
            action.prerequisite_ids = collect_prerequisite_ids(&action.findings);
            refresh_action_score(action);
        } else {
            by_action.insert(key, actions.len());
            actions.push(candidate);
        }
    }
    actions.sort_by(|left, right| left.id.cmp(&right.id));
    actions
}

fn refresh_action_score(candidate: &mut ResearchAction) {
    candidate.direct_functions = candidate.direct_function_ids.len();
    candidate.guaranteed_unlock = candidate.guaranteed_function_ids.len();
    candidate.optimistic_unlock = candidate.optimistic_function_ids.len();
    candidate.marginal_unlock_after_co_blockers = candidate.marginal_function_ids.len();
    candidate.co_blockers = candidate.co_blocker_ids.len();
    let cost = candidate
        .kinds
        .iter()
        .map(|kind| cost_units(kind, candidate.direct_functions))
        .max()
        .unwrap_or_default();
    candidate.estimated_cost = match cost {
        0..=2 => "low",
        3..=5 => "medium",
        _ => "high",
    }
    .to_owned();
    candidate.confidence = confidence(&candidate.kinds, candidate.co_blockers).to_owned();
    let event_route_only = candidate
        .findings
        .iter()
        .all(|finding| matches!(finding.subject, ResearchSubject::EventRouteBlocker { .. }));
    candidate.score_breakdown = ResearchScoreBreakdown {
        guaranteed_weight: candidate.guaranteed_unlock as u64 * 20,
        optimistic_weight: candidate.optimistic_unlock as u64 * 3,
        marginal_weight: candidate.marginal_unlock_after_co_blockers as u64 * 5,
        root_weight: if event_route_only {
            0
        } else {
            candidate.affected_scope_roots.len() as u64 * 10
        },
        capability_weight: 0,
        verification_weight: 0,
        publication_weight: candidate.publication_scopes.len() as u64 * 20,
        cost_penalty: cost * 10,
        co_blocker_penalty: candidate.co_blockers as u64 * 5,
    };
    let benefit = candidate.score_breakdown.guaranteed_weight
        + candidate.score_breakdown.optimistic_weight
        + candidate.score_breakdown.marginal_weight
        + candidate.score_breakdown.root_weight
        + candidate.score_breakdown.capability_weight
        + candidate.score_breakdown.verification_weight
        + candidate.score_breakdown.publication_weight;
    let effort =
        candidate.score_breakdown.cost_penalty + candidate.score_breakdown.co_blocker_penalty + 1;
    candidate.score_explanation = ResearchScoreExplanation {
        benefit_points: benefit,
        effort_points: effort,
        estimated_cost_units: cost,
    };
    candidate.score = benefit.saturating_mul(100) / effort;
}

fn merge_strings(target: &mut Vec<String>, source: Vec<String>) {
    target.extend(source);
    target.sort();
    target.dedup();
}

fn merge_ordered<T: Ord>(target: &mut Vec<T>, source: Vec<T>) {
    target.extend(source);
    target.sort();
    target.dedup();
}

fn cost_units(kind: &str, functions: usize) -> u64 {
    let base = match kind {
        "analysis-surface" => 3,
        "unresolved-call" => 2,
        "call-result-model" | "call-shape" => 3,
        "interface-layout" | "register-model" => 4,
        "register-write-semantics" => 6,
        "decode" => 7,
        kind if kind.starts_with("replacement-") => 5,
        _ => 4,
    };
    base + u64::from(functions > 8) + u64::from(functions > 32)
}

fn confidence(kinds: &[String], co_blockers: usize) -> &'static str {
    if kinds.iter().any(|kind| kind == "register-write-semantics") {
        "low-until-hil"
    } else if co_blockers == 0 {
        "high"
    } else {
        "medium"
    }
}

fn knowledge_required(kind: &str) -> &'static str {
    match kind {
        "analysis-surface" => "authenticated vendor source and public symbol-family coverage",
        "decode" => "ISA/backend decode support",
        "unresolved-call" => "symbol/linkage identity",
        "call-result-model" => "function return/effect contract",
        "call-shape" | "indirect-control-flow" | "interface-layout" => {
            "ABI and interface-table layout"
        }
        "register-model" => {
            "owning register region and manually confirmed register identity/semantics"
        }
        "register-write-semantics" => "hardware semantics backed by HIL or authoritative docs",
        kind if kind.starts_with("replacement-") => "production binding and verification evidence",
        "memory-load" | "memory-store" | "memory-intrinsic" => "memory-object/type layout",
        _ => "reviewed semantic model",
    }
}

fn evidence_required(kind: &str) -> Vec<String> {
    let values: &[&str] = match kind {
        "analysis-surface" => &[
            "exact authenticated source-artifact binding",
            "source-qualified public symbol prefix match",
            "valid non-empty linked-IR profile",
        ],
        "decode" => &[
            "exact undecoded instruction bytes and artifact provenance",
            "architecture or toolchain evidence for the missing ISA behavior",
            "focused decoder regression fixture",
        ],
        "unresolved-call" => &[
            "source-qualified call site and relocation evidence",
            "unique symbol or linkage identity",
        ],
        "call-result-model" | "call-shape" | "indirect-control-flow" => &[
            "caller and callee linked-IR projections",
            "body-bounded ABI/effect evidence",
        ],
        "interface-layout" => &[
            "producer and consumer slot access sites",
            "offset, width, selector and calling-convention evidence",
        ],
        "register-model" => &[
            "generated MMIO read/write observations",
            "reviewed address-space, register width and owning region",
            "authoritative or cross-checked register identity evidence",
        ],
        "register-write-semantics" => &[
            "reviewed HIL trace or authoritative hardware documentation",
            "software access evidence kept separate from hardware semantics",
        ],
        kind if kind.starts_with("replacement-") => &[
            "compiled production component identity",
            "qualifying vendor comparison or reviewed policy disposition",
        ],
        "memory-load" | "memory-store" | "memory-intrinsic" => &[
            "data-object provenance and access sites",
            "reviewed object or field layout",
        ],
        _ => &[
            "source-qualified linked-IR evidence",
            "applicability-bounded reviewed semantic assertion",
        ],
    };
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn analysis_surface_next_action_tokens(
    state: ResearchAnalysisSurfaceState,
    profile: Option<&str>,
) -> Vec<String> {
    match (state, profile) {
        (ResearchAnalysisSurfaceState::MissingSymbolInventory, _) => {
            vec!["project".to_owned(), "analyze".to_owned()]
        }
        (
            ResearchAnalysisSurfaceState::MissingProfileOutput
            | ResearchAnalysisSurfaceState::InvalidProfileOutput,
            Some(profile),
        ) => vec![
            "advanced".to_owned(),
            "ir".to_owned(),
            "build".to_owned(),
            "--profile".to_owned(),
            profile.to_owned(),
        ],
        (
            ResearchAnalysisSurfaceState::MissingVendorArtifact
            | ResearchAnalysisSurfaceState::StaleSymbolFamily
            | ResearchAnalysisSurfaceState::MissingProfileDefinition,
            _,
        )
        | (
            ResearchAnalysisSurfaceState::MissingProfileOutput
            | ResearchAnalysisSurfaceState::InvalidProfileOutput,
            None,
        ) => vec!["project".to_owned(), "status".to_owned()],
    }
}

fn next_action_tokens(candidate: &Accumulator) -> Vec<String> {
    if let ResearchSubject::EventRouteBlocker { route_id, .. } = &candidate.subject {
        return vec![
            "inspect".to_owned(),
            "flow".to_owned(),
            "--event-route".to_owned(),
            route_id.clone(),
        ];
    }
    if let ResearchSubject::PublicSymbolFamily { profile, state, .. } = &candidate.subject {
        return analysis_surface_next_action_tokens(*state, profile.as_deref());
    }
    if let ResearchSubject::MmioRegister { address, .. } = &candidate.subject {
        return vec![
            "inspect".to_owned(),
            "register".to_owned(),
            format!("{address:#010x}"),
        ];
    }
    let function = candidate.inspection.first().or_else(|| {
        (!candidate
            .evidence_channels
            .iter()
            .any(|channel| channel == "reference"))
        .then(|| candidate.direct.first())
        .flatten()
    });
    if let Some(function) = function {
        let selector = function.split_once("::").map_or_else(
            || function.clone(),
            |(source, symbol)| format!("{source}:{symbol}"),
        );
        vec!["inspect".to_owned(), "function".to_owned(), selector]
    } else if let Some(scope) = candidate.scopes.first() {
        vec!["inspect".to_owned(), "scope".to_owned(), scope.clone()]
    } else {
        vec!["project".to_owned(), "status".to_owned()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_action(command: &str) -> ExecutableAction {
        ExecutableAction::new(
            command.split_whitespace().map(str::to_owned).collect(),
            std::env::current_dir().unwrap(),
            ProjectContextRequirement::Analysis,
        )
        .unwrap()
    }

    fn finalize_test(
        candidate: Accumulator,
        capabilities: &CapabilityContexts,
        surfaces: &VerificationContexts,
        inspect_command: &str,
    ) -> ResearchAction {
        let finding_id = candidate.id.clone();
        finalize(
            candidate,
            capabilities,
            surfaces,
            test_action(inspect_command),
            test_action("blobray project analyze --project project.toml"),
            test_action(&format!(
                "blobray project research next --finding {finding_id} --project project.toml"
            )),
        )
    }

    fn persisted_observation(
        resolution: crate::application::capability_context::InterfaceObservationResolution,
    ) -> crate::application::capability_context::InterfaceResearchObservation {
        crate::application::capability_context::InterfaceResearchObservation {
            id: "fixture-observation".to_owned(),
            contract: "fixture-contract".to_owned(),
            source: "fixture".to_owned(),
            offset: 4,
            width: 32,
            selector: Some("slot".to_owned()),
            functions: vec!["archive::leaf".to_owned()],
            call_sites: vec![0x1000],
            resolution,
            anchor: Some("fixture-anchor".to_owned()),
            template: None,
            diagnostic: None,
        }
    }

    fn accumulator(id: &str, kind: &str) -> Accumulator {
        Accumulator {
            id: id.to_owned(),
            kind: kind.to_owned(),
            severity: "error".to_owned(),
            message: format!("resolve {id}"),
            subject: ResearchSubject::AnalysisRoot {
                root_id: id.to_owned(),
            },
            reviewed_memory_access: None,
            consumers: Vec::new(),
            blocker_resolution_route: Some(crate::BlockerResolutionRoute {
                owner: crate::BlockerResolutionOwner::Unsupported,
                required_model: format!("resolve {kind}"),
                evidence_required: vec!["test evidence".to_owned()],
                destination: None,
                record_kind: None,
                record_action: None,
                producer_effect: crate::BlockerProducerEffect::Unsupported,
                closes_producer: false,
                completion_predicate: crate::BlockerCompletionPredicate {
                    kind: crate::BlockerCompletionKind::Unsupported,
                    producer: "authenticated-linked-ir-review-scopes".to_owned(),
                    root_id: id.to_owned(),
                },
                rationale: "test fixture blocker".to_owned(),
            }),
            evidence_sites: BTreeSet::new(),
            evidence_channels: BTreeSet::new(),
            inspection: BTreeSet::new(),
            direct: BTreeSet::new(),
            guaranteed: BTreeSet::new(),
            optimistic: BTreeSet::new(),
            marginal: BTreeSet::new(),
            co_blockers: BTreeSet::new(),
            roots: BTreeSet::new(),
            scopes: ["radio".to_owned()].into(),
            publication_scopes: BTreeSet::new(),
        }
    }

    fn reviewed_memory_access(
        id: &'static str,
        function: &'static str,
        site: u32,
        operation: crate::ReviewedMemoryAccessOperation,
        role: crate::ReviewedMemoryAccessRole,
    ) -> crate::ReviewedMemoryAccessClassification {
        crate::ReviewedMemoryAccessClassification::new(
            id,
            crate::ReviewedMemoryAccessOccurrence::new(
                "fixture-artifact",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                function,
                site,
                operation,
            ),
            role,
            "fixture-memory-object",
            "fixture reviewed evidence",
        )
    }

    fn analysis_surface_accumulator(
        state: ResearchAnalysisSurfaceState,
        profile: Option<&str>,
    ) -> Accumulator {
        let surface = "fixture-public-controller";
        let mut candidate =
            accumulator(&stable_id("analysis-surface", surface), "analysis-surface");
        candidate.subject = ResearchSubject::PublicSymbolFamily {
            surface: surface.to_owned(),
            protocols: vec!["ble".to_owned()],
            source: "fixture-controller".to_owned(),
            symbol_prefix: "fixture_controller_".to_owned(),
            profile: profile.map(str::to_owned),
            state,
        };
        candidate.blocker_resolution_route = None;
        let output = match state {
            ResearchAnalysisSurfaceState::MissingProfileDefinition => None,
            ResearchAnalysisSurfaceState::MissingProfileOutput
            | ResearchAnalysisSurfaceState::InvalidProfileOutput => {
                profile.map(|profile| PathBuf::from(format!("artifacts/{profile}")))
            }
            ResearchAnalysisSurfaceState::MissingVendorArtifact
            | ResearchAnalysisSurfaceState::MissingSymbolInventory
            | ResearchAnalysisSurfaceState::StaleSymbolFamily => None,
        };
        candidate.consumers = vec![ResearchConsumer::RequiredAnalysisSurface {
            state,
            source: "fixture-controller".to_owned(),
            profile: profile.map(str::to_owned),
            output,
            project_manifest: PathBuf::from("project.toml"),
            working_directory: std::env::current_dir().unwrap(),
            target_spec_override: None,
            run_spec_override: None,
            svd_overrides: Vec::new(),
            run_spec: Some(PathBuf::from("run.toml")),
            diagnostic: "fixture surface is incomplete".to_owned(),
        }];
        candidate
    }

    #[test]
    fn persisted_capability_links_project_without_live_evaluation() {
        use crate::application::capability_context::{
            CapabilityContextLink, CapabilityContextRequirementKind, CapabilityContextStatus,
        };

        let call = CapabilityContextLink {
            function: "archive::leaf".to_owned(),
            rule: "fixture.radio.ready".to_owned(),
            status: CapabilityContextStatus::Matched,
            requirement_kind: CapabilityContextRequirementKind::Call,
            requirement: "runtime.call".to_owned(),
            evidence_site: Some(0x1000),
        };
        let contexts = capability_contexts(&[call.clone(), call]);
        let links = &contexts["archive::leaf"];
        assert_eq!(links.len(), 1);
        let link = links.first().unwrap();
        assert_eq!(link.status, "matched");
        assert_eq!(link.requirement_kind, "call");
        assert_eq!(link.relation, ResearchLinkRelation::ExistingEvidenceContext);
    }

    #[test]
    fn persisted_observation_owns_the_consumer_resolution() {
        use crate::application::capability_context::InterfaceObservationResolution;

        let ready = interface_consumer_with_path(
            Some(PathBuf::from("interfaces.toml")),
            &persisted_observation(InterfaceObservationResolution::Ready),
        );
        assert!(matches!(
            ready,
            ResearchConsumer::InterfacePackSlot {
                resolution: ResearchConsumerResolution::Ready,
                anchor: Some(anchor),
                diagnostic: None,
                ..
            } if anchor == "fixture-anchor"
        ));

        let mut needs_anchor = persisted_observation(InterfaceObservationResolution::NeedsAnchor);
        needs_anchor.diagnostic = Some("review anchor".to_owned());
        let consumer =
            interface_consumer_with_path(Some(PathBuf::from("interfaces.toml")), &needs_anchor);
        assert!(matches!(
            consumer,
            ResearchConsumer::InterfacePackSlot {
                resolution: ResearchConsumerResolution::NeedsAnchor,
                diagnostic: Some(diagnostic),
                ..
            } if diagnostic == "review anchor"
        ));
    }

    #[test]
    fn exact_register_queries_skip_unrelated_interface_materialization() {
        assert!(exact_register_or_semantic_lookup(Some(
            "register-0x20103100-32"
        )));
        assert!(exact_register_or_semantic_lookup(Some(
            "semantic-ieee802154.event-status.write-semantics"
        )));
        assert!(!exact_register_or_semantic_lookup(Some(
            "research-0123456789abcdef"
        )));
        assert!(!exact_register_or_semantic_lookup(None));
    }

    fn ranked_candidate(id: &str, benefit: u64, effort: u64, cost: u64) -> ResearchAction {
        let mut accumulator = accumulator(id, "unresolved-call");
        accumulator.direct.insert(id.to_owned());
        let mut candidate = finalize_test(
            accumulator,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &format!("blobray inspect function {id} --project project.toml"),
        );
        candidate.score = benefit.saturating_mul(100) / effort;
        candidate.score_explanation = ResearchScoreExplanation {
            benefit_points: benefit,
            effort_points: effort,
            estimated_cost_units: cost,
        };
        candidate
    }

    fn report_from_actions(
        actions: Vec<ResearchAction>,
        strategy: ResearchRankingStrategy,
        limit: usize,
        budget: Option<u64>,
    ) -> ResearchNextReport {
        report_from_actions_with_focus(actions, strategy, ResearchFocus::All, limit, budget)
    }

    fn report_from_actions_with_focus(
        actions: Vec<ResearchAction>,
        strategy: ResearchRankingStrategy,
        focus: ResearchFocus,
        limit: usize,
        budget: Option<u64>,
    ) -> ResearchNextReport {
        let prerequisites = build_prerequisites(&actions);
        let prerequisite_indices =
            ranked_prerequisite_indices_for_focus(&prerequisites, &actions, strategy, focus);
        let action_indices = ranked_action_indices_for_focus(&actions, strategy, focus);
        let (steps, consumed_budget) = select_ranked_steps(
            &prerequisites,
            &prerequisite_indices,
            &actions,
            &action_indices,
            limit,
            budget,
        );
        let analyzed_scopes = vec!["radio".to_owned()];
        let inventory =
            build_inventory("fixture", &analyzed_scopes, actions, prerequisites).unwrap();
        let report = ResearchNextReport {
            schema_version: RESEARCH_SCHEMA,
            command: "research next".to_owned(),
            project: "fixture".to_owned(),
            focus,
            protocol: None,
            scope: None,
            analyzed_scopes,
            finding_query: ResearchFindingQuery {
                state: ResearchFindingQueryState::All,
                finding_id: None,
                completion_claim: false,
                historical_finding_claim: false,
                interpretation: "fixture all-findings query".to_owned(),
                resolution_evidence: None,
            },
            completion_claim: false,
            capability_diagnostic: None,
            verification_diagnostic: None,
            reviewed_functions: Vec::new(),
            inventory,
            selection: ResearchSelection {
                strategy,
                limit,
                budget,
                consumed_budget,
                eligible_prerequisites: prerequisite_indices.len(),
                eligible_actions: action_indices.len(),
                diagnostic: None,
                steps,
            },
        };
        validate_report(&report).unwrap();
        report
    }

    fn refresh_inventory_digest(report: &mut ResearchNextReport) {
        report.inventory.sha256 = inventory_sha256(
            &report.project,
            &report.analyzed_scopes,
            &report.inventory.findings,
            &report.inventory.actions,
            &report.inventory.prerequisites,
        )
        .unwrap();
    }

    fn refresh_first_action_identity(report: &mut ResearchNextReport) {
        let old = report.inventory.actions[0].id.clone();
        let finding_id = &report.inventory.actions[0].finding_ids[0];
        let finding = report
            .inventory
            .findings
            .iter()
            .find(|finding| &finding.id == finding_id)
            .unwrap();
        let new = stable_id(
            "action",
            &action_canonical_identity(
                &report.inventory.actions[0].next_action,
                &finding_action_resolution_key(finding),
            ),
        );
        report.inventory.actions[0].id = new.clone();
        for prerequisite in &mut report.inventory.prerequisites {
            for action in &mut prerequisite.blocked_action_ids {
                if *action == old {
                    *action = new.clone();
                }
            }
            prerequisite.blocked_action_ids.sort();
        }
        for step in &mut report.selection.steps {
            if step.kind == ResearchStepKind::Action && step.id == old {
                step.id = new.clone();
            }
        }
        refresh_inventory_digest(report);
    }

    fn graph_function(
        identity: &str,
        dependencies: &[&str],
        direct_diagnostic_roots: &[&str],
    ) -> ResearchGraphFunction {
        let (source, symbol) = identity.split_once("::").unwrap_or(("vendor", identity));
        ResearchGraphFunction {
            identity: identity.to_owned(),
            node: GraphNode {
                source: source.to_owned(),
                symbol: symbol.to_owned(),
                dependencies: dependencies
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                direct_diagnostic_roots: direct_diagnostic_roots
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect(),
                complete: direct_diagnostic_roots.is_empty(),
            },
        }
    }

    #[test]
    fn shared_profile_output_is_loaded_once_and_scattered_to_all_scopes() {
        let shared = Path::new("shared.ir");
        let profiles = [
            ResearchProfileInput {
                id: "profile-a",
                output: shared,
            },
            ResearchProfileInput {
                id: "profile-b",
                output: shared,
            },
        ];
        let profiles_a = vec!["profile-a".to_owned()];
        let profiles_b = vec!["profile-b".to_owned()];
        let scopes = [
            ResearchScopeInput {
                id: "scope-a",
                profiles: &profiles_a,
                function_identities: ["vendor::a"].into(),
            },
            ResearchScopeInput {
                id: "scope-b",
                profiles: &profiles_b,
                function_identities: ["vendor::b"].into(),
            },
        ];
        let functions = [
            graph_function("vendor::a", &[], &[]),
            graph_function("vendor::b", &[], &[]),
        ];
        let mut loads = 0;

        let (_, graphs) = load_research_graphs_with(&profiles, &scopes, |output, visit| {
            assert_eq!(output, shared);
            loads += 1;
            for function in functions.iter().cloned() {
                visit(function)?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(loads, 1);
        assert_eq!(
            graphs["scope-a"]
                .nodes
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["vendor::a"]
        );
        assert_eq!(
            graphs["scope-b"]
                .nodes
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["vendor::b"]
        );
    }

    #[test]
    fn unselected_functions_still_populate_global_direct_diagnostic_owners() {
        let output = Path::new("all.ir");
        let profiles = [ResearchProfileInput { id: "all", output }];
        let scope_profiles = vec!["all".to_owned()];
        let scopes = [ResearchScopeInput {
            id: "selected",
            profiles: &scope_profiles,
            function_identities: ["vendor::selected"].into(),
        }];

        let (owners, graphs) = load_research_graphs_with(&profiles, &scopes, |_, visit| {
            visit(graph_function("vendor::selected", &[], &[]))?;
            visit(graph_function(
                "vendor::outside-scope",
                &[],
                &["cause:direct@1000"],
            ))?;
            Ok(())
        })
        .unwrap();

        assert_eq!(
            owners["cause:direct@1000"],
            ["vendor::outside-scope".to_owned()].into()
        );
        assert!(graphs["selected"].nodes.contains_key("vendor::selected"));
        assert!(
            !graphs["selected"]
                .nodes
                .contains_key("vendor::outside-scope")
        );
    }

    #[test]
    fn conflicting_function_identity_across_outputs_fails_closed() {
        let profiles = [
            ResearchProfileInput {
                id: "first",
                output: Path::new("first.ir"),
            },
            ResearchProfileInput {
                id: "second",
                output: Path::new("second.ir"),
            },
        ];
        let scope_profiles = vec!["first".to_owned(), "second".to_owned()];
        let scopes = [ResearchScopeInput {
            id: "radio",
            profiles: &scope_profiles,
            function_identities: ["vendor::same"].into(),
        }];

        let error = load_research_graphs_with(&profiles, &scopes, |output, visit| {
            let mut function = graph_function("vendor::same", &[], &[]);
            if output == Path::new("second.ir") {
                function.node.symbol = "conflicting-symbol".to_owned();
            }
            visit(function)
        })
        .unwrap_err();

        assert!(error.to_string().contains("inconsistent projections"));
        assert!(error.to_string().contains("vendor::same"));
    }

    #[test]
    fn one_pass_graph_scatter_is_deterministic_and_preserves_edges() {
        fn build(reverse: bool) -> (DirectDiagnosticOwners, BTreeMap<String, ScopeGraph>) {
            let mut profiles = vec![
                ResearchProfileInput {
                    id: "caller",
                    output: Path::new("caller.ir"),
                },
                ResearchProfileInput {
                    id: "leaf",
                    output: Path::new("leaf.ir"),
                },
            ];
            let mut main_profiles = vec!["caller".to_owned(), "leaf".to_owned()];
            let leaf_profiles = vec!["leaf".to_owned()];
            if reverse {
                profiles.reverse();
                main_profiles.reverse();
            }
            let mut scopes = vec![
                ResearchScopeInput {
                    id: "main",
                    profiles: &main_profiles,
                    function_identities: ["vendor::caller", "vendor::leaf"].into(),
                },
                ResearchScopeInput {
                    id: "leaf-only",
                    profiles: &leaf_profiles,
                    function_identities: ["vendor::leaf"].into(),
                },
            ];
            if reverse {
                scopes.reverse();
            }
            load_research_graphs_with(&profiles, &scopes, |output, visit| {
                let mut functions = if output == Path::new("caller.ir") {
                    vec![graph_function(
                        "vendor::caller",
                        &["vendor::leaf"],
                        &["cause:caller"],
                    )]
                } else {
                    vec![graph_function("vendor::leaf", &[], &[])]
                };
                if reverse {
                    functions.reverse();
                }
                for function in functions {
                    visit(function)?;
                }
                Ok(())
            })
            .unwrap()
        }

        let forward = build(false);
        let reverse = build(true);

        assert_eq!(forward, reverse);
        assert_eq!(
            forward.1["main"].outgoing["vendor::caller"],
            ["vendor::leaf".to_owned()].into()
        );
        assert_eq!(
            forward.1["main"].incoming["vendor::leaf"],
            ["vendor::caller".to_owned()].into()
        );
        assert_eq!(
            forward.0["cause:caller"],
            ["vendor::caller".to_owned()].into()
        );
    }

    #[test]
    fn reverse_impact_reaches_callers_but_not_unrelated_functions() {
        let graph = ScopeGraph {
            nodes: ["root", "middle", "leaf", "other"]
                .into_iter()
                .map(|id| {
                    (
                        id.to_owned(),
                        GraphNode {
                            source: "vendor".to_owned(),
                            symbol: id.to_owned(),
                            dependencies: BTreeSet::new(),
                            direct_diagnostic_roots: BTreeSet::new(),
                            complete: false,
                        },
                    )
                })
                .collect(),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::from([
                ("leaf".to_owned(), ["middle".to_owned()].into()),
                ("middle".to_owned(), ["root".to_owned()].into()),
            ]),
        };
        assert_eq!(
            reverse_reachable(&graph, &["leaf".to_owned()].into()),
            ["leaf", "middle", "root"].map(str::to_owned).into()
        );
    }

    #[test]
    fn blocker_inspection_targets_the_function_with_direct_causal_evidence() {
        let graph = ScopeGraph {
            nodes: BTreeMap::from([
                (
                    "libpp::caller".to_owned(),
                    GraphNode {
                        source: "libpp".to_owned(),
                        symbol: "caller".to_owned(),
                        dependencies: ["libpp::callee".to_owned()].into(),
                        direct_diagnostic_roots: BTreeSet::new(),
                        complete: false,
                    },
                ),
                (
                    "libpp::callee".to_owned(),
                    GraphNode {
                        source: "libpp".to_owned(),
                        symbol: "callee".to_owned(),
                        dependencies: BTreeSet::new(),
                        direct_diagnostic_roots: ["cause:call@1000".to_owned()].into(),
                        complete: false,
                    },
                ),
            ]),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
        };
        let item = crate::review_scopes::ReviewQueueItem {
            id: "cause:call@1000".to_owned(),
            kind: "call-boundary".to_owned(),
            priority: 1,
            severity: "warning".to_owned(),
            occurrences: 2,
            functions: vec!["libpp::caller".to_owned(), "libpp::callee".to_owned()],
            affected_scope_roots: vec!["libpp::caller".to_owned()],
            potentially_unblocked_functions: 2,
            sites: vec![0x1000],
            channels: vec!["direct".to_owned(), "reference".to_owned()],
            message: "inspect the causal callee".to_owned(),
        };

        assert_eq!(
            blocker_inspection_targets(&graph, &DirectDiagnosticOwners::new(), &item),
            ["libpp::callee".to_owned()].into()
        );
    }

    #[test]
    fn reference_blocker_uses_all_exact_cross_profile_owners_not_a_lower_address() {
        let graph = ScopeGraph {
            nodes: BTreeMap::from([
                (
                    "libpp::caller@0x2000".to_owned(),
                    GraphNode {
                        source: "libpp".to_owned(),
                        symbol: "caller".to_owned(),
                        dependencies: BTreeSet::new(),
                        direct_diagnostic_roots: BTreeSet::new(),
                        complete: false,
                    },
                ),
                (
                    "archive::unrelated_lower@0x10001000".to_owned(),
                    GraphNode {
                        source: "archive".to_owned(),
                        symbol: "unrelated_lower".to_owned(),
                        dependencies: BTreeSet::new(),
                        direct_diagnostic_roots: BTreeSet::new(),
                        complete: false,
                    },
                ),
            ]),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
        };
        let owners = DirectDiagnosticOwners::from([(
            "reference-cause".to_owned(),
            [
                "rom::causal_callee_a@0x2f8261b0".to_owned(),
                "rom::causal_callee_b@0x2f826200".to_owned(),
            ]
            .into(),
        )]);
        let item = crate::review_scopes::ReviewQueueItem {
            id: "reference-cause".to_owned(),
            kind: "call-result-model".to_owned(),
            priority: 1,
            severity: "warning".to_owned(),
            occurrences: 1,
            functions: vec!["libpp::caller@0x2000".to_owned()],
            affected_scope_roots: vec!["libpp::caller@0x2000".to_owned()],
            potentially_unblocked_functions: 1,
            sites: vec![0x2f82_61c0],
            channels: vec!["reference".to_owned()],
            message: "callee evidence".to_owned(),
        };

        assert_eq!(
            blocker_inspection_targets(&graph, &owners, &item),
            [
                "rom::causal_callee_a@0x2f8261b0".to_owned(),
                "rom::causal_callee_b@0x2f826200".to_owned(),
            ]
            .into()
        );
    }

    #[test]
    fn reference_blocker_without_an_exact_owner_fails_closed_to_scope_inspection() {
        let graph = ScopeGraph {
            nodes: BTreeMap::from([(
                "libpp::impacted_caller".to_owned(),
                GraphNode {
                    source: "libpp".to_owned(),
                    symbol: "impacted_caller".to_owned(),
                    dependencies: BTreeSet::new(),
                    direct_diagnostic_roots: BTreeSet::new(),
                    complete: false,
                },
            )]),
            ..ScopeGraph::default()
        };
        let item = crate::review_scopes::ReviewQueueItem {
            id: "missing-cause".to_owned(),
            kind: "call-result-model".to_owned(),
            priority: 1,
            severity: "warning".to_owned(),
            occurrences: 1,
            functions: vec!["libpp::impacted_caller".to_owned()],
            affected_scope_roots: vec!["libpp::impacted_caller".to_owned()],
            potentially_unblocked_functions: 1,
            sites: vec![0x2000],
            channels: vec!["reference".to_owned()],
            message: "missing direct owner".to_owned(),
        };
        let inspection = blocker_inspection_targets(&graph, &DirectDiagnosticOwners::new(), &item);
        assert!(inspection.is_empty());

        let mut candidate = accumulator("missing-cause", "call-result-model");
        candidate.direct = ["libpp::impacted_caller".to_owned()].into();
        candidate.evidence_channels = ["reference".to_owned()].into();
        assert_eq!(
            next_action_tokens(&candidate),
            ["inspect", "scope", "radio"]
        );
    }

    #[test]
    fn score_exposes_benefit_and_cost_terms() {
        let mut candidate = accumulator("candidate", "unresolved-call");
        candidate.message = "resolve call".to_owned();
        candidate.direct = ["leaf".to_owned()].into();
        candidate.guaranteed = ["leaf".to_owned()].into();
        candidate.optimistic = ["leaf".to_owned(), "root".to_owned()].into();
        candidate.marginal = ["leaf".to_owned()].into();
        candidate.co_blockers = ["other-cause".to_owned()].into();
        candidate.roots = ["root".to_owned()].into();
        candidate.publication_scopes = ["radio".to_owned()].into();
        let capability = ResearchCapabilityLink {
            rule: "wifi.log".to_owned(),
            status: "matched".to_owned(),
            requirement_kind: "call".to_owned(),
            requirement: "logger".to_owned(),
            function: "leaf".to_owned(),
            evidence_site: Some(0x1000),
            relation: ResearchLinkRelation::ExistingEvidenceContext,
        };
        let surface = ResearchVerificationLink {
            surface: "wifi-review".to_owned(),
            surface_kind: "review-scope".to_owned(),
            review_scope: "radio".to_owned(),
            closed: true,
            relation: ResearchLinkRelation::ReviewScopeContext,
        };
        let result = finalize_test(
            candidate,
            &BTreeMap::from([("leaf".to_owned(), [capability].into())]),
            &BTreeMap::from([("radio".to_owned(), [surface].into())]),
            "blobray inspect function leaf --project project.toml",
        );
        assert_eq!(result.guaranteed_unlock, 1);
        assert_eq!(result.optimistic_unlock, 2);
        assert_eq!(result.direct_function_ids, ["leaf"]);
        assert_eq!(result.co_blocker_ids, ["other-cause"]);
        assert_eq!(result.score_breakdown.cost_penalty, 20);
        assert_eq!(result.capability_links.len(), 1);
        assert_eq!(result.verification_links.len(), 1);
        assert_eq!(result.score_breakdown.capability_weight, 0);
        assert_eq!(result.score_breakdown.verification_weight, 0);
        assert_eq!(result.score_explanation.benefit_points, 61);
        assert_eq!(result.score_explanation.effort_points, 26);
        assert_eq!(result.score_explanation.estimated_cost_units, 2);
        assert_eq!(result.score, 234);
        assert!(result.score > 100);
        assert_eq!(
            result.next_action.render_posix(),
            "blobray inspect function leaf --project project.toml"
        );
    }

    #[test]
    fn ranking_strategies_are_deterministic_and_frontier_is_nondominated() {
        let candidates = vec![
            ranked_candidate("high-ratio", 100, 21, 4),
            ranked_candidate("quick", 30, 11, 1),
            ranked_candidate("high-benefit", 200, 61, 6),
            ranked_candidate("dominated", 20, 31, 3),
        ];

        let impact = ranked_action_indices(&candidates, ResearchRankingStrategy::Impact);
        assert_eq!(
            impact
                .iter()
                .map(|index| candidates[*index].findings[0].id.as_str())
                .collect::<Vec<_>>(),
            ["high-ratio", "high-benefit", "quick", "dominated"]
        );

        let quick_wins = ranked_action_indices(&candidates, ResearchRankingStrategy::QuickWins);
        assert_eq!(
            quick_wins
                .iter()
                .map(|index| candidates[*index].findings[0].id.as_str())
                .collect::<Vec<_>>(),
            ["quick", "dominated", "high-ratio", "high-benefit"]
        );

        let frontier = ranked_action_indices(&candidates, ResearchRankingStrategy::Frontier);
        assert_eq!(
            frontier
                .iter()
                .map(|index| candidates[*index].findings[0].id.as_str())
                .collect::<Vec<_>>(),
            ["high-ratio", "high-benefit", "quick"]
        );
    }

    #[test]
    fn bounded_selection_retains_the_complete_inventory() {
        let report = report_from_actions(
            vec![
                ranked_candidate("first", 100, 21, 4),
                ranked_candidate("second", 60, 21, 2),
                ranked_candidate("third", 30, 21, 1),
            ],
            ResearchRankingStrategy::Impact,
            1,
            None,
        );

        assert_eq!(report.schema_version, 18);
        assert_eq!(report.selection.steps.len(), 1);
        assert_eq!(report.inventory.actions.len(), 3);
        assert_eq!(report.inventory.findings.len(), 3);
        assert_eq!(
            report
                .inventory
                .actions
                .iter()
                .flat_map(|action| &action.finding_ids)
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
    }

    #[test]
    fn hardware_access_focus_keeps_inventory_and_selects_only_direct_hardware_work() {
        let mut interface = accumulator("interface-slot", "interface-layout");
        interface.subject = ResearchSubject::InterfaceObservation {
            observation: "controller-table@+0x4/32".to_owned(),
            contract: "controller-table".to_owned(),
            source: "controller".to_owned(),
            offset: 4,
            width: 32,
            selector: Some("slot".to_owned()),
            call_sites: vec![0x1000],
        };
        interface.blocker_resolution_route = None;
        interface.consumers = vec![ResearchConsumer::InterfacePackSlot {
            resolution: ResearchConsumerResolution::NeedsAnchor,
            path: Some(PathBuf::from("interfaces.toml")),
            contract: "controller-table".to_owned(),
            anchor: None,
            template: None,
            offset: 4,
            width: 32,
            diagnostic: Some("create interface anchor".to_owned()),
        }];
        let mut interface = finalize_test(
            interface,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect function interface-slot --project project.toml",
        );
        interface.score = 10_000;

        let mut generic = ranked_candidate("unresolved-call", 1_000, 1, 1);
        generic.score = 9_000;

        let mut memory = accumulator("sram-load", "memory-load");
        memory.direct.insert("controller::packet".to_owned());
        memory.reviewed_memory_access = Some(reviewed_memory_access(
            "hardware-load",
            "controller::packet",
            0x2000,
            crate::ReviewedMemoryAccessOperation::Load,
            crate::ReviewedMemoryAccessRole::HardwareShared,
        ));
        let mut memory = finalize_test(
            memory,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect function controller::packet --project project.toml",
        );
        memory.score = 1;
        let memory_action_id = memory.id.clone();

        let mut software_memory = accumulator("allocator-load", "memory-load");
        software_memory.reviewed_memory_access = Some(reviewed_memory_access(
            "software-load",
            "controller::allocator",
            0x3000,
            crate::ReviewedMemoryAccessOperation::Load,
            crate::ReviewedMemoryAccessRole::SoftwareOnly,
        ));
        let mut software_memory = finalize_test(
            software_memory,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect function controller::allocator --project project.toml",
        );
        software_memory.score = 20_000;

        let mut unclassified_memory = accumulator("unknown-load", "memory-load");
        unclassified_memory
            .direct
            .insert("controller::unknown".to_owned());
        let mut unclassified_memory = finalize_test(
            unclassified_memory,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect function controller::unknown --project project.toml",
        );
        unclassified_memory.score = 30_000;

        let actions = vec![
            interface,
            generic,
            memory,
            software_memory,
            unclassified_memory,
        ];

        let all = report_from_actions_with_focus(
            actions.clone(),
            ResearchRankingStrategy::Impact,
            ResearchFocus::All,
            10,
            None,
        );
        let hardware = report_from_actions_with_focus(
            actions,
            ResearchRankingStrategy::Impact,
            ResearchFocus::HardwareAccess,
            10,
            None,
        );

        assert_eq!(hardware.focus, ResearchFocus::HardwareAccess);
        assert_eq!(hardware.inventory, all.inventory);
        assert_eq!(hardware.inventory.findings.len(), 5);
        assert_eq!(hardware.inventory.actions.len(), 5);
        assert_eq!(hardware.inventory.prerequisites.len(), 1);
        assert_eq!(hardware.selection.eligible_prerequisites, 0);
        assert_eq!(hardware.selection.eligible_actions, 1);
        assert_eq!(
            hardware.selection.steps,
            [ResearchStepRef {
                kind: ResearchStepKind::Action,
                id: memory_action_id,
            }]
        );
    }

    #[test]
    fn reviewed_memory_ranking_requires_the_exact_active_artifact() {
        let item = crate::review_scopes::ReviewQueueItem {
            id: "memory-finding".to_owned(),
            kind: "memory-store".to_owned(),
            priority: 50,
            severity: "warning".to_owned(),
            occurrences: 1,
            functions: vec!["controller::packet".to_owned()],
            affected_scope_roots: Vec::new(),
            potentially_unblocked_functions: 1,
            sites: vec![0x2000],
            channels: vec!["reference".to_owned()],
            message: "unmodeled memory store".to_owned(),
        };
        let fact = reviewed_memory_access(
            "hardware-store",
            "controller::packet",
            0x2000,
            crate::ReviewedMemoryAccessOperation::Store,
            crate::ReviewedMemoryAccessRole::HardwareShared,
        );
        let exact = open_radio_vendor_contracts::ApplicabilityContext {
            artifacts: vec![
                open_radio_vendor_contracts::ArtifactIdentity::new(
                    fact.occurrence.artifact_source,
                    fact.occurrence.artifact_sha256,
                )
                .unwrap(),
            ],
            ..Default::default()
        };
        assert_eq!(
            classify_reviewed_memory_access(&item, &[fact], &exact),
            Some(fact)
        );

        let changed = open_radio_vendor_contracts::ApplicabilityContext {
            artifacts: vec![
                open_radio_vendor_contracts::ArtifactIdentity::new(
                    fact.occurrence.artifact_source,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                )
                .unwrap(),
            ],
            ..Default::default()
        };
        assert_eq!(
            classify_reviewed_memory_access(&item, &[fact], &changed),
            None
        );
    }

    #[test]
    fn hardware_and_software_memory_findings_do_not_share_one_ranked_action() {
        let mut hardware = accumulator("hardware-store", "memory-store");
        hardware.reviewed_memory_access = Some(reviewed_memory_access(
            "hardware-store",
            "controller::packet",
            0x2000,
            crate::ReviewedMemoryAccessOperation::Store,
            crate::ReviewedMemoryAccessRole::HardwareShared,
        ));
        let hardware = finalize_test(
            hardware,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect scope radio --project project.toml",
        );
        let hardware_action_id = hardware.id.clone();

        let mut software = accumulator("software-store", "memory-store");
        software.reviewed_memory_access = Some(reviewed_memory_access(
            "software-store",
            "controller::allocator",
            0x3000,
            crate::ReviewedMemoryAccessOperation::Store,
            crate::ReviewedMemoryAccessRole::SoftwareOnly,
        ));
        let software = finalize_test(
            software,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect scope radio --project project.toml",
        );

        let actions = coalesce_actions(vec![hardware, software]);
        assert_eq!(actions.len(), 2);
        let report = report_from_actions_with_focus(
            actions,
            ResearchRankingStrategy::Impact,
            ResearchFocus::HardwareAccess,
            10,
            None,
        );
        assert_eq!(report.inventory.findings.len(), 2);
        assert_eq!(report.selection.eligible_actions, 1);
        assert_eq!(
            report.selection.steps,
            [ResearchStepRef {
                kind: ResearchStepKind::Action,
                id: hardware_action_id,
            }]
        );
    }

    #[test]
    fn exact_finding_query_reports_open_or_not_present_without_completion() {
        let candidates = || {
            BTreeMap::from([
                (
                    "register-0x20103100-32".to_owned(),
                    accumulator("register-0x20103100-32", "register-model"),
                ),
                ("other".to_owned(), accumulator("other", "unresolved-call")),
            ])
        };

        let mut all = candidates();
        let all_query = apply_finding_query(&mut all, None, None).unwrap();
        assert_eq!(all_query.state, ResearchFindingQueryState::All);
        assert_eq!(all.len(), 2);
        assert!(!all_query.completion_claim);

        let mut open = candidates();
        let open_query =
            apply_finding_query(&mut open, Some("register-0x20103100-32"), None).unwrap();
        assert_eq!(open_query.state, ResearchFindingQueryState::Open);
        assert_eq!(
            open.keys().map(String::as_str).collect::<Vec<_>>(),
            ["register-0x20103100-32"]
        );
        assert!(!open_query.completion_claim);

        let mut missing = candidates();
        let missing_query =
            apply_finding_query(&mut missing, Some("register-missing"), None).unwrap();
        assert_eq!(missing_query.state, ResearchFindingQueryState::NotPresent);
        assert!(missing.is_empty());
        assert!(!missing_query.completion_claim);
        assert!(missing_query.interpretation.contains("not proof"));
    }

    #[test]
    fn inventory_digest_is_selection_independent_but_selection_is_not() {
        let actions = vec![
            ranked_candidate("impact", 100, 21, 4),
            ranked_candidate("quick", 30, 11, 1),
            ranked_candidate("other", 20, 31, 3),
        ];
        let impact = report_from_actions(actions.clone(), ResearchRankingStrategy::Impact, 1, None);
        let quick = report_from_actions(actions, ResearchRankingStrategy::QuickWins, 2, Some(1));

        assert_eq!(impact.inventory.sha256, quick.inventory.sha256);
        assert_ne!(impact.selection.steps, quick.selection.steps);
        let selected_finding = |report: &ResearchNextReport| {
            report
                .inventory
                .actions
                .iter()
                .find(|action| action.id == report.selection.steps[0].id)
                .unwrap()
                .finding_ids[0]
                .clone()
        };
        assert_eq!(selected_finding(&impact), "impact");
        assert_eq!(selected_finding(&quick), "quick");
    }

    #[test]
    fn hidden_finding_change_updates_digest_and_fails_generated_check() {
        let actions = vec![
            ranked_candidate("selected", 100, 21, 4),
            ranked_candidate("hidden", 10, 21, 1),
        ];
        let original =
            report_from_actions(actions.clone(), ResearchRankingStrategy::Impact, 1, None);
        let mut changed_actions = actions;
        changed_actions[1].findings[0]
            .summary
            .push_str(" with newly discovered evidence");
        let changed =
            report_from_actions(changed_actions, ResearchRankingStrategy::Impact, 1, None);

        assert_eq!(original.selection.steps, changed.selection.steps);
        assert_ne!(original.inventory.sha256, changed.inventory.sha256);
        let path = std::env::temp_dir().join(format!(
            "blobray-research-schema14-check-{}.json",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        crate::application::generated_file::write_or_check_json(
            &path,
            &original,
            false,
            "research fixture",
            true,
        )
        .unwrap();
        let error = crate::application::generated_file::write_or_check_json(
            &path,
            &changed,
            true,
            "research fixture",
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("differs"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn report_validation_rejects_dangling_catalog_and_selection_references() {
        let report = report_from_actions(
            vec![ranked_candidate("selected", 100, 21, 4)],
            ResearchRankingStrategy::Impact,
            1,
            None,
        );
        let mut dangling_finding = report.clone();
        dangling_finding.inventory.actions[0]
            .finding_ids
            .push("missing-finding".to_owned());
        dangling_finding.inventory.actions[0].finding_ids.sort();
        assert!(
            validate_report(&dangling_finding)
                .unwrap_err()
                .to_string()
                .contains("missing finding")
        );

        let mut dangling_step = report;
        dangling_step.selection.steps[0].id = "missing-action".to_owned();
        assert!(
            validate_report(&dangling_step)
                .unwrap_err()
                .to_string()
                .contains("selection references missing")
        );
    }

    #[test]
    fn report_validation_rejects_nonreciprocal_prerequisite_links() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingVendorArtifact,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project status --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 2, None);

        let mut missing = report.clone();
        missing.inventory.findings[0].prerequisite_ids[0] = "missing-prerequisite".to_owned();
        assert!(
            validate_report(&missing)
                .unwrap_err()
                .to_string()
                .contains("prerequisite set does not match its typed consumers")
        );

        let mut nonreciprocal = report;
        nonreciprocal.inventory.prerequisites[0]
            .satisfies_finding_ids
            .clear();
        assert!(
            validate_report(&nonreciprocal)
                .unwrap_err()
                .to_string()
                .contains("are not reciprocal")
        );
    }

    #[test]
    fn report_validation_rejects_inconsistent_analysis_surface_types() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingProfileOutput,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray advanced ir build --profile fixture-ir --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 2, None);

        let mut mismatched_state = report.clone();
        let ResearchConsumer::RequiredAnalysisSurface { state, .. } =
            &mut mismatched_state.inventory.findings[0].consumers[0]
        else {
            panic!("fixture must contain a required analysis surface consumer");
        };
        *state = ResearchAnalysisSurfaceState::InvalidProfileOutput;
        assert!(
            validate_report(&mismatched_state)
                .unwrap_err()
                .to_string()
                .contains("inconsistent subject and consumer state")
        );

        let manual_action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingVendorArtifact,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project status --project project.toml",
        );
        let mut wrong_prerequisite = report_from_actions(
            vec![manual_action],
            ResearchRankingStrategy::Impact,
            2,
            None,
        );
        wrong_prerequisite.inventory.prerequisites[0].kind =
            ResearchPrerequisiteKind::CreateInterfaceAnchor;
        assert!(
            validate_report(&wrong_prerequisite)
                .unwrap_err()
                .to_string()
                .contains("does not match the typed consumer")
        );

        let mut extra_consumer = report;
        extra_consumer.inventory.findings[0].consumers.push(
            ResearchConsumer::RequiredAnalysisSurface {
                state: ResearchAnalysisSurfaceState::MissingProfileOutput,
                source: "fixture-controller".to_owned(),
                profile: Some("fixture-ir".to_owned()),
                output: Some(PathBuf::from("artifacts/fixture-ir")),
                project_manifest: PathBuf::from("project.toml"),
                working_directory: std::env::current_dir().unwrap(),
                target_spec_override: None,
                run_spec_override: None,
                svd_overrides: Vec::new(),
                run_spec: Some(PathBuf::from("run.toml")),
                diagnostic: "duplicate".to_owned(),
            },
        );
        assert!(
            validate_report(&extra_consumer)
                .unwrap_err()
                .to_string()
                .contains("inconsistent kind, actionability, or consumer set")
        );
    }

    #[test]
    fn report_validation_rejects_resigned_surface_graph_bypasses() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingVendorArtifact,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project status --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 2, None);

        let mut unblocked = report.clone();
        unblocked.inventory.prerequisites[0]
            .blocked_action_ids
            .clear();
        refresh_inventory_digest(&mut unblocked);
        assert!(
            validate_report(&unblocked)
                .unwrap_err()
                .to_string()
                .contains("blocked action set does not match")
        );

        let mut orphan = report.clone();
        let mut orphan_entry = orphan.inventory.prerequisites[0].clone();
        orphan_entry.id = "zz-orphan-prerequisite".to_owned();
        orphan_entry.satisfies_finding_ids.clear();
        orphan_entry.blocked_action_ids.clear();
        orphan.inventory.prerequisites.push(orphan_entry);
        refresh_inventory_digest(&mut orphan);
        assert!(
            validate_report(&orphan)
                .unwrap_err()
                .to_string()
                .contains("is not owned by any finding")
        );

        let mut forged_seed = report.clone();
        forged_seed.inventory.prerequisites[0].subject = "source-artifact:other".to_owned();
        refresh_inventory_digest(&mut forged_seed);
        assert!(
            validate_report(&forged_seed)
                .unwrap_err()
                .to_string()
                .contains("does not match the typed consumer")
        );

        let mut empty_subject = report;
        let ResearchSubject::PublicSymbolFamily { surface, .. } =
            &mut empty_subject.inventory.findings[0].subject
        else {
            panic!("fixture must contain a public symbol family subject");
        };
        surface.clear();
        refresh_inventory_digest(&mut empty_subject);
        assert!(
            validate_report(&empty_subject)
                .unwrap_err()
                .to_string()
                .contains("invalid typed subject or consumer payload")
        );
    }

    #[test]
    fn report_validation_rejects_a_resigned_surface_action_for_the_wrong_state() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingSymbolInventory,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project analyze --project project.toml",
        );
        let mut report =
            report_from_actions(vec![action], ResearchRankingStrategy::Impact, 1, None);
        report.inventory.actions[0].next_action.argv[2] = "status".to_owned();
        refresh_first_action_identity(&mut report);
        assert!(
            validate_report(&report)
                .unwrap_err()
                .to_string()
                .contains("next action that does not match its state")
        );
    }

    #[test]
    fn report_validation_rejects_resigned_surface_action_flags_and_foreign_project() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingSymbolInventory,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project analyze --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 1, None);

        let mut check_only = report.clone();
        check_only.inventory.actions[0]
            .next_action
            .argv
            .push("--check".to_owned());
        refresh_first_action_identity(&mut check_only);
        assert!(
            validate_report(&check_only)
                .unwrap_err()
                .to_string()
                .contains("next action that does not match its state")
        );

        let mut foreign_project = report.clone();
        let project_value = foreign_project.inventory.actions[0]
            .next_action
            .argv
            .iter()
            .position(|argument| argument == "--project")
            .unwrap()
            + 1;
        foreign_project.inventory.actions[0].next_action.argv[project_value] =
            "other.toml".to_owned();
        refresh_first_action_identity(&mut foreign_project);
        assert!(
            validate_report(&foreign_project)
                .unwrap_err()
                .to_string()
                .contains("next action that does not match its state")
        );

        let mut foreign_directory = report;
        foreign_directory.inventory.actions[0]
            .next_action
            .working_directory = PathBuf::from("/tmp/blobray-foreign-working-directory");
        refresh_first_action_identity(&mut foreign_directory);
        assert!(
            validate_report(&foreign_directory)
                .unwrap_err()
                .to_string()
                .contains("next action that does not match its state")
        );
    }

    #[test]
    fn report_validation_rejects_resigned_surface_follow_up_actions() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingSymbolInventory,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project analyze --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 1, None);

        let mut forged_requery = report.clone();
        forged_requery.inventory.findings[0].requery_action =
            test_action("blobray project status --project project.toml");
        refresh_inventory_digest(&mut forged_requery);
        assert!(
            validate_report(&forged_requery)
                .unwrap_err()
                .to_string()
                .contains("requery action")
        );

        let mut absent_revalidation = report.clone();
        absent_revalidation.inventory.findings[0]
            .revalidation_actions
            .clear();
        refresh_inventory_digest(&mut absent_revalidation);
        assert!(
            validate_report(&absent_revalidation)
                .unwrap_err()
                .to_string()
                .contains("exactly one revalidation action")
        );

        let mut foreign_directory = report;
        foreign_directory.inventory.findings[0]
            .requery_action
            .working_directory = PathBuf::from("/tmp/blobray-foreign-working-directory");
        foreign_directory.inventory.findings[0].revalidation_actions[0].working_directory =
            PathBuf::from("/tmp/blobray-foreign-working-directory");
        refresh_inventory_digest(&mut foreign_directory);
        assert!(
            validate_report(&foreign_directory)
                .unwrap_err()
                .to_string()
                .contains("next action that does not match its state")
        );
    }

    #[test]
    fn report_validation_rejects_context_options_used_as_path_values() {
        let mut report = report_from_actions(
            vec![ranked_candidate("generic-finding", 10, 10, 1)],
            ResearchRankingStrategy::Impact,
            1,
            None,
        );
        let finding = &mut report.inventory.findings[0];
        for action in std::iter::once(&mut finding.requery_action)
            .chain(finding.revalidation_actions.iter_mut())
        {
            let project_value = action
                .argv
                .iter()
                .position(|argument| argument == "--project")
                .unwrap()
                + 1;
            action.argv[project_value] = "--evil".to_owned();
        }
        refresh_inventory_digest(&mut report);
        assert!(
            validate_report(&report)
                .unwrap_err()
                .to_string()
                .contains("follow-up actions do not share one exact analysis context")
        );
    }

    #[test]
    fn report_validation_rejects_resigned_identity_and_orphan_action_bypasses() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingSymbolInventory,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project analyze --project project.toml",
        );
        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 1, None);

        let mut stale_action_id = report.clone();
        stale_action_id.inventory.actions[0]
            .next_action
            .argv
            .push("--check".to_owned());
        refresh_inventory_digest(&mut stale_action_id);
        assert!(
            validate_report(&stale_action_id)
                .unwrap_err()
                .to_string()
                .contains("canonical executable identity")
        );

        let mut stale_finding_id = report.clone();
        let ResearchSubject::PublicSymbolFamily { surface, .. } =
            &mut stale_finding_id.inventory.findings[0].subject
        else {
            panic!("fixture must contain a public symbol family subject");
        };
        *surface = "forged-public-controller".to_owned();
        refresh_inventory_digest(&mut stale_finding_id);
        assert!(
            validate_report(&stale_finding_id)
                .unwrap_err()
                .to_string()
                .contains("canonical surface identity")
        );

        let mut orphan_action = report;
        let next_action = test_action("blobray project status --project project.toml");
        orphan_action
            .inventory
            .actions
            .push(ResearchActionCatalogEntry {
                id: stable_id("action", &next_action.canonical_execution_key()),
                kinds: vec!["analysis-surface".to_owned()],
                score: 0,
                next_action,
                estimated_cost: "low".to_owned(),
                confidence: "high".to_owned(),
                resolution_owner: crate::BlockerResolutionOwner::ProjectComposition,
                required_model: knowledge_required("analysis-surface").to_owned(),
                score_breakdown: ResearchScoreBreakdown {
                    guaranteed_weight: 0,
                    optimistic_weight: 0,
                    marginal_weight: 0,
                    root_weight: 0,
                    capability_weight: 0,
                    verification_weight: 0,
                    publication_weight: 0,
                    cost_penalty: 0,
                    co_blocker_penalty: 0,
                },
                score_explanation: ResearchScoreExplanation {
                    benefit_points: 0,
                    effort_points: 1,
                    estimated_cost_units: 1,
                },
                finding_ids: Vec::new(),
            });
        orphan_action
            .inventory
            .actions
            .sort_by(|left, right| left.id.cmp(&right.id));
        refresh_inventory_digest(&mut orphan_action);
        assert!(
            validate_report(&orphan_action)
                .unwrap_err()
                .to_string()
                .contains("is not owned by any finding")
        );
    }

    #[test]
    fn report_validation_rejects_a_protocol_header_outside_the_surface() {
        let action = finalize_test(
            analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingVendorArtifact,
                Some("fixture-ir"),
            ),
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project status --project project.toml",
        );
        let mut report =
            report_from_actions(vec![action], ResearchRankingStrategy::Impact, 2, None);
        report.protocol = Some("bluetooth".to_owned());
        assert!(
            validate_report(&report)
                .unwrap_err()
                .to_string()
                .contains("invalid typed subject or consumer payload")
        );
        report.protocol = Some("Bluetooth".to_owned());
        assert!(
            validate_report(&report)
                .unwrap_err()
                .to_string()
                .contains("canonical supported protocol")
        );
    }

    #[test]
    fn exact_resolution_validation_rejects_a_filtered_scope_intersection() {
        let mut report = report_from_actions(
            vec![ranked_candidate("selected", 100, 21, 4)],
            ResearchRankingStrategy::Impact,
            1,
            None,
        );
        let observation = ResearchRegisterObservationEvidence {
            analysis_artifacts: Vec::new(),
            range: "radio".to_owned(),
            publication_ownership: ResearchRegisterPublicationOwnership::Owned,
            read_functions: vec!["vendor::read".to_owned()],
            write_functions: Vec::new(),
            read_sites: vec![ResearchRegisterObservationSite {
                function: "vendor::read".to_owned(),
                pc: 0x1000,
            }],
            write_sites: Vec::new(),
        };
        report.finding_query = ResearchFindingQuery {
            state: ResearchFindingQueryState::FilteredOut,
            finding_id: Some("register-0x10000000-32".to_owned()),
            completion_claim: false,
            historical_finding_claim: false,
            interpretation: "fixture filtered query".to_owned(),
            resolution_evidence: Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
                subject: ResearchRegisterResolutionSubject {
                    chip: "fixture-chip".to_owned(),
                    address_space: "cpu".to_owned(),
                    address: 0x1000_0000,
                    width: 32,
                },
                current_observation: Some(observation),
                current_identity: None,
                matching_scopes: vec!["other-radio".to_owned()],
                applied_assertions: Vec::new(),
                model_sources: Vec::new(),
            }),
        };
        validate_finding_resolution(&report).unwrap();

        if let Some(ResearchFindingResolutionEvidence::AbsentRegisterModel { subject, .. }) =
            report.finding_query.resolution_evidence.as_mut()
        {
            subject.address += 4;
        }
        assert!(
            validate_finding_resolution(&report)
                .unwrap_err()
                .to_string()
                .contains("does not match the exact finding ID")
        );
        let Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
            subject,
            matching_scopes,
            ..
        }) = report.finding_query.resolution_evidence.as_mut()
        else {
            panic!("fixture must contain register resolution evidence");
        };
        subject.address -= 4;
        *matching_scopes = report.analyzed_scopes.clone();
        assert!(
            validate_finding_resolution(&report)
                .unwrap_err()
                .to_string()
                .contains("does not satisfy state FilteredOut")
        );
    }

    #[test]
    fn satisfied_register_resolution_requires_one_exact_identity_assertion() {
        let pack = open_radio_vendor_review::ReviewPack::from_toml(
            r#"
schema = 2
id = "fixture"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[[assertions]]
id = "radio.status.identity"
subject = "register:fixture-chip/cpu/0x10000000/32"
kind = "register-identity"
value = "RADIO.STATUS"
[[assertions.evidence]]
source = "manual"
locator = "RADIO.STATUS"
"#,
        )
        .unwrap();
        let knowledge = open_radio_vendor_review::ReviewKnowledge::merge([pack]).unwrap();
        let assertion = knowledge.assertions()["radio.status.identity"].clone();
        let observation = ResearchRegisterObservationEvidence {
            analysis_artifacts: Vec::new(),
            range: "radio".to_owned(),
            publication_ownership: ResearchRegisterPublicationOwnership::Owned,
            read_functions: vec!["vendor::read".to_owned()],
            write_functions: Vec::new(),
            read_sites: vec![ResearchRegisterObservationSite {
                function: "vendor::read".to_owned(),
                pc: 0x1000,
            }],
            write_sites: Vec::new(),
        };
        let mut report = report_from_actions(
            vec![ranked_candidate("selected", 100, 21, 4)],
            ResearchRankingStrategy::Impact,
            1,
            None,
        );
        report.finding_query = ResearchFindingQuery {
            state: ResearchFindingQueryState::ConditionSatisfied,
            finding_id: Some("register-0x10000000-32".to_owned()),
            completion_claim: false,
            historical_finding_claim: false,
            interpretation: "fixture satisfied query".to_owned(),
            resolution_evidence: Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
                subject: ResearchRegisterResolutionSubject {
                    chip: "fixture-chip".to_owned(),
                    address_space: "cpu".to_owned(),
                    address: 0x1000_0000,
                    width: 32,
                },
                current_observation: Some(observation),
                current_identity: Some("RADIO.STATUS".to_owned()),
                matching_scopes: vec!["radio".to_owned()],
                applied_assertions: vec![assertion],
                model_sources: Vec::new(),
            }),
        };
        validate_finding_resolution(&report).unwrap();

        let Some(ResearchFindingResolutionEvidence::AbsentRegisterModel {
            applied_assertions, ..
        }) = report.finding_query.resolution_evidence.as_mut()
        else {
            panic!("fixture must contain register identity evidence");
        };
        applied_assertions[0].value =
            open_radio_vendor_review::AssertionValue::String("RADIO.OTHER".to_owned());
        assert!(
            validate_finding_resolution(&report)
                .unwrap_err()
                .to_string()
                .contains("does not satisfy state ConditionSatisfied")
        );
    }

    #[test]
    fn semantic_resolution_requires_owned_identity_for_a_positive_current_claim() {
        assert_eq!(
            semantic_resolution_state(
                "unknown",
                ResearchRegisterPublicationOwnership::Owned,
                Some("RADIO.STATUS")
            ),
            ResearchFindingQueryState::Open
        );
        assert_eq!(
            semantic_resolution_state(
                "one-to-clear",
                ResearchRegisterPublicationOwnership::Owned,
                Some("RADIO.STATUS")
            ),
            ResearchFindingQueryState::ConditionSatisfied
        );
        assert_eq!(
            semantic_resolution_state(
                "one-to-clear",
                ResearchRegisterPublicationOwnership::External,
                Some("SYSTEM.STATUS")
            ),
            ResearchFindingQueryState::NotPresent
        );
        assert_eq!(
            semantic_resolution_state(
                "one-to-clear",
                ResearchRegisterPublicationOwnership::Owned,
                None
            ),
            ResearchFindingQueryState::NotPresent
        );
    }

    #[test]
    fn register_resolution_requires_an_owned_retained_identity() {
        assert_eq!(
            modeled_register_resolution_state(ResearchRegisterPublicationOwnership::Owned, true),
            ResearchFindingQueryState::ConditionSatisfied
        );
        for (ownership, retained) in [
            (ResearchRegisterPublicationOwnership::Owned, false),
            (ResearchRegisterPublicationOwnership::External, true),
            (ResearchRegisterPublicationOwnership::External, false),
        ] {
            assert_eq!(
                modeled_register_resolution_state(ownership, retained),
                ResearchFindingQueryState::NotPresent
            );
        }
    }

    #[test]
    fn inventory_and_selection_are_deterministic_across_construction_order() {
        let mut actions = vec![
            ranked_candidate("alpha", 30, 21, 2),
            ranked_candidate("beta", 30, 21, 2),
            ranked_candidate("gamma", 30, 21, 2),
        ];
        let forward =
            report_from_actions(actions.clone(), ResearchRankingStrategy::Impact, 2, None);
        actions.reverse();
        let reverse = report_from_actions(actions, ResearchRankingStrategy::Impact, 2, None);

        assert_eq!(forward, reverse);
        assert_eq!(
            serde_json::to_vec(&forward).unwrap(),
            serde_json::to_vec(&reverse).unwrap()
        );
    }

    #[test]
    fn budget_selection_skips_actions_that_do_not_fit_without_reordering() {
        let candidates = vec![
            ranked_candidate("first", 60, 20, 3),
            ranked_candidate("too-large", 100, 30, 4),
            ranked_candidate("fills-budget", 10, 10, 1),
        ];

        let (selected, consumed) =
            select_ranked_steps(&[], &[], &candidates, &[0, 1, 2], 10, Some(4));

        assert_eq!(consumed, 4);
        assert_eq!(
            selected
                .iter()
                .map(|step| step.id.as_str())
                .collect::<Vec<_>>(),
            [candidates[0].id.as_str(), candidates[2].id.as_str()]
        );
    }

    #[test]
    fn prerequisite_and_inspection_lanes_share_the_visible_selection() {
        let mut candidate = accumulator("register", "register-model");
        candidate.subject = ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address: 0x6000_1000,
            width: 32,
            assertion: None,
        };
        candidate.blocker_resolution_route = None;
        candidate.consumers = vec![ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::NeedsDestination,
            configured_paths: vec![PathBuf::from("facts.toml")],
            selected_path: None,
            assertion_kinds: vec!["register-identity".to_owned()],
            diagnostic: Some("select default-pack".to_owned()),
        }];
        let action = finalize_test(
            candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect register 0x60001000 --project project.toml",
        );
        let mut second = action.clone();
        second.id = "second-action".to_owned();
        second.findings[0].id = "second-register".to_owned();
        let prerequisites = build_prerequisites(&[action.clone(), second.clone()]);

        assert_eq!(prerequisites.len(), 1);
        assert_eq!(prerequisites[0].satisfies_finding_ids.len(), 2);

        let actions = vec![action, second];
        let (selected, cost) =
            select_ranked_steps(&prerequisites, &[0], &actions, &[0, 1], 2, None);

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].kind, ResearchStepKind::Prerequisite);
        assert_eq!(selected[1].kind, ResearchStepKind::Action);
        assert_eq!(cost, 5);
    }

    #[test]
    fn saturated_selection_stably_interleaves_prerequisites_and_actions() {
        let mut prerequisite = ResearchPrerequisiteAction {
            rank: 0,
            id: String::new(),
            kind: ResearchPrerequisiteKind::CreateInterfaceAnchor,
            reason: "fixture".to_owned(),
            path: None,
            subject: "fixture".to_owned(),
            manual_action: "configure fixture".to_owned(),
            satisfies_finding_ids: Vec::new(),
            blocked_action_ids: Vec::new(),
            guaranteed_unlock: 0,
            optimistic_unlock: 0,
            affected_scope_roots: Vec::new(),
            scopes: Vec::new(),
            benefit_points: 1,
            estimated_cost_units: 1,
        };
        let prerequisites = (0..25)
            .map(|index| {
                prerequisite.id = format!("prerequisite-{index:02}");
                prerequisite.clone()
            })
            .collect::<Vec<_>>();
        let actions = (0..25)
            .map(|index| ranked_candidate(&format!("action-{index:02}"), 10, 10, 1))
            .collect::<Vec<_>>();
        let indices = (0..25).collect::<Vec<_>>();

        let (selected, consumed) =
            select_ranked_steps(&prerequisites, &indices, &actions, &indices, 20, None);

        assert_eq!(consumed, 20);
        assert_eq!(selected.len(), 20);
        assert!(selected.chunks_exact(2).all(|pair| {
            pair[0].kind == ResearchStepKind::Prerequisite
                && pair[1].kind == ResearchStepKind::Action
        }));
    }

    #[test]
    fn scope_and_protocol_filters_are_exact_and_fail_closed() {
        let scopes = BTreeMap::from([
            ("ble-runtime".to_owned(), vec!["ble".to_owned()]),
            ("bredr-runtime".to_owned(), vec!["bluetooth".to_owned()]),
            (
                "btdm-runtime".to_owned(),
                vec!["bluetooth".to_owned(), "ble".to_owned()],
            ),
            (
                "ieee802154-baseband".to_owned(),
                vec!["ieee802154".to_owned()],
            ),
            ("station-state".to_owned(), vec!["wifi".to_owned()]),
            (
                "shared-phy".to_owned(),
                vec![
                    "wifi".to_owned(),
                    "bluetooth".to_owned(),
                    "ble".to_owned(),
                    "ieee802154".to_owned(),
                    "shared".to_owned(),
                ],
            ),
        ]);

        assert_eq!(
            select_scope_ids(&scopes, None, Some("wifi")).unwrap(),
            ["shared-phy".to_owned(), "station-state".to_owned()].into()
        );
        assert_eq!(
            select_scope_ids(&scopes, Some("ble-runtime"), Some("ble")).unwrap(),
            ["ble-runtime".to_owned()].into()
        );
        assert_eq!(
            select_scope_ids(&scopes, None, Some("ble")).unwrap(),
            [
                "ble-runtime".to_owned(),
                "btdm-runtime".to_owned(),
                "shared-phy".to_owned(),
            ]
            .into()
        );
        assert_eq!(
            select_scope_ids(&scopes, None, Some("bluetooth")).unwrap(),
            [
                "bredr-runtime".to_owned(),
                "btdm-runtime".to_owned(),
                "shared-phy".to_owned(),
            ]
            .into()
        );
        let unknown = select_scope_ids(&scopes, None, Some("radio")).unwrap_err();
        assert!(unknown.to_string().contains("configured protocols"));
        let mismatch = select_scope_ids(&scopes, Some("ble-runtime"), Some("wifi")).unwrap_err();
        assert!(mismatch.to_string().contains("not tagged with protocol"));
    }

    #[test]
    fn protocol_filter_normalizes_only_supported_cli_aliases() {
        assert_eq!(normalize_protocol_filter("bt").unwrap(), "bluetooth");
        assert_eq!(normalize_protocol_filter("bluetooth").unwrap(), "bluetooth");
        for alias in ["802.15.4", "802154", "ieee802154"] {
            assert_eq!(normalize_protocol_filter(alias).unwrap(), "ieee802154");
        }
        for canonical in ["wifi", "ble", "coex", "shared"] {
            assert_eq!(normalize_protocol_filter(canonical).unwrap(), canonical);
        }
        assert!(normalize_protocol_filter("radio").is_err());
        assert!(normalize_protocol_filter("Bluetooth").is_err());
    }

    #[test]
    fn one_user_action_keeps_all_related_findings_without_duplicate_ranks() {
        fn candidate(id: &str, message: &str, extra_function: Option<&str>) -> Accumulator {
            let mut direct = BTreeSet::from(["ble-controller::logger".to_owned()]);
            direct.extend(extra_function.map(str::to_owned));
            let mut candidate = accumulator(id, "interface-layout");
            candidate.severity = "warning".to_owned();
            candidate.message = message.to_owned();
            candidate.direct = direct.clone();
            candidate.optimistic = direct.clone();
            candidate.marginal = direct;
            candidate.co_blockers =
                [if id == "slot-a" { "slot-b" } else { "slot-a" }.to_owned()].into();
            candidate.roots = [format!("root-{id}")].into();
            candidate.scopes = ["ble".to_owned()].into();
            candidate.publication_scopes = ["ble".to_owned()].into();
            candidate
        }

        let actions = coalesce_actions(vec![
            finalize_test(
                candidate("slot-a", "review slot A", None),
                &BTreeMap::new(),
                &BTreeMap::new(),
                "blobray inspect function ble-controller:logger --project project.toml",
            ),
            finalize_test(
                candidate("slot-b", "review slot B", Some("ble-controller::worker")),
                &BTreeMap::new(),
                &BTreeMap::new(),
                "blobray inspect function ble-controller:logger --project project.toml",
            ),
        ]);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].findings.len(), 2);
        assert_eq!(actions[0].findings[0].id, "slot-a");
        assert_eq!(actions[0].findings[1].id, "slot-b");
        assert_eq!(actions[0].findings[1].summary, "review slot B");
        assert_eq!(actions[0].findings[0].co_blocker_ids, ["slot-b"]);
        assert_eq!(actions[0].findings[1].co_blocker_ids, ["slot-a"]);
        assert!(actions[0].co_blocker_ids.is_empty());
        assert_eq!(
            actions[0].affected_scope_roots,
            ["root-slot-a", "root-slot-b"]
        );
        assert_eq!(actions[0].direct_functions, 2);
        assert_eq!(
            actions[0].direct_function_ids,
            ["ble-controller::logger", "ble-controller::worker"]
        );
    }

    #[test]
    fn next_action_preserves_cli_selector_as_one_argument() {
        let mut candidate = accumulator("candidate", "unresolved-call");
        candidate.direct = ["archive::function".to_owned()].into();
        assert_eq!(
            next_action_tokens(&candidate),
            ["inspect", "function", "archive:function"]
        );
    }

    #[test]
    fn event_route_blocker_is_visible_without_claiming_function_unlocks() {
        let route_id = "route-a";
        let blocker_kind = "event-queue-producer-precondition-unproven";
        let root = "fixture::root";
        let producer = "fixture::producer";
        let mut graph = ScopeGraph::default();
        graph.nodes.insert(
            root.to_owned(),
            GraphNode {
                source: "fixture".to_owned(),
                symbol: "root".to_owned(),
                dependencies: [producer.to_owned()].into(),
                direct_diagnostic_roots: BTreeSet::new(),
                complete: true,
            },
        );
        graph.nodes.insert(
            producer.to_owned(),
            GraphNode {
                source: "fixture".to_owned(),
                symbol: "producer".to_owned(),
                dependencies: BTreeSet::new(),
                direct_diagnostic_roots: BTreeSet::new(),
                complete: true,
            },
        );
        build_graph_edges(&mut graph);
        let inspection =
            event_route_scope_inspection(&[producer.to_owned()].into(), &graph).unwrap();
        assert_eq!(inspection, [producer.to_owned()].into());

        let finding_id = event_route_finding_id(route_id, blocker_kind);
        let mut candidate = accumulator(&finding_id, blocker_kind);
        candidate.subject = ResearchSubject::EventRouteBlocker {
            route_id: route_id.to_owned(),
            blocker_kind: blocker_kind.to_owned(),
        };
        candidate.blocker_resolution_route = Some(
            crate::blocker_resolution::event_route_blocker_resolution_route(route_id, blocker_kind),
        );
        candidate.inspection = inspection;
        candidate.scopes = ["fixture-scope".to_owned()].into();
        candidate.publication_scopes = ["fixture-scope".to_owned()].into();
        assert_eq!(
            next_action_tokens(&candidate),
            ["inspect", "flow", "--event-route", route_id]
        );

        let action = finalize_test(
            candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &format!("blobray inspect flow --event-route {route_id} --project project.toml"),
        );
        let finding = &action.findings[0];
        assert!(finding.affected_scope_roots.is_empty());
        assert_eq!(finding.scopes, ["fixture-scope"]);
        assert!(finding.direct_function_ids.is_empty());
        assert!(finding.guaranteed_function_ids.is_empty());
        assert!(finding.optimistic_function_ids.is_empty());
        assert!(finding.marginal_function_ids.is_empty());
        assert!(finding.publication_scopes.is_empty());
        assert_eq!(action.direct_functions, 0);
        assert_eq!(action.guaranteed_unlock, 0);
        assert_eq!(action.optimistic_unlock, 0);
        assert_eq!(action.marginal_unlock_after_co_blockers, 0);
        assert_eq!(action.score_breakdown.root_weight, 0);
        assert_eq!(action.score_breakdown.publication_weight, 0);
        assert_eq!(action.score_explanation.benefit_points, 0);
        let resolution = finding.blocker_resolution_route.as_ref().unwrap();
        assert_eq!(
            resolution.completion_predicate.kind,
            crate::BlockerCompletionKind::EventRouteBlockerAbsent
        );
        assert_eq!(
            resolution.completion_predicate.root_id,
            crate::blocker_resolution::event_route_blocker_root(route_id, blocker_kind)
        );
    }

    #[test]
    fn one_event_route_action_does_not_coalesce_distinct_resolution_models() {
        let mut first = accumulator("first", "event-queue-producer-lifetime-unproven");
        first.blocker_resolution_route = Some(
            crate::blocker_resolution::event_route_blocker_resolution_route(
                "route-a",
                "event-queue-producer-lifetime-unproven",
            ),
        );
        let mut second = accumulator("second", "event-receive-run-order-unproven");
        second.blocker_resolution_route = Some(
            crate::blocker_resolution::event_route_blocker_resolution_route(
                "route-a",
                "event-receive-run-order-unproven",
            ),
        );
        let command = "blobray inspect flow --event-route route-a --project project.toml";
        let actions = coalesce_actions(vec![
            finalize_test(first, &BTreeMap::new(), &BTreeMap::new(), command),
            finalize_test(second, &BTreeMap::new(), &BTreeMap::new(), command),
        ]);
        assert_eq!(actions.len(), 2);
        assert_ne!(actions[0].id, actions[1].id);
    }

    #[test]
    fn analysis_surface_next_actions_only_offer_executable_state_transitions() {
        for state in [
            ResearchAnalysisSurfaceState::MissingVendorArtifact,
            ResearchAnalysisSurfaceState::StaleSymbolFamily,
            ResearchAnalysisSurfaceState::MissingProfileDefinition,
        ] {
            assert_eq!(
                next_action_tokens(&analysis_surface_accumulator(state, Some("fixture-ir"))),
                ["project", "status"],
                "manual state {state:?} must not offer a non-runnable IR build"
            );
        }
        assert_eq!(
            next_action_tokens(&analysis_surface_accumulator(
                ResearchAnalysisSurfaceState::MissingSymbolInventory,
                Some("fixture-ir")
            )),
            ["project", "analyze"]
        );
        for state in [
            ResearchAnalysisSurfaceState::MissingProfileOutput,
            ResearchAnalysisSurfaceState::InvalidProfileOutput,
        ] {
            assert_eq!(
                next_action_tokens(&analysis_surface_accumulator(state, Some("fixture-ir"))),
                ["advanced", "ir", "build", "--profile", "fixture-ir"]
            );
            assert_eq!(
                next_action_tokens(&analysis_surface_accumulator(state, None)),
                ["project", "status"],
                "an output state without a configured profile must fail closed"
            );
        }
    }

    #[test]
    fn required_analysis_surface_classifier_covers_every_transition_and_analyzed_omission() {
        let cases = [
            (
                [false, false, false, false, false, false],
                Some(ResearchAnalysisSurfaceState::MissingVendorArtifact),
            ),
            (
                [true, false, false, false, false, false],
                Some(ResearchAnalysisSurfaceState::MissingSymbolInventory),
            ),
            (
                [true, true, false, false, false, false],
                Some(ResearchAnalysisSurfaceState::StaleSymbolFamily),
            ),
            (
                [true, true, true, false, false, false],
                Some(ResearchAnalysisSurfaceState::MissingProfileDefinition),
            ),
            (
                [true, true, true, true, false, false],
                Some(ResearchAnalysisSurfaceState::MissingProfileOutput),
            ),
            (
                [true, true, true, true, true, false],
                Some(ResearchAnalysisSurfaceState::InvalidProfileOutput),
            ),
            ([true, true, true, true, true, true], None),
        ];
        for (inputs, expected) in cases {
            assert_eq!(
                required_analysis_surface_state(
                    inputs[0], inputs[1], inputs[2], inputs[3], inputs[4], inputs[5]
                ),
                expected
            );
        }
    }

    #[test]
    fn analysis_surface_states_separate_manual_prerequisites_from_ready_actions() {
        for state in [
            ResearchAnalysisSurfaceState::MissingVendorArtifact,
            ResearchAnalysisSurfaceState::StaleSymbolFamily,
            ResearchAnalysisSurfaceState::MissingProfileDefinition,
            ResearchAnalysisSurfaceState::MissingSymbolInventory,
            ResearchAnalysisSurfaceState::MissingProfileOutput,
            ResearchAnalysisSurfaceState::InvalidProfileOutput,
        ] {
            let candidate = analysis_surface_accumulator(state, Some("fixture-ir"));
            let next = format!(
                "blobray {} --project project.toml",
                next_action_tokens(&candidate).join(" ")
            );
            let action = finalize_test(candidate, &BTreeMap::new(), &BTreeMap::new(), &next);
            let report =
                report_from_actions(vec![action], ResearchRankingStrategy::Impact, 2, None);
            let finding = &report.inventory.findings[0];
            let automatic = matches!(
                state,
                ResearchAnalysisSurfaceState::MissingSymbolInventory
                    | ResearchAnalysisSurfaceState::MissingProfileOutput
                    | ResearchAnalysisSurfaceState::InvalidProfileOutput
            );
            assert_eq!(
                finding.actionability,
                if automatic {
                    ResearchActionability::Ready
                } else {
                    ResearchActionability::CoverageBlocked
                }
            );
            assert_eq!(finding.prerequisite_ids.len(), usize::from(!automatic));
            assert_eq!(
                report.inventory.prerequisites.len(),
                usize::from(!automatic)
            );
            if state == ResearchAnalysisSurfaceState::MissingProfileDefinition {
                assert_eq!(
                    report.inventory.prerequisites[0].path.as_deref(),
                    Some(Path::new("project.toml"))
                );
            }
        }
    }

    #[test]
    fn ready_analysis_surface_actions_have_the_first_action_lane() {
        let surface_candidate = analysis_surface_accumulator(
            ResearchAnalysisSurfaceState::MissingSymbolInventory,
            Some("fixture-ir"),
        );
        let surface = finalize_test(
            surface_candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray project analyze --project project.toml",
        );
        let ordinary = ranked_candidate("ordinary-ready", 1_000, 1, 1);
        let actions = vec![ordinary, surface];
        let ranked = ranked_action_indices(&actions, ResearchRankingStrategy::Impact);
        assert_eq!(actions[ranked[0]].kinds, ["analysis-surface"]);
    }

    #[test]
    fn next_action_prefers_the_causal_function_over_an_impacted_caller() {
        let mut candidate = accumulator("candidate", "call-boundary");
        candidate.direct = ["libpp::caller".to_owned()].into();
        candidate.inspection = ["libpp::causal_callee@0x10001000".to_owned()].into();

        assert_eq!(
            next_action_tokens(&candidate),
            ["inspect", "function", "libpp:causal_callee@0x10001000"]
        );
    }

    #[test]
    fn register_inspection_command_uses_typed_subject_not_text() {
        let mut candidate = accumulator("misleading-id", "register-write-semantics");
        candidate.message = "message contains 0xdeadbeef".to_owned();
        candidate.subject = ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address: 0x6000_1000,
            width: 32,
            assertion: Some("write-semantics".to_owned()),
        };
        candidate.blocker_resolution_route = None;

        assert_eq!(
            next_action_tokens(&candidate),
            ["inspect", "register", "0x60001000"]
        );
    }

    #[test]
    fn external_registers_cannot_route_identities_or_write_semantics() {
        let facts = crate::registers::RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                crate::registers::FactRange {
                    name: "coex-hw-timer".to_owned(),
                    start: 0x2010_f400,
                    end: 0x2010_f450,
                },
                crate::registers::FactRange {
                    name: "modem-lpcon-platform-high".to_owned(),
                    start: 0x2010_f450,
                    end: 0x2010_f800,
                },
                crate::registers::FactRange {
                    name: "lp-peripheral".to_owned(),
                    start: 0x2080_0000,
                    end: 0x2090_0000,
                },
            ],
            registers: Vec::new(),
        };
        let owned = vec!["coex-hw-timer".to_owned()];
        let owned_control = classify_register_publication(&facts, &owned, 0x2010_f420, 32).unwrap();
        let external_coex = classify_register_publication(&facts, &owned, 0x2010_f4a0, 32).unwrap();
        let external_tsens =
            classify_register_publication(&facts, &owned, 0x2081_8000, 32).unwrap();
        let consumer = || ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::Ready,
            configured_paths: vec![PathBuf::from("reviewed/project-facts.toml")],
            selected_path: Some(PathBuf::from("reviewed/project-facts.toml")),
            assertion_kinds: vec!["register-identity".to_owned()],
            diagnostic: None,
        };

        assert_eq!(
            register_publication_consumers(owned_control, consumer).len(),
            1
        );
        assert!(register_publication_consumers(external_coex, consumer).is_empty());
        assert!(register_publication_consumers(external_tsens, consumer).is_empty());

        let write_semantics = register_publication_consumers(external_coex, || {
            ResearchConsumer::ReviewedKnowledgeAssertions {
                resolution: ResearchConsumerResolution::Ready,
                configured_paths: vec![PathBuf::from("reviewed/project-facts.toml")],
                selected_path: Some(PathBuf::from("reviewed/project-facts.toml")),
                assertion_kinds: vec!["hardware-write-semantics".to_owned()],
                diagnostic: None,
            }
        });
        assert!(write_semantics.is_empty());
        assert_eq!(
            finding_actionability(&write_semantics),
            ResearchActionability::InspectionOnly
        );
    }

    #[test]
    fn interface_finding_identity_distinguishes_same_slot_in_different_tables() {
        assert_ne!(
            interface_finding_id("fact-0@+0x10/32"),
            interface_finding_id("fact-1@+0x10/32")
        );
    }

    #[test]
    fn overlapping_candidates_are_co_blockers_only_within_one_domain() {
        fn candidate(id: &str, kind: &str) -> Accumulator {
            let mut candidate = accumulator(id, kind);
            candidate.severity = "warning".to_owned();
            candidate.message = "review".to_owned();
            candidate.direct = ["function".to_owned()].into();
            candidate.scopes.clear();
            candidate
        }

        let mut candidates = BTreeMap::from([
            (
                "interface-a".to_owned(),
                candidate("interface-a", "interface-layout"),
            ),
            (
                "interface-b".to_owned(),
                candidate("interface-b", "interface-layout"),
            ),
            (
                "register".to_owned(),
                candidate("register", "register-model"),
            ),
        ]);
        attach_candidate_co_blockers(&mut candidates);
        assert_eq!(
            candidates["interface-a"].co_blockers,
            ["interface-b".to_owned()].into()
        );
        assert_eq!(
            candidates["interface-b"].co_blockers,
            ["interface-a".to_owned()].into()
        );
        assert!(candidates["register"].co_blockers.is_empty());
    }

    #[test]
    fn schema_eighteen_serializes_focus_routes_query_and_executable_actions() {
        let mut candidate = accumulator("register", "register-model");
        candidate.subject = ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address: 0x6000_1000,
            width: 32,
            assertion: None,
        };
        candidate.blocker_resolution_route = None;
        candidate.consumers = vec![ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::NeedsDestination,
            configured_paths: vec![PathBuf::from("a.toml"), PathBuf::from("b.toml")],
            selected_path: None,
            assertion_kinds: vec!["register-identity".to_owned()],
            diagnostic: Some("select one pack".to_owned()),
        }];
        let action = finalize_test(
            candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect register 0x60001000 --project project.toml",
        );

        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 10, None);
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema_version"], 18);
        assert_eq!(value["focus"], "all");
        assert_eq!(value["finding_query"]["state"], "all");
        assert_eq!(value["finding_query"]["completion_claim"], false);
        assert_eq!(
            value["inventory"]["findings"][0]["subject"]["kind"],
            "mmio-register"
        );
        assert_eq!(
            value["inventory"]["findings"][0]["consumers"][0]["kind"],
            "reviewed-knowledge-assertions"
        );
        assert_eq!(
            value["inventory"]["findings"][0]["consumers"][0]["resolution"],
            "needs-destination"
        );
        assert_eq!(
            value["inventory"]["findings"][0]["actionability"],
            "needs-destination"
        );
        assert_eq!(
            value["inventory"]["findings"][0]["resolution_owner"],
            "reviewed-knowledge"
        );
        assert_eq!(
            value["inventory"]["actions"][0]["resolution_owner"],
            "reviewed-knowledge"
        );
        assert_eq!(
            value["inventory"]["actions"][0]["required_model"],
            knowledge_required("register-model")
        );
        assert_eq!(
            value["inventory"]["actions"][0]["finding_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(value["inventory"]["actions"][0].get("findings").is_none());
        assert_eq!(
            value["inventory"]["actions"][0]["next_action"]["argv"][0],
            "blobray"
        );
        assert_eq!(
            value["inventory"]["actions"][0]["next_action"]["context"],
            "analysis"
        );
        assert!(
            value["inventory"]["actions"][0]
                .get("inspect_command")
                .is_none()
        );
        assert_eq!(
            value["inventory"]["findings"][0]["requery_action"]["argv"][1],
            "project"
        );
        assert!(
            value["inventory"]["findings"][0]
                .get("revalidation_commands")
                .is_none()
        );
        assert!(value["inventory"]["prerequisites"][0].get("rank").is_none());
        assert_eq!(value["selection"]["steps"][0]["kind"], "prerequisite");
    }

    #[test]
    fn report_validation_rejects_missing_and_forged_blocker_routes_after_resigning() {
        let report = report_from_actions(
            vec![ranked_candidate("typed-root", 10, 2, 1)],
            ResearchRankingStrategy::Impact,
            10,
            None,
        );

        let mut missing = report.clone();
        missing.inventory.findings[0].blocker_resolution_route = None;
        refresh_inventory_digest(&mut missing);
        assert!(
            validate_report(&missing)
                .unwrap_err()
                .to_string()
                .contains("has no typed resolution route")
        );

        let mut forged = report;
        let route = forged.inventory.findings[0]
            .blocker_resolution_route
            .as_mut()
            .unwrap();
        route.owner = crate::BlockerResolutionOwner::GenericBackend;
        route.destination = Some(PathBuf::from("reviewed.toml"));
        route.record_kind = Some(crate::BlockerResolutionRecordKind::ReviewedFunctionFact);
        route.record_action = Some("hide the producer diagnostic".to_owned());
        refresh_inventory_digest(&mut forged);
        assert!(
            validate_report(&forged)
                .unwrap_err()
                .to_string()
                .contains("owner without a declarative consumer")
        );
    }

    #[test]
    fn register_subject_parser_preserves_non_cpu_address_space() {
        let subject = "register:esp32s31/radio/0x60001000/32"
            .parse::<SemanticEntityId>()
            .unwrap();
        assert_eq!(
            register_entity(&subject),
            Some(("esp32s31", "radio", 0x6000_1000, 32))
        );
    }

    #[test]
    fn mixed_kind_action_cost_and_findings_are_order_independent() {
        let command = "blobray inspect function radio:worker --project project.toml";
        let action_for = |id: &str, kind: &str| {
            let mut candidate = accumulator(id, kind);
            candidate.subject = ResearchSubject::MmioRegister {
                address_space: "radio".to_owned(),
                address: 0x6000_1000,
                width: 32,
                assertion: None,
            };
            candidate.blocker_resolution_route = None;
            candidate.direct = ["radio::worker".to_owned()].into();
            finalize_test(candidate, &BTreeMap::new(), &BTreeMap::new(), command)
        };
        let analysis = action_for("analysis", "unresolved-call");
        let semantics = action_for("semantics", "register-write-semantics");

        let forward = coalesce_actions(vec![analysis.clone(), semantics.clone()]);
        let reverse = coalesce_actions(vec![semantics, analysis]);

        assert_eq!(forward, reverse);
        assert_eq!(forward.len(), 2);
        let semantics = forward
            .iter()
            .find(|action| action.kinds == ["register-write-semantics"])
            .unwrap();
        assert_eq!(semantics.score_explanation.estimated_cost_units, 6);
        assert_eq!(semantics.confidence, "low-until-hil");
        let analysis = forward
            .iter()
            .find(|action| action.kinds == ["unresolved-call"])
            .unwrap();
        assert_eq!(analysis.score_explanation.estimated_cost_units, 2);
    }

    #[test]
    fn action_identity_uses_typed_owner_and_required_model_for_every_finding() {
        let command = "blobray inspect function radio:worker --project project.toml";
        let action_for = |id: &str, subject: ResearchSubject, kind: &str| {
            let mut candidate = accumulator(id, kind);
            candidate.subject = subject;
            candidate.blocker_resolution_route = None;
            finalize_test(candidate, &BTreeMap::new(), &BTreeMap::new(), command)
        };
        let register_subject = |address| ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address,
            width: 32,
            assertion: None,
        };
        let public_surface = ResearchSubject::PublicSymbolFamily {
            surface: "controller".to_owned(),
            protocols: vec!["ble".to_owned()],
            source: "controller".to_owned(),
            symbol_prefix: "r_".to_owned(),
            profile: Some("controller".to_owned()),
            state: ResearchAnalysisSurfaceState::MissingSymbolInventory,
        };

        let same_pair = coalesce_actions(vec![
            action_for("first", register_subject(0x6000_1000), "register-model"),
            action_for("second", register_subject(0x6000_1004), "register-model"),
        ]);
        assert_eq!(same_pair.len(), 1);
        assert_eq!(same_pair[0].findings.len(), 2);

        let different_model = coalesce_actions(vec![
            action_for("identity", register_subject(0x6000_1000), "register-model"),
            action_for(
                "semantics",
                register_subject(0x6000_1004),
                "register-write-semantics",
            ),
        ]);
        assert_eq!(different_model.len(), 2);

        let different_owner = coalesce_actions(vec![
            action_for("register", register_subject(0x6000_1000), "unresolved-call"),
            action_for("surface", public_surface, "unresolved-call"),
        ]);
        assert_eq!(different_owner.len(), 2);
        assert_eq!(
            different_owner
                .iter()
                .map(|action| action.findings[0].resolution_owner)
                .collect::<BTreeSet<_>>(),
            [
                crate::BlockerResolutionOwner::ProjectComposition,
                crate::BlockerResolutionOwner::ReviewedKnowledge,
            ]
            .into()
        );
    }
}
