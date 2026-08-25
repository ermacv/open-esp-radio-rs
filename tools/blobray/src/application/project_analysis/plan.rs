//! Typed, frontend-neutral project analysis dry-run model.

use std::{collections::BTreeMap, path::PathBuf};

use serde::Serialize;

use super::{ProjectAnalysisInputs, ProjectAnalysisReport, ProjectAnalysisStatus};
use crate::project::ProjectSpec;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectAnalysisPlanAction {
    Current,
    Restore,
    Compute,
    Verify,
    Deferred,
    Blocked,
    Failed,
    NotConfigured,
    Skip,
}

impl ProjectAnalysisPlanAction {
    pub(super) const fn materializes_inputs(self) -> bool {
        matches!(self, Self::Restore | Self::Compute | Self::Deferred)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Restore => "restore",
            Self::Compute => "compute",
            Self::Verify => "verify",
            Self::Deferred => "deferred",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::NotConfigured => "not-configured",
            Self::Skip => "skip",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAnalysisPlanWorkItem {
    pub name: String,
    pub action: ProjectAnalysisPlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub outputs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(rename = "awaiting-inputs", skip_serializing_if = "Vec::is_empty")]
    pub awaiting_inputs: Vec<ProjectAnalysisPlanAwaitingInput>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAnalysisPlanAwaitingInput {
    pub path: PathBuf,
    #[serde(rename = "producer-stage")]
    pub producer_stage: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAnalysisPlanStage {
    pub order: usize,
    pub name: String,
    #[serde(rename = "depends-on")]
    pub dependencies: Vec<String>,
    #[serde(rename = "optional-depends-on", skip_serializing_if = "Vec::is_empty")]
    pub optional_dependencies: Vec<String>,
    pub action: ProjectAnalysisPlanAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(rename = "work-items", skip_serializing_if = "Vec::is_empty")]
    pub work_items: Vec<ProjectAnalysisPlanWorkItem>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAnalysisPlanReport {
    pub schema: u32,
    pub command: &'static str,
    pub mode: &'static str,
    pub read_only: bool,
    pub status: ProjectAnalysisStatus,
    pub stages: Vec<ProjectAnalysisPlanStage>,
    pub current: usize,
    pub restored: usize,
    pub computed: usize,
    pub verified: usize,
    pub deferred: usize,
    pub blocked: usize,
    pub failed: usize,
    #[serde(rename = "not-configured")]
    pub not_configured: usize,
    pub skipped: usize,
}

impl ProjectAnalysisPlanReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, ProjectAnalysisStatus::Complete)
    }
}

#[derive(Default)]
pub(super) struct ProjectAnalysisPlanner {
    stages: BTreeMap<String, Vec<ProjectAnalysisPlanWorkItem>>,
}

impl ProjectAnalysisPlanner {
    pub(super) fn input_materialization(
        &self,
        input: &std::path::Path,
    ) -> Option<(&str, &std::path::Path)> {
        self.stages.iter().find_map(|(stage, items)| {
            items.iter().find_map(|item| {
                item.action
                    .materializes_inputs()
                    .then(|| {
                        item.outputs.iter().find(|output| {
                            input == output.as_path()
                                || input.starts_with(output)
                                || output.starts_with(input)
                        })
                    })
                    .flatten()
                    .map(|output| (stage.as_str(), output.as_path()))
            })
        })
    }

    pub(super) fn record(&mut self, stage: &str, item: ProjectAnalysisPlanWorkItem) {
        self.stages.entry(stage.to_owned()).or_default().push(item);
    }

    pub(super) fn finish(
        mut self,
        project: &ProjectSpec,
        inputs: ProjectAnalysisInputs,
        execution: ProjectAnalysisReport,
    ) -> ProjectAnalysisPlanReport {
        let mut stages = Vec::with_capacity(execution.stages.len());
        for (index, stage) in execution.stages.into_iter().enumerate() {
            let dependencies = stage_dependencies(project, inputs, &stage.name);
            let optional_dependencies = stage_optional_dependencies(project, &stage.name);
            let work_items = self.stages.remove(&stage.name).unwrap_or_default();
            let terminal = action_for_stage_status(stage.status);
            let (action, cause) = if let Some(action) = terminal {
                (
                    action,
                    stage.reason.or_else(|| Some(stage.status.to_owned())),
                )
            } else if work_items.is_empty() {
                (
                    ProjectAnalysisPlanAction::Skip,
                    stage.reason.or_else(|| Some(stage.status.to_owned())),
                )
            } else {
                let action = aggregate_actions(work_items.iter().map(|item| item.action));
                (action, aggregate_cause(&work_items))
            };
            stages.push(ProjectAnalysisPlanStage {
                order: index + 1,
                name: stage.name,
                dependencies,
                optional_dependencies,
                action,
                cause,
                work_items,
            });
        }

        let count = |action| stages.iter().filter(|stage| stage.action == action).count();
        ProjectAnalysisPlanReport {
            schema: 2,
            command: "project analyze --plan",
            mode: execution.mode,
            read_only: true,
            status: execution.status,
            current: count(ProjectAnalysisPlanAction::Current),
            restored: count(ProjectAnalysisPlanAction::Restore),
            computed: count(ProjectAnalysisPlanAction::Compute),
            verified: count(ProjectAnalysisPlanAction::Verify),
            deferred: count(ProjectAnalysisPlanAction::Deferred),
            blocked: count(ProjectAnalysisPlanAction::Blocked),
            failed: count(ProjectAnalysisPlanAction::Failed),
            not_configured: count(ProjectAnalysisPlanAction::NotConfigured),
            skipped: count(ProjectAnalysisPlanAction::Skip),
            stages,
        }
    }
}

fn aggregate_cause(work_items: &[ProjectAnalysisPlanWorkItem]) -> Option<String> {
    let pending = work_items
        .iter()
        .filter(|item| item.action != ProjectAnalysisPlanAction::Current)
        .collect::<Vec<_>>();
    if pending.len() == 1 {
        let item = pending[0];
        return Some(item.cause.as_deref().map_or_else(
            || format!("{}: {}", item.name, item.action.label()),
            |cause| format!("{}: {cause}", item.name),
        ));
    }
    if pending.is_empty() {
        return None;
    }
    let counts = [
        ProjectAnalysisPlanAction::Restore,
        ProjectAnalysisPlanAction::Compute,
        ProjectAnalysisPlanAction::Verify,
        ProjectAnalysisPlanAction::Deferred,
        ProjectAnalysisPlanAction::Blocked,
        ProjectAnalysisPlanAction::Failed,
    ]
    .into_iter()
    .filter_map(|action| {
        let count = pending.iter().filter(|item| item.action == action).count();
        (count != 0).then(|| format!("{}={count}", action.label()))
    })
    .collect::<Vec<_>>()
    .join(", ");
    Some(format!("{} work items: {counts}", pending.len()))
}

fn action_for_stage_status(status: &str) -> Option<ProjectAnalysisPlanAction> {
    match status {
        "blocked" => Some(ProjectAnalysisPlanAction::Blocked),
        "failed" => Some(ProjectAnalysisPlanAction::Failed),
        "not-configured" => Some(ProjectAnalysisPlanAction::NotConfigured),
        _ => None,
    }
}

fn aggregate_actions(
    actions: impl IntoIterator<Item = ProjectAnalysisPlanAction>,
) -> ProjectAnalysisPlanAction {
    actions
        .into_iter()
        .max_by_key(|action| match action {
            ProjectAnalysisPlanAction::Skip => 0,
            ProjectAnalysisPlanAction::Current => 1,
            ProjectAnalysisPlanAction::Verify => 2,
            ProjectAnalysisPlanAction::Restore => 3,
            ProjectAnalysisPlanAction::Compute => 4,
            ProjectAnalysisPlanAction::Deferred => 5,
            ProjectAnalysisPlanAction::NotConfigured => 6,
            ProjectAnalysisPlanAction::Blocked => 7,
            ProjectAnalysisPlanAction::Failed => 8,
        })
        .unwrap_or(ProjectAnalysisPlanAction::Skip)
}

pub(super) fn stage_dependencies(
    project: &ProjectSpec,
    inputs: ProjectAnalysisInputs,
    stage: &str,
) -> Vec<String> {
    let mut dependencies = Vec::new();
    let mut configured = |name: &str, include: bool| {
        if include {
            dependencies.push(name.to_owned());
        }
    };
    match stage {
        "mmio-discovery" | "interface-discovery" => {
            configured(
                "symbol-inventory",
                project.code.is_some() && project.symbol_inventory.is_some(),
            );
        }
        "linked-ir" => {
            configured(
                "symbol-inventory",
                project.code.is_some() && project.symbol_inventory.is_some(),
            );
            configured(
                "interface-discovery",
                super::linked_ir_uses_reviewed_interfaces(project),
            );
        }
        "event-replays" => {
            configured(
                "interface-discovery",
                inputs.event_replays_require_interfaces && project.interfaces.is_some(),
            );
        }
        "review-scopes" => {
            configured("linked-ir", !project.ir_profiles.is_empty());
            configured("mmio-discovery", project.registers.is_some());
        }
        "navigation-index" => {
            configured("symbol-inventory", project.symbol_inventory.is_some());
            configured("linked-ir", !project.ir_profiles.is_empty());
            configured("interface-discovery", project.interfaces.is_some());
        }
        "code-boundary-validation" | "code-boundary-review" => {
            configured("symbol-inventory", project.symbol_inventory.is_some());
        }
        "register-validation" => {
            configured("mmio-discovery", project.registers.is_some());
        }
        "register-review" => {
            configured("mmio-discovery", project.registers.is_some());
            configured(
                "linked-ir",
                project.registers.as_ref().is_some_and(|registers| {
                    project.ir_profiles.iter().any(|profile| {
                        registers
                            .review_ir_reports
                            .iter()
                            .any(|report| report == &profile.output)
                    })
                }),
            );
        }
        "function-validation" => {
            configured("linked-ir", !project.ir_profiles.is_empty());
        }
        "function-review" => {
            configured("linked-ir", !project.ir_profiles.is_empty());
            configured(
                "interface-discovery",
                project
                    .interfaces
                    .as_ref()
                    .and_then(|paths| paths.pack.as_deref())
                    .is_some_and(std::path::Path::is_file),
            );
        }
        "interface-validation" => {
            configured("interface-discovery", project.interfaces.is_some());
        }
        _ => {}
    }
    dependencies
}

pub(super) fn stage_optional_dependencies(project: &ProjectSpec, stage: &str) -> Vec<String> {
    match stage {
        "linked-ir" if project.code.is_none() && project.symbol_inventory.is_some() => {
            vec!["symbol-inventory".to_owned()]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectSpec {
        ProjectSpec {
            id: "fixture".to_owned(),
            target_spec: "target.toml".into(),
            ecosystem_packs: Vec::new(),
            chip_pack: None,
            analysis_provider: None,
            run_spec: None,
            memory_map: None,
            svd_paths: Vec::new(),
            reviewed_knowledge: Vec::new(),
            review_context: open_radio_vendor_review::ApplicabilityContext::default(),
            symbol_inventory: None,
            navigation_index: None,
            code: None,
            ir_profiles: Vec::new(),
            registers: None,
            interfaces: None,
            functions: None,
            review: None,
            verification: None,
        }
    }

    #[test]
    fn aggregate_reports_the_most_expensive_required_action() {
        assert_eq!(
            aggregate_actions([
                ProjectAnalysisPlanAction::Current,
                ProjectAnalysisPlanAction::Restore,
                ProjectAnalysisPlanAction::Compute,
            ]),
            ProjectAnalysisPlanAction::Compute
        );
    }

    #[test]
    fn aggregate_cause_keeps_many_profile_misses_bounded() {
        let items = (0..19)
            .map(|index| ProjectAnalysisPlanWorkItem {
                name: format!("linked-ir:profile-{index}"),
                action: ProjectAnalysisPlanAction::Compute,
                signature: Some(format!("signature-{index}")),
                outputs: Vec::new(),
                cause: Some("no cached result matches the current stage signature".to_owned()),
                awaiting_inputs: Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            aggregate_cause(&items).as_deref(),
            Some("19 work items: compute=19")
        );
    }

    #[test]
    fn failed_stage_outcome_dominates_work_items_planned_before_the_failure() {
        let mut planner = ProjectAnalysisPlanner::default();
        planner.record(
            "linked-ir",
            ProjectAnalysisPlanWorkItem {
                name: "linked-ir:first".to_owned(),
                action: ProjectAnalysisPlanAction::Current,
                signature: Some("first-signature".to_owned()),
                outputs: vec!["generated/first.ir".into()],
                cause: None,
                awaiting_inputs: Vec::new(),
            },
        );
        let report = planner.finish(
            &project(),
            ProjectAnalysisInputs::default(),
            ProjectAnalysisReport {
                schema: 5,
                command: "project analyze",
                mode: "write",
                status: ProjectAnalysisStatus::Failed,
                stages: vec![crate::application::pipeline::StageReport {
                    name: "linked-ir".to_owned(),
                    status: "failed",
                    duration_ms: None,
                    reason: Some("second profile input is unavailable".to_owned()),
                }],
                written: 0,
                restored: 0,
                verified: 0,
                current: 0,
                failed: 1,
                blocked: 0,
                not_configured: 0,
                duration_ms: None,
            },
        );

        assert_eq!(report.failed, 1);
        assert_eq!(report.current, 0);
        assert_eq!(report.stages[0].action, ProjectAnalysisPlanAction::Failed);
        assert_eq!(
            report.stages[0].cause.as_deref(),
            Some("second profile input is unavailable")
        );
        assert_eq!(report.stages[0].work_items.len(), 1);
    }
}
