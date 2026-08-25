//! CLI presentation for the application-owned project analysis report.

use crate::application::project_analysis::{ProjectAnalysisReport, ProjectAnalysisStatus};

use super::human_duration;

pub(super) fn render(document: &ProjectAnalysisReport) {
    crate::cli::output::render_report(document, || print_human(document));
}

fn print_human(document: &ProjectAnalysisReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project analysis"));
    outputln!("Mode: {}", document.mode);
    let outcome = match document.status {
        ProjectAnalysisStatus::Complete => output::success(format!(
            "READY — {} written, {} restored, {} verified, {} up to date",
            document.written, document.restored, document.verified, document.current
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
    outputln!("Duration: {}", human_duration(document.duration_ms));

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
            ["Stage", "Status", "Duration", "Details"],
            document.stages.iter().map(|stage| {
                [
                    stage.name.clone(),
                    stage.status.to_owned(),
                    human_duration(stage.duration_ms),
                    stage.reason.clone().unwrap_or_default(),
                ]
            })
        )
    );
    if !document.next_steps.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, step) in document.next_steps.iter().enumerate() {
            outputln!("{}. {}", index + 1, step.instruction);
            for command in &step.commands {
                outputln!("   {}", command.render_posix());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MmioMap, TargetSpec,
        application::{ExplicitProjectContext, ProjectContext, pipeline::StageReport},
        project::ProjectSpec,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn analysis_document_keeps_stage_states_and_counts_typed() {
        let document = ProjectAnalysisReport {
            schema: 6,
            command: "project analyze",
            mode: "check",
            status: ProjectAnalysisStatus::Failed,
            stages: vec![StageReport {
                name: "linked-ir".to_owned(),
                status: "blocked",
                duration_ms: None,
                reason: Some("missing input".to_owned()),
            }],
            written: 0,
            restored: 0,
            verified: 0,
            current: 0,
            failed: 0,
            blocked: 1,
            not_configured: 0,
            duration_ms: None,
            next_steps: Vec::new(),
        };
        let value = serde_json::to_value(document).unwrap();
        assert_eq!(value["blocked"], 1);
        assert_eq!(value["stages"][0]["status"], "blocked");
        assert!(value["stages"][0].get("duration_ms").is_none());
        assert!(value.get("duration_ms").is_none());
    }

    #[test]
    fn duration_rendering_distinguishes_fast_and_unmeasured_stages() {
        assert_eq!(human_duration(Some(0)), "<1 ms");
        assert_eq!(human_duration(Some(12)), "12 ms");
        assert_eq!(human_duration(None), "not measured");
    }

    #[test]
    fn analysis_follow_ups_quote_context_and_keep_repeated_register_catalogs() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/vendor-project.toml");
        let project = ProjectSpec::load(&fixture).unwrap();
        let target = TargetSpec::load(&project.target_spec).unwrap();
        let svd = MmioMap::load_all(&[]).unwrap();
        let manifest = PathBuf::from("/tmp/vendor  owner's project/vendor project.toml");
        let explicit_target = PathBuf::from("/tmp/target owner's.toml");
        let explicit_run = PathBuf::from("/tmp/run spec.toml");
        let explicit_svds = vec![
            PathBuf::from("/tmp/registers one.svd"),
            PathBuf::from("/tmp/register owner's two.svd"),
        ];
        let explicit_context = ExplicitProjectContext {
            target_spec: Some(explicit_target.clone()),
            run_spec: Some(explicit_run.clone()),
            svd_paths: explicit_svds.clone(),
        };
        let context = ProjectContext {
            project_path: &manifest,
            project: &project,
            target_path: &project.target_spec,
            target: &target,
            run_spec_path: None,
            run_spec: None,
            memory_map: None,
            svd_paths: &[],
            svd: &svd,
            explicit_context: &explicit_context,
            invocation_directory: Path::new("/tmp"),
        };
        let document = ProjectAnalysisReport {
            schema: 6,
            command: "project analyze",
            mode: "write",
            status: ProjectAnalysisStatus::Failed,
            stages: vec![StageReport {
                name: "function-validation".to_owned(),
                status: "failed",
                duration_ms: None,
                reason: Some("cannot read function pack from configured path".to_owned()),
            }],
            written: 0,
            restored: 0,
            verified: 0,
            current: 0,
            failed: 1,
            blocked: 0,
            not_configured: 0,
            duration_ms: None,
            next_steps: Vec::new(),
        };
        let steps = crate::application::project_analysis::follow_up_steps(&document, &context);
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].instruction,
            "Create the missing reviewed packs, then rerun project analysis."
        );
        assert_eq!(steps[0].commands.len(), 2);
        assert_eq!(
            steps[0].commands[0].argv,
            [
                "blobray",
                "advanced",
                "functions",
                "init-pack",
                "--project",
                manifest.to_str().unwrap(),
                "--target-spec",
                explicit_target.to_str().unwrap(),
            ]
        );
        assert_eq!(
            steps[0].commands[1].argv,
            [
                "blobray",
                "project",
                "analyze",
                "--project",
                manifest.to_str().unwrap(),
                "--target-spec",
                explicit_target.to_str().unwrap(),
                "--run-spec",
                explicit_run.to_str().unwrap(),
                "--svd",
                explicit_svds[0].to_str().unwrap(),
                "--svd",
                explicit_svds[1].to_str().unwrap(),
            ]
        );
    }
}
