//! Explainable, scope-aware prioritization of the next research action.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::Write,
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ProjectSession;
use crate::{
    Result,
    registers::{
        ProjectRegisterWorkspace, RegisterPublicationOwnership, classify_register_publication,
    },
    review_scopes::{ReviewScopeReport, ReviewScopesDocument},
};

pub(crate) const RESEARCH_SCHEMA: u32 = 10;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResearchNextOptions<'a> {
    pub(crate) scope: Option<&'a str>,
    pub(crate) protocol: Option<&'a str>,
    pub(crate) finding: Option<&'a str>,
    pub(crate) strategy: ResearchRankingStrategy,
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
    pub(crate) consumers: Vec<ResearchConsumer>,
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
    pub(crate) revalidation_commands: Vec<String>,
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
    pub(crate) inspect_command: String,
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
    pub(crate) inspect_command: String,
    pub(crate) estimated_cost: String,
    pub(crate) confidence: String,
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
pub(crate) struct ResearchNextReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) project: String,
    pub(crate) protocol: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) analyzed_scopes: Vec<String>,
    pub(crate) finding_query: ResearchFindingQuery,
    /// Research prioritization never proves that the investigation is complete.
    pub(crate) completion_claim: bool,
    pub(crate) capability_diagnostic: Option<String>,
    pub(crate) verification_diagnostic: Option<String>,
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
    consumers: Vec<ResearchConsumer>,
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
    consumers: Vec<ResearchConsumer>,
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
    let mut candidates = BTreeMap::new();
    for scope in &scopes {
        add_blockers(
            scope,
            &graphs[&scope.id],
            &direct_diagnostic_owners,
            &mut candidates,
        )?;
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
    add_interfaces(session, &scopes, &graphs, &mut candidates)?;
    attach_candidate_co_blockers(&mut candidates);
    let finding_query = apply_finding_query(&mut candidates, options.finding, exact_resolution)?;
    let (capabilities, capability_diagnostic) = capability_contexts(session);
    let (surfaces, verification_diagnostic) = verification_contexts(&session.project);
    let context = session.context();
    let ranked = candidates
        .into_values()
        .map(|candidate| {
            let inspect_command = context.follow_up_command(
                &next_command(&candidate),
                super::FollowUpRequirements::ANALYSIS,
            );
            let revalidation_command =
                context.follow_up_command("project analyze", super::FollowUpRequirements::ANALYSIS);
            finalize(
                candidate,
                &capabilities,
                &surfaces,
                inspect_command,
                revalidation_command,
            )
        })
        .collect::<Vec<_>>();
    let actions = coalesce_actions(ranked);
    let prerequisites = build_prerequisites(&actions);
    let prerequisite_indices = ranked_prerequisite_indices(&prerequisites, options.strategy);
    let action_indices = ranked_action_indices(&actions, options.strategy);
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
    let report = ResearchNextReport {
        schema_version: RESEARCH_SCHEMA,
        command: "research next".to_owned(),
        project: session.project.id.clone(),
        protocol: selected_protocol.map(str::to_owned),
        scope: options.scope.map(str::to_owned),
        analyzed_scopes,
        finding_query,
        completion_claim: false,
        capability_diagnostic,
        verification_diagnostic,
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
            inspect_command: action.inspect_command,
            estimated_cost: action.estimated_cost,
            confidence: action.confidence,
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
    if report.completion_claim
        || report.finding_query.completion_claim
        || report.finding_query.historical_finding_claim
    {
        return Err(crate::Error::invalid(
            "research finding lookup cannot claim completion or historical occurrence",
        ));
    }
    let inventory = &report.inventory;
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
    let mut referenced_findings = BTreeSet::new();
    for action in &inventory.actions {
        validate_sorted_unique_ids(
            "action finding reference",
            action.finding_ids.iter().map(String::as_str),
        )?;
        for finding in &action.finding_ids {
            if !finding_ids.contains(finding.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research action {:?} references missing finding {finding:?}",
                    action.id
                )));
            }
            if !referenced_findings.insert(finding.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research finding {finding:?} belongs to more than one action"
                )));
            }
        }
    }
    if referenced_findings != finding_ids {
        return Err(crate::Error::invalid(
            "research inventory contains a finding that belongs to no action",
        ));
    }
    for prerequisite in &inventory.prerequisites {
        for finding in &prerequisite.satisfies_finding_ids {
            if !finding_ids.contains(finding.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research prerequisite {:?} references missing finding {finding:?}",
                    prerequisite.id
                )));
            }
        }
        for action in &prerequisite.blocked_action_ids {
            if !action_ids.contains(action.as_str()) {
                return Err(crate::Error::invalid(format!(
                    "research prerequisite {:?} references missing action {action:?}",
                    prerequisite.id
                )));
            }
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
    let expected = format!(
        "mmio:{}:{:#010x}/{}",
        subject.address_space, subject.address, subject.width
    );
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
                && !assertion.evidence.is_empty()
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
                && !assertion.evidence.is_empty()
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

fn ranked_action_indices(
    candidates: &[ResearchAction],
    strategy: ResearchRankingStrategy,
) -> Vec<usize> {
    let mut indices = (0..candidates.len())
        .filter(|index| {
            strategy != ResearchRankingStrategy::Frontier
                || !candidates.iter().enumerate().any(|(other_index, other)| {
                    other_index != *index
                        && actionability_lane(other) == actionability_lane(&candidates[*index])
                        && dominates(other, &candidates[*index])
                })
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

fn actionability_lane(action: &ResearchAction) -> u8 {
    if action.actionability.ready.count != 0 {
        0
    } else if action.actionability.inspection_only.count != 0 {
        1
    } else {
        2
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
    for index in prerequisite_indices {
        if selected.len() == limit {
            break;
        }
        let prerequisite = &prerequisites[*index];
        let cost = prerequisite.estimated_cost_units;
        if budget.is_some_and(|budget| consumed_budget.saturating_add(cost) > budget) {
            continue;
        }
        consumed_budget = consumed_budget.saturating_add(cost);
        selected.push(ResearchStepRef {
            kind: ResearchStepKind::Prerequisite,
            id: prerequisite.id.clone(),
        });
    }
    for index in action_indices {
        if selected.len() == limit {
            break;
        }
        let action = &actions[*index];
        let cost = action.score_explanation.estimated_cost_units;
        if budget.is_some_and(|budget| consumed_budget.saturating_add(cost) > budget) {
            continue;
        }
        consumed_budget = consumed_budget.saturating_add(cost);
        selected.push(ResearchStepRef {
            kind: ResearchStepKind::Action,
            id: action.id.clone(),
        });
    }
    (selected, consumed_budget)
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
    scope: &ReviewScopeReport,
    graph: &ScopeGraph,
    direct_diagnostic_owners: &DirectDiagnosticOwners,
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
                consumers: Vec::new(),
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

fn merge(
    candidates: &mut BTreeMap<String, Accumulator>,
    seed: Seed,
    scope: &ReviewScopeReport,
) -> Result<()> {
    if let Some(existing) = candidates.get(&seed.id)
        && (existing.kind != seed.kind
            || existing.severity != seed.severity
            || existing.message != seed.message
            || existing.subject != seed.subject
            || existing.consumers != seed.consumers)
    {
        return Err(crate::Error::invalid(format!(
            "research finding id {:?} resolves to conflicting typed subjects or consumers",
            seed.id
        )));
    }
    let item = candidates
        .entry(seed.id.clone())
        .or_insert_with(|| Accumulator {
            id: seed.id,
            kind: seed.kind,
            severity: seed.severity,
            message: seed.message,
            subject: seed.subject,
            consumers: seed.consumers,
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
    if scope.publication {
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
    let identities = model.register_identities()?;
    for fact in &facts.registers {
        if identities.contains_key(&(u64::from(fact.address), u32::from(fact.width))) {
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
                consumers: template.consumers.clone(),
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
        let Some((address_space, address, width)) = parse_register(&assertion.subject) else {
            continue;
        };
        if address_space != model_address_space {
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
                    address_space,
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

fn parse_register(subject: &str) -> Option<(String, u32, u8)> {
    let value = subject.strip_prefix("mmio:")?;
    let (address_space, physical) = value.split_once(':')?;
    if address_space.is_empty() {
        return None;
    }
    let physical = physical.split('#').next()?;
    let (address, width) = physical.split_once('/')?;
    Some((
        address_space.to_owned(),
        u32::from_str_radix(address.strip_prefix("0x")?, 16).ok()?,
        width.parse().ok()?,
    ))
}

fn add_interfaces(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let Some(workspace) = session.interface_workspace()? else {
        return Ok(());
    };
    for observation in workspace.unreviewed_observations() {
        let id = interface_finding_id(observation);
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
                    consumers: vec![interface_consumer(session, workspace, observation)],
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

fn interface_finding_id(observation: &crate::interfaces::UnreviewedInterfaceObservation) -> String {
    stable_id("interface", &observation.id)
}

fn interface_consumer(
    session: &ProjectSession,
    workspace: &crate::interfaces::InterfaceWorkspace,
    observation: &crate::interfaces::UnreviewedInterfaceObservation,
) -> ResearchConsumer {
    let path = session
        .project
        .interfaces
        .as_ref()
        .and_then(|paths| paths.pack.clone());
    let contract = workspace
        .contracts()
        .iter()
        .find(|contract| contract.id == observation.contract);
    let (resolution, anchor, template, diagnostic) = match (path.as_ref(), contract) {
        (None, _) => (
            ResearchConsumerResolution::NeedsDestination,
            None,
            None,
            Some("project has no reviewed interface pack".to_owned()),
        ),
        (Some(_), None) => (
            ResearchConsumerResolution::NeedsAnchor,
            None,
            None,
            Some("observation is not bound to a reviewed interface anchor".to_owned()),
        ),
        (Some(_), Some(contract)) if contract.template.is_some() => (
            ResearchConsumerResolution::NeedsAnchor,
            Some(contract.anchor.clone()),
            contract.template.clone(),
            Some("templated anchors cannot accept an unreviewed additive project slot".to_owned()),
        ),
        (Some(_), Some(contract)) => (
            ResearchConsumerResolution::Ready,
            Some(contract.anchor.clone()),
            None,
            None,
        ),
    };
    ResearchConsumer::InterfacePackSlot {
        resolution,
        path,
        contract: observation.contract.clone(),
        anchor,
        template,
        offset: observation.offset,
        width: observation.width,
        diagnostic,
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
    if assertion.kind != "hardware-write-semantics" || assertion.evidence.is_empty() {
        return Ok(None);
    }
    let Some(effective_write_semantics) = normalize_write_semantics(&assertion.value) else {
        return Ok(None);
    };
    let Some((address_space, address, width)) = parse_register(&assertion.subject) else {
        return Ok(None);
    };
    if address_space != workspace.model().address_space() {
        return Ok(None);
    }
    let subject = ResearchRegisterResolutionSubject {
        address_space,
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
    let model_sources = current_identity
        .as_deref()
        .map(|identity| model_sources(workspace, identity))
        .unwrap_or_default();
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
    let model_sources = current_identity
        .as_deref()
        .map(|identity| model_sources(workspace, identity))
        .unwrap_or_default();
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
        .reviewed_register_identities()
        .iter()
        .find(|identity| {
            identity.address_space == workspace.model().address_space()
                && identity.address == u64::from(address)
                && identity.width == u32::from(width)
                && current_identity.as_deref() == Some(identity.identity.as_str())
        })
        .and_then(|identity| {
            let configured = knowledge.assertions().get(&identity.assertion.id)?;
            (configured == &identity.assertion
                && register_identity_assertion_matches(
                    &identity.identity,
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

fn model_sources(workspace: &ProjectRegisterWorkspace, identity: &str) -> Vec<String> {
    workspace
        .model()
        .review()
        .iter()
        .filter(|annotation| annotation.entity == identity)
        .flat_map(|annotation| annotation.sources.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
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

fn capability_contexts(session: &ProjectSession) -> (CapabilityContexts, Option<String>) {
    let Some(paths) = session.project.interfaces.as_ref() else {
        return (BTreeMap::new(), None);
    };
    if paths.capability_packs.is_empty() {
        return (BTreeMap::new(), None);
    }
    let workspace = match session.interface_workspace() {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return (BTreeMap::new(), None),
        Err(error) => return (BTreeMap::new(), Some(error.to_string())),
    };
    let report = match workspace.evaluate_capabilities(&paths.capability_packs) {
        Ok(report) => report,
        Err(error) => return (BTreeMap::new(), Some(error.to_string())),
    };
    let mut by_function = CapabilityContexts::new();
    for rule in report.rules {
        for requirement in rule.requirements {
            let requirement_kind = match requirement.kind {
                crate::interfaces::CapabilityMatcherKind::Operation => "operation",
                crate::interfaces::CapabilityMatcherKind::Effect => "effect",
                crate::interfaces::CapabilityMatcherKind::Call => "call",
            };
            for evidence in requirement.matches {
                let Some(function) = evidence.function else {
                    continue;
                };
                by_function
                    .entry(function.clone())
                    .or_default()
                    .insert(ResearchCapabilityLink {
                        rule: rule.id.clone(),
                        status: rule.status.label().to_owned(),
                        requirement_kind: requirement_kind.to_owned(),
                        requirement: requirement.value.clone(),
                        function,
                        evidence_site: evidence.site,
                        relation: ResearchLinkRelation::ExistingEvidenceContext,
                    });
            }
        }
    }
    (by_function, None)
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

fn ranked_prerequisite_indices(
    prerequisites: &[ResearchPrerequisiteAction],
    strategy: ResearchRankingStrategy,
) -> Vec<usize> {
    let mut indices = (0..prerequisites.len())
        .filter(|index| {
            let prerequisite = &prerequisites[*index];
            strategy != ResearchRankingStrategy::Frontier
                || !prerequisites
                    .iter()
                    .enumerate()
                    .any(|(other_index, other)| {
                        other_index != *index
                            && other.benefit_points >= prerequisite.benefit_points
                            && other.estimated_cost_units <= prerequisite.estimated_cost_units
                            && (other.benefit_points > prerequisite.benefit_points
                                || other.estimated_cost_units < prerequisite.estimated_cost_units)
                    })
        })
        .collect::<Vec<_>>();
    indices.sort_by(|left, right| {
        let left = &prerequisites[*left];
        let right = &prerequisites[*right];
        match strategy {
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
        }
    });
    indices
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
    inspect_command: String,
    revalidation_command: String,
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
        consumers: candidate.consumers,
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
        publication_scopes: candidate.publication_scopes.into_iter().collect(),
        knowledge_required: knowledge_required(&kind).to_owned(),
        evidence_required: evidence_required(&kind),
        revalidation_commands: vec![revalidation_command],
        summary: candidate.message,
    };
    let mut result = ResearchAction {
        rank: 0,
        id: stable_id("action", &inspect_command),
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
        inspect_command,
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

fn coalesce_actions(candidates: Vec<ResearchAction>) -> Vec<ResearchAction> {
    let mut actions = Vec::<ResearchAction>::new();
    let mut by_command = BTreeMap::<String, usize>::new();
    for candidate in candidates {
        if let Some(index) = by_command.get(&candidate.inspect_command).copied() {
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
            by_command.insert(candidate.inspect_command.clone(), actions.len());
            actions.push(candidate);
        }
    }
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
    candidate.score_breakdown = ResearchScoreBreakdown {
        guaranteed_weight: candidate.guaranteed_unlock as u64 * 20,
        optimistic_weight: candidate.optimistic_unlock as u64 * 3,
        marginal_weight: candidate.marginal_unlock_after_co_blockers as u64 * 5,
        root_weight: candidate.affected_scope_roots.len() as u64 * 10,
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

fn next_command(candidate: &Accumulator) -> String {
    if let ResearchSubject::MmioRegister { address, .. } = &candidate.subject {
        return format!("inspect register {address:#010x}");
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
        format!(
            "inspect function {}",
            crate::shell::arg(std::ffi::OsStr::new(&selector))
        )
    } else if let Some(scope) = candidate.scopes.first() {
        format!(
            "inspect scope {}",
            crate::shell::arg(std::ffi::OsStr::new(scope))
        )
    } else {
        "project status".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accumulator(id: &str, kind: &str) -> Accumulator {
        Accumulator {
            id: id.to_owned(),
            kind: kind.to_owned(),
            severity: "error".to_owned(),
            message: format!("resolve {id}"),
            subject: ResearchSubject::AnalysisRoot {
                root_id: id.to_owned(),
            },
            consumers: Vec::new(),
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

    fn ranked_candidate(id: &str, benefit: u64, effort: u64, cost: u64) -> ResearchAction {
        let mut accumulator = accumulator(id, "unresolved-call");
        accumulator.direct.insert(id.to_owned());
        let mut candidate = finalize(
            accumulator,
            &BTreeMap::new(),
            &BTreeMap::new(),
            format!("blobray inspect function {id} --project project.toml"),
            "blobray project analyze --project project.toml".to_owned(),
        );
        candidate.id = id.to_owned();
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
        let prerequisites = build_prerequisites(&actions);
        let prerequisite_indices = ranked_prerequisite_indices(&prerequisites, strategy);
        let action_indices = ranked_action_indices(&actions, strategy);
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
        assert_eq!(next_command(&candidate), "inspect scope radio");
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
        let result = finalize(
            candidate,
            &BTreeMap::from([("leaf".to_owned(), [capability].into())]),
            &BTreeMap::from([("radio".to_owned(), [surface].into())]),
            "blobray inspect function leaf --project project.toml".to_owned(),
            "blobray project analyze --project project.toml".to_owned(),
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
            result.inspect_command,
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
                .map(|index| candidates[*index].id.as_str())
                .collect::<Vec<_>>(),
            ["high-ratio", "high-benefit", "quick", "dominated"]
        );

        let quick_wins = ranked_action_indices(&candidates, ResearchRankingStrategy::QuickWins);
        assert_eq!(
            quick_wins
                .iter()
                .map(|index| candidates[*index].id.as_str())
                .collect::<Vec<_>>(),
            ["quick", "dominated", "high-ratio", "high-benefit"]
        );

        let frontier = ranked_action_indices(&candidates, ResearchRankingStrategy::Frontier);
        assert_eq!(
            frontier
                .iter()
                .map(|index| candidates[*index].id.as_str())
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

        assert_eq!(report.schema_version, 10);
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
        assert_eq!(impact.selection.steps[0].id, "impact");
        assert_eq!(quick.selection.steps[0].id, "quick");
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
            "blobray-research-schema10-check-{}.json",
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
schema = 1
id = "fixture"
[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"
[[assertions]]
id = "radio.status.identity"
subject = "mmio:cpu:0x10000000/32"
kind = "register-identity"
value = "RADIO.STATUS"
[[assertions.evidence]]
source = "MANUAL"
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
            ["first", "fills-budget"]
        );
    }

    #[test]
    fn prerequisite_lane_consumes_the_shared_limit_before_research_actions() {
        let mut candidate = accumulator("register", "register-model");
        candidate.subject = ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address: 0x6000_1000,
            width: 32,
            assertion: None,
        };
        candidate.consumers = vec![ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::NeedsDestination,
            configured_paths: vec![PathBuf::from("facts.toml")],
            selected_path: None,
            assertion_kinds: vec!["register-identity".to_owned()],
            diagnostic: Some("select default-pack".to_owned()),
        }];
        let action = finalize(
            candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect register 0x60001000 --project project.toml".to_owned(),
            "blobray project analyze --project project.toml".to_owned(),
        );
        let mut second = action.clone();
        second.id = "second-action".to_owned();
        second.findings[0].id = "second-register".to_owned();
        let prerequisites = build_prerequisites(&[action.clone(), second.clone()]);

        assert_eq!(prerequisites.len(), 1);
        assert_eq!(prerequisites[0].satisfies_finding_ids.len(), 2);

        let actions = vec![action, second];
        let (selected, cost) =
            select_ranked_steps(&prerequisites, &[0], &actions, &[0, 1], 1, None);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].kind, ResearchStepKind::Prerequisite);
        assert_eq!(cost, 1);
    }

    #[test]
    fn scope_and_protocol_filters_are_exact_and_fail_closed() {
        let scopes = BTreeMap::from([
            (
                "ble-runtime".to_owned(),
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
            finalize(
                candidate("slot-a", "review slot A", None),
                &BTreeMap::new(),
                &BTreeMap::new(),
                "blobray inspect function ble-controller:logger --project project.toml".to_owned(),
                "blobray project analyze --project project.toml".to_owned(),
            ),
            finalize(
                candidate("slot-b", "review slot B", Some("ble-controller::worker")),
                &BTreeMap::new(),
                &BTreeMap::new(),
                "blobray inspect function ble-controller:logger --project project.toml".to_owned(),
                "blobray project analyze --project project.toml".to_owned(),
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
    fn next_command_converts_internal_identity_to_cli_selector() {
        let mut candidate = accumulator("candidate", "unresolved-call");
        candidate.direct = ["archive::function".to_owned()].into();
        assert_eq!(
            next_command(&candidate),
            "inspect function archive:function"
        );
    }

    #[test]
    fn next_command_prefers_the_causal_function_over_an_impacted_caller() {
        let mut candidate = accumulator("candidate", "call-boundary");
        candidate.direct = ["libpp::caller".to_owned()].into();
        candidate.inspection = ["libpp::causal_callee@0x10001000".to_owned()].into();

        assert_eq!(
            next_command(&candidate),
            "inspect function libpp:causal_callee@0x10001000"
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

        assert_eq!(next_command(&candidate), "inspect register 0x60001000");
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
        let observation = |id: &str| crate::interfaces::UnreviewedInterfaceObservation {
            id: id.to_owned(),
            contract: "unmatched:btbb:relocated-symbol".to_owned(),
            source: "btbb".to_owned(),
            offset: 0x10,
            width: 32,
            selector: None,
            functions: vec!["btbb::worker".to_owned()],
            call_sites: vec![0x1000],
        };

        assert_ne!(
            interface_finding_id(&observation("fact-0@+0x10/32")),
            interface_finding_id(&observation("fact-1@+0x10/32"))
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
    fn schema_ten_serializes_query_and_one_typed_finding_catalog() {
        let mut candidate = accumulator("register", "register-model");
        candidate.subject = ResearchSubject::MmioRegister {
            address_space: "radio".to_owned(),
            address: 0x6000_1000,
            width: 32,
            assertion: None,
        };
        candidate.consumers = vec![ResearchConsumer::ReviewedKnowledgeAssertions {
            resolution: ResearchConsumerResolution::NeedsDestination,
            configured_paths: vec![PathBuf::from("a.toml"), PathBuf::from("b.toml")],
            selected_path: None,
            assertion_kinds: vec!["register-identity".to_owned()],
            diagnostic: Some("select one pack".to_owned()),
        }];
        let action = finalize(
            candidate,
            &BTreeMap::new(),
            &BTreeMap::new(),
            "blobray inspect register 0x60001000 --project project.toml".to_owned(),
            "blobray project analyze --project project.toml".to_owned(),
        );

        let report = report_from_actions(vec![action], ResearchRankingStrategy::Impact, 10, None);
        let value = serde_json::to_value(report).unwrap();
        assert_eq!(value["schema_version"], 10);
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
            value["inventory"]["actions"][0]["finding_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(value["inventory"]["actions"][0].get("findings").is_none());
        assert!(value["inventory"]["prerequisites"][0].get("rank").is_none());
        assert_eq!(value["selection"]["steps"][0]["kind"], "prerequisite");
    }

    #[test]
    fn register_subject_parser_preserves_non_cpu_address_space() {
        assert_eq!(
            parse_register("mmio:radio:0x60001000/32#write-semantics"),
            Some(("radio".to_owned(), 0x6000_1000, 32))
        );
    }

    #[test]
    fn mixed_kind_action_cost_and_findings_are_order_independent() {
        let command = "blobray inspect function radio:worker --project project.toml";
        let revalidate = "blobray project analyze --project project.toml";
        let action_for = |id: &str, kind: &str| {
            let mut candidate = accumulator(id, kind);
            candidate.direct = ["radio::worker".to_owned()].into();
            finalize(
                candidate,
                &BTreeMap::new(),
                &BTreeMap::new(),
                command.to_owned(),
                revalidate.to_owned(),
            )
        };
        let analysis = action_for("analysis", "unresolved-call");
        let semantics = action_for("semantics", "register-write-semantics");

        let forward = coalesce_actions(vec![analysis.clone(), semantics.clone()]);
        let reverse = coalesce_actions(vec![semantics, analysis]);

        assert_eq!(forward, reverse);
        assert_eq!(
            forward[0].kinds,
            ["register-write-semantics", "unresolved-call"]
        );
        assert_eq!(forward[0].score_explanation.estimated_cost_units, 6);
        assert_eq!(forward[0].confidence, "low-until-hil");
    }
}
