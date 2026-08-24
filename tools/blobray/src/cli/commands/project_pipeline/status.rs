//! CLI presentation for the application-owned project analysis report.

use crate::application::{
    FollowUpRequirements, ProjectContext,
    project_analysis::{ProjectAnalysisReport, ProjectAnalysisStatus},
};

use super::human_duration;

pub(super) fn render(document: &ProjectAnalysisReport, context: ProjectContext<'_>) {
    crate::cli::output::render_report(document, || print_human(document, &context));
}

fn print_human(document: &ProjectAnalysisReport, context: &ProjectContext<'_>) {
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
    let next = next_lines(document, context);
    if !next.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for line in next {
            outputln!("{line}");
        }
    }
}

fn next_lines(document: &ProjectAnalysisReport, context: &ProjectContext<'_>) -> Vec<String> {
    let missing_pack = |kind: &str| {
        document.stages.iter().any(|stage| {
            stage
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains(kind))
        })
    };
    let missing_function_pack = missing_pack("cannot read function pack");
    let missing_interface_pack = missing_pack("cannot read interface pack");
    let analyze = || context.follow_up_command("project analyze", FollowUpRequirements::ANALYSIS);
    if missing_function_pack || missing_interface_pack {
        let mut lines = Vec::new();
        if missing_function_pack {
            lines.push(format!(
                "- {}",
                context.follow_up_command(
                    "advanced functions init-pack",
                    FollowUpRequirements::TARGET,
                )
            ));
        }
        if missing_interface_pack {
            lines.push(format!(
                "- {}",
                context.follow_up_command(
                    "advanced interfaces init-pack",
                    FollowUpRequirements::TARGET,
                )
            ));
        }
        lines.push(format!("Then rerun `{}`.", analyze()));
        lines
    } else if document.status == ProjectAnalysisStatus::NothingConfigured {
        vec![format!(
            "Configure at least one analysis, validation, or review stage, then rerun `{}`.",
            analyze()
        )]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MmioMap, TargetSpec,
        application::{ExplicitProjectContext, pipeline::StageReport},
        project::ProjectSpec,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn analysis_document_keeps_stage_states_and_counts_typed() {
        let document = ProjectAnalysisReport {
            schema: 5,
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
        };
        let document = ProjectAnalysisReport {
            schema: 5,
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
        };
        let arg = |path: &Path| crate::shell::arg(path.as_os_str());

        assert_eq!(
            next_lines(&document, &context),
            [
                format!(
                    "- blobray advanced functions init-pack --project {} --target-spec {}",
                    arg(&manifest),
                    arg(&explicit_target),
                ),
                format!(
                    "Then rerun `blobray project analyze --project {} --target-spec {} --run-spec {} --svd {} --svd {}`.",
                    arg(&manifest),
                    arg(&explicit_target),
                    arg(&explicit_run),
                    arg(&explicit_svds[0]),
                    arg(&explicit_svds[1]),
                ),
            ]
        );
    }
}
