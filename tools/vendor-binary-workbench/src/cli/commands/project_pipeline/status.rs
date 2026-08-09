//! CLI presentation for the application-owned project analysis report.

use crate::application::project_analysis::ProjectAnalysisReport;

pub(super) fn render(document: &ProjectAnalysisReport) {
    crate::cli::output::render_report(document, || print_human(document));
}

fn print_human(document: &ProjectAnalysisReport) {
    outputln!("Project analysis: {} ({})", document.status, document.mode);
    for stage in &document.stages {
        outputln!(
            "  {:<24} {:<14} {}",
            stage.name,
            stage.status,
            stage.reason.as_deref().unwrap_or("")
        );
    }
    outputln!(
        "  written={} verified={} failed={} blocked={} not-configured={}",
        document.written,
        document.verified,
        document.failed,
        document.blocked,
        document.not_configured
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
        outputln!("Next review workspace setup:");
        if missing_function_pack {
            outputln!("  vendor-binary-workbench functions init-pack");
        }
        if missing_interface_pack {
            outputln!("  vendor-binary-workbench interfaces init-pack");
        }
        outputln!("Then rerun `vendor-binary-workbench project analyze`.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::pipeline::StageReport;

    #[test]
    fn analysis_document_keeps_stage_states_and_counts_typed() {
        let document = ProjectAnalysisReport {
            schema: 1,
            command: "project analyze",
            mode: "check",
            status: "failed",
            stages: vec![StageReport {
                name: "linked-ir".to_owned(),
                status: "blocked",
                reason: Some("missing input".to_owned()),
            }],
            written: 0,
            verified: 0,
            failed: 0,
            blocked: 1,
            not_configured: 0,
        };
        let value = serde_json::to_value(document).unwrap();
        assert_eq!(value["blocked"], 1);
        assert_eq!(value["stages"][0]["status"], "blocked");
    }
}
