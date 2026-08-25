//! Stable project-status model shared by application frontends.

use serde::Serialize;
use std::collections::BTreeMap;

use crate::application::ExecutableAction;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LinkedIrProfileDetail {
    pub id: String,
    pub sources: Vec<String>,
    pub missing_sources: Vec<String>,
    pub entry_contract: String,
    pub contract_status: &'static str,
    pub contract_error: Option<String>,
    pub output: String,
    pub output_status: &'static str,
    pub output_error: Option<String>,
    pub functions: usize,
    pub registers: usize,
    pub field_candidates: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AnalysisSurfaceDetail {
    pub id: String,
    pub protocols: Vec<String>,
    pub kind: String,
    pub status: String,
    pub profile: Option<String>,
    pub sources: Vec<String>,
    pub missing_sources: Vec<String>,
    pub output: Option<String>,
    pub symbol_prefix: Option<String>,
    pub matched_symbols: Vec<String>,
    pub reason: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MmioRegionDetail {
    pub name: String,
    pub address_space: String,
    pub start: u64,
    pub end_exclusive: u64,
    pub permissions: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArtifactDetail {
    pub role: String,
    pub status: &'static str,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objects: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_members: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_facts: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewScopeDetail {
    pub id: String,
    pub protocols: Vec<String>,
    pub publication: bool,
    pub replacement_coverage: String,
    pub analysis_inventory_complete: bool,
    pub profiles: Vec<String>,
    pub roots: usize,
    pub functions: usize,
    pub replacement_functions: usize,
    pub complete_functions: usize,
    pub mmio_registers: usize,
    pub linked_mmio_registers: usize,
    pub static_mmio_registers: usize,
    pub table_calls: usize,
    pub context_fields: usize,
    pub memory_fields: usize,
    pub decode_blockers: usize,
    pub decode_blocker_functions: usize,
    pub direct_blockers: usize,
    pub call_graph_blockers: usize,
    pub reference_blockers: usize,
    pub unresolved_calls: usize,
    pub replacement_behavioral_matches: usize,
    pub replacement_production_matches: usize,
    pub replacement_policy_excluded: usize,
    pub replacement_bounded_matches: usize,
    pub replacement_probe_only_matches: usize,
    pub replacement_unmapped_matches: usize,
    pub replacement_mismatches: usize,
    pub replacement_incomplete: usize,
    pub replacement_unqualified: usize,
    pub replacement_uncovered: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DetailValue {
    String(String),
    Unsigned(u64),
    Bool(bool),
    Strings(Vec<String>),
    LinkedIrProfiles(Vec<LinkedIrProfileDetail>),
    MmioRegions(Vec<MmioRegionDetail>),
    Artifacts(Vec<ArtifactDetail>),
    ReviewScopes(Vec<ReviewScopeDetail>),
    AnalysisSurfaces(Vec<AnalysisSurfaceDetail>),
}

impl From<String> for DetailValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for DetailValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<usize> for DetailValue {
    fn from(value: usize) -> Self {
        Self::Unsigned(value as u64)
    }
}

impl From<u64> for DetailValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for DetailValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<Vec<String>> for DetailValue {
    fn from(value: Vec<String>) -> Self {
        Self::Strings(value)
    }
}

impl From<Vec<LinkedIrProfileDetail>> for DetailValue {
    fn from(value: Vec<LinkedIrProfileDetail>) -> Self {
        Self::LinkedIrProfiles(value)
    }
}

impl From<Vec<MmioRegionDetail>> for DetailValue {
    fn from(value: Vec<MmioRegionDetail>) -> Self {
        Self::MmioRegions(value)
    }
}

impl From<Vec<ArtifactDetail>> for DetailValue {
    fn from(value: Vec<ArtifactDetail>) -> Self {
        Self::Artifacts(value)
    }
}

impl From<Vec<ReviewScopeDetail>> for DetailValue {
    fn from(value: Vec<ReviewScopeDetail>) -> Self {
        Self::ReviewScopes(value)
    }
}

impl From<Vec<AnalysisSurfaceDetail>> for DetailValue {
    fn from(value: Vec<AnalysisSurfaceDetail>) -> Self {
        Self::AnalysisSurfaces(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    Ready,
    /// Valid non-gating artifact-wide evidence remains available for review.
    Inventory,
    Incomplete,
    NotConfigured,
    Invalid,
}

impl Readiness {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Inventory => "inventory",
            Self::Incomplete => "incomplete",
            Self::NotConfigured => "not-configured",
            Self::Invalid => "invalid",
        }
    }
}

/// Validation contract for the lightweight project-status projection.
///
/// Status collection intentionally avoids the reproducibility work performed by
/// `project doctor` and `project check`. Keeping that boundary in the shared
/// model prevents frontends from presenting file presence as validated
/// freshness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationDepth {
    Shallow,
    Deep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceFreshness {
    Unknown,
    Current,
    Stale,
}

/// Research progress is deliberately separate from artifact readiness.
///
/// A review report can be present and structurally valid while still exposing
/// hundreds of unresolved root causes. Calling that state merely `ready`
/// makes the normal project-status projection easy to misread.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResearchCompleteness {
    Complete,
    Open,
    Unknown,
    NotConfigured,
    Invalid,
}

impl ResearchCompleteness {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Open => "open",
            Self::Unknown => "unknown",
            Self::NotConfigured => "not-configured",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResearchProgress {
    pub status: ResearchCompleteness,
    pub scopes: usize,
    pub inventory_complete: usize,
    pub inventory_open: usize,
    pub root_causes: usize,
    pub publication_coverage_gaps: usize,
}

impl ResearchProgress {
    fn collect(phases: &[Phase]) -> Self {
        let Some(component) =
            phases
                .iter()
                .find(|phase| phase.name == "review")
                .and_then(|phase| {
                    phase
                        .components
                        .iter()
                        .find(|component| component.name == "scopes")
                })
        else {
            return Self::empty(ResearchCompleteness::NotConfigured);
        };
        if component.status == Readiness::Invalid {
            return Self::empty(ResearchCompleteness::Invalid);
        }
        if component.status == Readiness::NotConfigured {
            return Self::empty(ResearchCompleteness::NotConfigured);
        }
        let Some(scopes) = unsigned_detail(component, "count") else {
            return Self::empty(ResearchCompleteness::Unknown);
        };
        let Some(inventory_open) = unsigned_detail(component, "analysis_inventory_blocked") else {
            return Self::empty(ResearchCompleteness::Unknown);
        };
        let Some(root_causes) = unsigned_detail(component, "research_root_causes") else {
            return Self::empty(ResearchCompleteness::Unknown);
        };
        let Some(publication_coverage_gaps) =
            unsigned_detail(component, "replacement_coverage_gaps")
        else {
            return Self::empty(ResearchCompleteness::Unknown);
        };
        let status = if inventory_open == 0 && root_causes == 0 && publication_coverage_gaps == 0 {
            ResearchCompleteness::Complete
        } else {
            ResearchCompleteness::Open
        };
        Self {
            status,
            scopes,
            inventory_complete: scopes.saturating_sub(inventory_open),
            inventory_open,
            root_causes,
            publication_coverage_gaps,
        }
    }

    const fn empty(status: ResearchCompleteness) -> Self {
        Self {
            status,
            scopes: 0,
            inventory_complete: 0,
            inventory_open: 0,
            root_causes: 0,
            publication_coverage_gaps: 0,
        }
    }
}

fn unsigned_detail(component: &Component, key: &str) -> Option<usize> {
    match component.details.get(key) {
        Some(DetailValue::Unsigned(value)) => usize::try_from(*value).ok(),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct StatusValidation {
    pub depth: ValidationDepth,
    pub freshness: EvidenceFreshness,
}

/// One explicit human instruction and the exact commands that implement it.
///
/// Manual work has an empty `commands` list. Frontends may render commands for
/// copy/paste, but automation executes their argument vectors directly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FollowUpStep {
    pub instruction: String,
    pub commands: Vec<ExecutableAction>,
}

impl FollowUpStep {
    pub fn manual(instruction: impl Into<String>) -> Self {
        Self {
            instruction: instruction.into(),
            commands: Vec::new(),
        }
    }

    pub fn command(instruction: impl Into<String>, command: ExecutableAction) -> Self {
        Self::commands(instruction, vec![command])
    }

    pub fn commands(instruction: impl Into<String>, commands: Vec<ExecutableAction>) -> Self {
        Self {
            instruction: instruction.into(),
            commands,
        }
    }
}

impl StatusValidation {
    const SHALLOW: Self = Self {
        depth: ValidationDepth::Shallow,
        freshness: EvidenceFreshness::Unknown,
    };
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Component {
    pub name: String,
    pub status: Readiness,
    pub details: BTreeMap<String, DetailValue>,
    pub diagnostic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<FollowUpStep>,
}

impl Component {
    pub(crate) fn new(name: &'static str, status: Readiness) -> Self {
        Self {
            name: name.to_owned(),
            status,
            details: BTreeMap::new(),
            diagnostic: None,
            next_step: None,
        }
    }

    pub(crate) fn detail(mut self, key: &str, value: impl Into<DetailValue>) -> Self {
        self.details.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn diagnostic(mut self, value: impl ToString) -> Self {
        self.diagnostic = Some(value.to_string());
        self
    }

    pub(crate) fn next_step(mut self, value: FollowUpStep) -> Self {
        self.next_step = Some(value);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Phase {
    pub name: String,
    pub status: Readiness,
    pub components: Vec<Component>,
}

impl Phase {
    pub(crate) fn collect(name: &'static str, components: Vec<Component>) -> Self {
        let status = if components
            .iter()
            .any(|component| component.status == Readiness::Invalid)
        {
            Readiness::Invalid
        } else if components
            .iter()
            .any(|component| component.status == Readiness::Incomplete)
        {
            Readiness::Incomplete
        } else if components
            .iter()
            .any(|component| matches!(component.status, Readiness::Ready | Readiness::Inventory))
        {
            Readiness::Ready
        } else {
            Readiness::NotConfigured
        };
        Self {
            name: name.to_owned(),
            status,
            components,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TargetIdentity {
    pub id: String,
    pub architecture: String,
    #[serde(rename = "calling_convention")]
    pub calling_convention: String,
    pub knowledge_provider: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectStatusReport {
    pub project_id: String,
    pub manifest: String,
    pub target: TargetIdentity,
    pub validation: StatusValidation,
    pub research: ResearchProgress,
    pub verification: Readiness,
    pub overall: Readiness,
    pub phases: Vec<Phase>,
}

impl ProjectStatusReport {
    pub(crate) fn new(
        project_id: String,
        manifest: String,
        target: TargetIdentity,
        phases: Vec<Phase>,
    ) -> Self {
        let research = ResearchProgress::collect(&phases);
        let verification = phases
            .iter()
            .find(|phase| phase.name == "verification")
            .map_or(Readiness::NotConfigured, |phase| phase.status);
        let overall = if phases
            .iter()
            .any(|phase| phase.status == Readiness::Invalid)
        {
            Readiness::Invalid
        } else if phases
            .iter()
            .all(|phase| matches!(phase.status, Readiness::Ready | Readiness::NotConfigured))
        {
            Readiness::Ready
        } else {
            Readiness::Incomplete
        };
        Self {
            project_id,
            manifest,
            target,
            validation: StatusValidation::SHALLOW,
            research,
            verification,
            overall,
            phases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_contract_serializes_future_deep_and_fresh_states() {
        assert_eq!(
            serde_json::to_value(StatusValidation {
                depth: ValidationDepth::Deep,
                freshness: EvidenceFreshness::Current,
            })
            .unwrap(),
            serde_json::json!({ "depth": "deep", "freshness": "current" })
        );
        assert_eq!(
            serde_json::to_value(EvidenceFreshness::Stale).unwrap(),
            serde_json::json!("stale")
        );
    }

    #[test]
    fn component_serializes_typed_follow_up_step_without_legacy_action_key() {
        let command = ExecutableAction::new(
            vec![
                "blobray".into(),
                "project".into(),
                "verify".into(),
                "--suite".into(),
                "suite with spaces".into(),
            ],
            std::env::current_dir().unwrap(),
            crate::application::ProjectContextRequirement::Analysis,
        )
        .unwrap();
        let value = serde_json::to_value(
            Component::new("verification", Readiness::Incomplete)
                .next_step(FollowUpStep::command("replay the failing suite", command)),
        )
        .unwrap();

        assert!(value.get("next_action").is_none());
        assert_eq!(
            value["next_step"]["instruction"],
            "replay the failing suite"
        );
        assert_eq!(
            value["next_step"]["commands"][0]["argv"][4],
            "suite with spaces"
        );
        assert_eq!(value["next_step"]["commands"][0]["context"], "analysis");
    }

    #[test]
    fn manual_follow_up_step_has_no_fake_executable_command() {
        let value = serde_json::to_value(FollowUpStep::manual(
            "choose a concrete candidate evidence directory",
        ))
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "instruction": "choose a concrete candidate evidence directory",
                "commands": [],
            })
        );
    }

    #[test]
    fn optional_components_do_not_hide_incomplete_or_invalid_phases() {
        let ready = Component::new("ready", Readiness::Ready);
        let absent = Component::new("optional", Readiness::NotConfigured);
        assert_eq!(
            Phase::collect("phase", vec![ready, absent]).status,
            Readiness::Ready
        );
        assert_eq!(
            Phase::collect(
                "phase",
                vec![Component::new("missing", Readiness::Incomplete)]
            )
            .status,
            Readiness::Incomplete
        );
        assert_eq!(
            Phase::collect(
                "phase",
                vec![Component::new("full-artifact", Readiness::Inventory)]
            )
            .status,
            Readiness::Ready
        );
    }

    #[test]
    fn research_progress_is_independent_from_ready_review_artifacts() {
        let report = ProjectStatusReport::new(
            "fixture".to_owned(),
            "vendor-project.toml".to_owned(),
            TargetIdentity {
                id: "target".to_owned(),
                architecture: "riscv32".to_owned(),
                calling_convention: "riscv-ilp32".to_owned(),
                knowledge_provider: None,
            },
            vec![
                Phase::collect(
                    "review",
                    vec![
                        Component::new("scopes", Readiness::Ready)
                            .detail("count", 7usize)
                            .detail("analysis_inventory_blocked", 3usize)
                            .detail("research_root_causes", 11usize)
                            .detail("replacement_coverage_gaps", 1usize),
                    ],
                ),
                Phase::collect(
                    "verification",
                    vec![Component::new("last-verification", Readiness::Incomplete)],
                ),
            ],
        );
        assert_eq!(report.research.status, ResearchCompleteness::Open);
        assert_eq!(report.research.scopes, 7);
        assert_eq!(report.research.inventory_complete, 4);
        assert_eq!(report.research.inventory_open, 3);
        assert_eq!(report.research.root_causes, 11);
        assert_eq!(report.research.publication_coverage_gaps, 1);
        assert_eq!(report.verification, Readiness::Incomplete);
    }
}
