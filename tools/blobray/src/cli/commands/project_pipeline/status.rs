//! CLI presentation for the application-owned project analysis report.

use crate::application::project_analysis::{ProjectAnalysisReport, ProjectAnalysisStatus};

pub(super) fn render(document: &ProjectAnalysisReport) {
    crate::cli::output::render_report(document, || print_human(document));
}

fn print_human(document: &ProjectAnalysisReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project analysis"));
    outputln!("Mode: {}", document.mode);
    let outcome = match document.status {
        ProjectAnalysisStatus::Complete => output::success(format!(
            "READY — {} written, {} verified, {} up to date",
            document.written, document.verified, document.current
        )),
        ProjectAnalysisStatus::NothingConfigured => {
            output::warning("NOTHING CONFIGURED — no project analysis stage ran")
        }
        ProjectAnalysisStatus::Failed => output::failure(format!(
            "BLOCKED — {} failed, {} blocked",
            document.failed, document.blocked
        )),
    };
    outputln!("\n{outcome}");

    let problems = document
        .stages
        .iter()
        .filter(|stage| matches!(stage.status, "failed" | "blocked"))
        .collect::<Vec<_>>();
    if !problems.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        for (index, stage) in problems.iter().enumerate() {
            outputln!(
                "{}. {}: {}",
                index + 1,
                stage.name,
                stage.reason.as_deref().unwrap_or(stage.status)
            );
        }
    }

    outputln!("\n{}", output::heading("Stages"));
    outputln!(
        "{}",
        table::render(
            ["Stage", "Status", "Details"],
            document.stages.iter().map(|stage| [
                stage.name.clone(),
                stage.status.to_owned(),
                stage.reason.clone().unwrap_or_default(),
            ])
        )
    );
    let missing_function_pack = document.stages.iter().any(|stage| {
        stage
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("cannot read function pack"))
    });
    let missing_interface_pack = document.stages.iter().any(|stage| {
        stage
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("cannot read interface pack"))
    });
    if missing_function_pack || missing_interface_pack {
        outputln!("\n{}", output::heading("Next"));
        if missing_function_pack {
            outputln!("- blobray advanced functions init-pack");
        }
        if missing_interface_pack {
            outputln!("- blobray advanced interfaces init-pack");
        }
        outputln!("Then rerun `blobray project analyze`.");
    } else if document.status == ProjectAnalysisStatus::NothingConfigured {
        outputln!("\n{}", output::heading("Next"));
        outputln!(
            "Configure at least one analysis, validation, or review stage, then rerun `blobray project analyze`."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::pipeline::StageReport;

    #[test]
    fn analysis_document_keeps_stage_states_and_counts_typed() {
        let document = ProjectAnalysisReport {
            schema: 3,
            command: "project analyze",
            mode: "check",
            status: ProjectAnalysisStatus::Failed,
            stages: vec![StageReport {
                name: "linked-ir".to_owned(),
                status: "blocked",
                reason: Some("missing input".to_owned()),
            }],
            written: 0,
            verified: 0,
            current: 0,
            failed: 0,
            blocked: 1,
            not_configured: 0,
        };
        let value = serde_json::to_value(document).unwrap();
        assert_eq!(value["blocked"], 1);
        assert_eq!(value["stages"][0]["status"], "blocked");
    }
}
