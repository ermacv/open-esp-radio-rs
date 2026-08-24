//! CLI presentation for the read-only project analysis plan.

use crate::application::project_analysis::{ProjectAnalysisPlanReport, ProjectAnalysisStatus};

pub(super) fn render(document: &ProjectAnalysisPlanReport) {
    crate::cli::output::render_report(document, || print_human(document));
}

fn print_human(document: &ProjectAnalysisPlanReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project analysis plan"));
    outputln!("Mode: {}", document.mode);
    outputln!("Read only: yes");
    let outcome = match document.status {
        ProjectAnalysisStatus::Complete => output::success(format!(
            "PLAN READY — {} current, {} restore, {} compute, {} verify, {} deferred",
            document.current,
            document.restored,
            document.computed,
            document.verified,
            document.deferred
        )),
        ProjectAnalysisStatus::NothingConfigured => {
            output::warning("NOTHING CONFIGURED — every analysis stage would be skipped")
        }
        ProjectAnalysisStatus::Failed => {
            output::failure("PLAN BLOCKED — one or more configured stages cannot be planned")
        }
    };
    outputln!("\n{outcome}");
    outputln!("No stage was executed and no generated output was changed.");
    outputln!(
        "Execution can still fail when a stage performs its full structural or semantic validation."
    );

    outputln!("\n{}", output::heading("Stages"));
    outputln!(
        "{}",
        table::render(
            ["#", "Stage", "Action", "After"],
            document.stages.iter().map(|stage| {
                [
                    stage.order.to_string(),
                    stage.name.clone(),
                    stage.action.label().to_owned(),
                    dependency_summary(stage),
                ]
            })
        )
    );
    let decisions = document
        .stages
        .iter()
        .filter_map(|stage| {
            stage
                .cause
                .as_ref()
                .map(|cause| [stage.name.clone(), cause.clone()])
        })
        .collect::<Vec<_>>();
    if !decisions.is_empty() {
        outputln!("\n{}", output::heading("Decisions"));
        outputln!("{}", table::render(["Stage", "Reason"], decisions));
    }

    if !output::details() {
        return;
    }

    outputln!("\n{}", output::heading("Work items"));
    let rows = work_item_rows(document);
    if rows.is_empty() {
        outputln!("No stage work items could be planned.");
        return;
    }
    outputln!(
        "{}",
        table::render(
            ["Stage", "Name", "Action", "Signature", "Outputs", "Cause"],
            rows,
        )
    );

    let awaiting = awaiting_input_rows(document);
    if !awaiting.is_empty() {
        outputln!("\n{}", output::heading("Awaiting generated inputs"));
        outputln!(
            "{}",
            table::render(["Stage", "Work item", "Producer", "Input"], awaiting,)
        );
    }
}

fn dependency_summary(
    stage: &crate::application::project_analysis::ProjectAnalysisPlanStage,
) -> String {
    let mut parts = Vec::new();
    if !stage.dependencies.is_empty() {
        parts.push(stage.dependencies.join(", "));
    }
    if !stage.optional_dependencies.is_empty() {
        parts.push(format!(
            "optional: {}",
            stage.optional_dependencies.join(", ")
        ));
    }
    parts.join("; ")
}

fn work_item_rows(document: &ProjectAnalysisPlanReport) -> Vec<[String; 6]> {
    document
        .stages
        .iter()
        .flat_map(|stage| {
            stage.work_items.iter().map(|item| {
                [
                    stage.name.clone(),
                    item.name.clone(),
                    item.action.label().to_owned(),
                    item.signature.clone().unwrap_or_else(|| "none".to_owned()),
                    if item.outputs.is_empty() {
                        "none".to_owned()
                    } else {
                        item.outputs
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    },
                    item.cause.clone().unwrap_or_else(|| "none".to_owned()),
                ]
            })
        })
        .collect()
}

fn awaiting_input_rows(document: &ProjectAnalysisPlanReport) -> Vec<[String; 4]> {
    document
        .stages
        .iter()
        .flat_map(|stage| {
            stage.work_items.iter().flat_map(|item| {
                item.awaiting_inputs.iter().map(|input| {
                    [
                        stage.name.clone(),
                        item.name.clone(),
                        input.producer_stage.clone(),
                        input.path.display().to_string(),
                    ]
                })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::project_analysis::{
        ProjectAnalysisPlanAction, ProjectAnalysisPlanStage, ProjectAnalysisPlanWorkItem,
    };
    use std::path::PathBuf;

    #[test]
    fn plan_document_keeps_order_dependencies_and_actions_typed() {
        let document = ProjectAnalysisPlanReport {
            schema: 2,
            command: "project analyze --plan",
            mode: "write",
            read_only: true,
            status: ProjectAnalysisStatus::Complete,
            stages: vec![ProjectAnalysisPlanStage {
                order: 1,
                name: "linked-ir".to_owned(),
                dependencies: vec!["interface-discovery".to_owned()],
                optional_dependencies: Vec::new(),
                action: ProjectAnalysisPlanAction::Deferred,
                cause: Some("input will change".to_owned()),
                work_items: Vec::new(),
            }],
            current: 0,
            restored: 0,
            computed: 0,
            verified: 0,
            deferred: 1,
            blocked: 0,
            failed: 0,
            not_configured: 0,
            skipped: 0,
        };
        let value = serde_json::to_value(document).unwrap();
        assert_eq!(value["read_only"], true);
        assert_eq!(value["stages"][0]["action"], "deferred");
        assert_eq!(
            value["stages"][0]["depends-on"],
            serde_json::json!(["interface-discovery"])
        );
    }

    #[test]
    fn detail_rows_keep_every_profile_and_all_work_item_fields() {
        let document = ProjectAnalysisPlanReport {
            schema: 2,
            command: "project analyze --plan",
            mode: "write",
            read_only: true,
            status: ProjectAnalysisStatus::Complete,
            stages: vec![ProjectAnalysisPlanStage {
                order: 1,
                name: "linked-ir".to_owned(),
                dependencies: Vec::new(),
                optional_dependencies: Vec::new(),
                action: ProjectAnalysisPlanAction::Failed,
                cause: Some("one profile failed".to_owned()),
                work_items: vec![
                    ProjectAnalysisPlanWorkItem {
                        name: "linked-ir:release".to_owned(),
                        action: ProjectAnalysisPlanAction::Current,
                        signature: Some("sha256:release".to_owned()),
                        outputs: vec![PathBuf::from("generated/release/linked-ir.json")],
                        cause: None,
                        awaiting_inputs: Vec::new(),
                    },
                    ProjectAnalysisPlanWorkItem {
                        name: "linked-ir:debug".to_owned(),
                        action: ProjectAnalysisPlanAction::Failed,
                        signature: None,
                        outputs: vec![
                            PathBuf::from("generated/debug/linked-ir.json"),
                            PathBuf::from("generated/debug/calls.json"),
                        ],
                        cause: Some("profile input is invalid".to_owned()),
                        awaiting_inputs: Vec::new(),
                    },
                ],
            }],
            current: 0,
            restored: 0,
            computed: 0,
            verified: 0,
            deferred: 0,
            blocked: 0,
            failed: 1,
            not_configured: 0,
            skipped: 0,
        };

        let rows = work_item_rows(&document);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            [
                "linked-ir",
                "linked-ir:release",
                "current",
                "sha256:release",
                "generated/release/linked-ir.json",
                "none",
            ]
            .map(str::to_owned)
        );
        assert_eq!(rows[1][0], "linked-ir");
        assert_eq!(rows[1][1], "linked-ir:debug");
        assert_eq!(rows[1][2], "failed");
        assert_eq!(rows[1][3], "none");
        assert_eq!(
            rows[1][4],
            "generated/debug/linked-ir.json\ngenerated/debug/calls.json"
        );
        assert_eq!(rows[1][5], "profile input is invalid");
    }
}
