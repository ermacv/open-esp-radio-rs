//! Human and machine project-file inventory.

use std::collections::BTreeMap;

use crate::{
    Result,
    application::{FollowUpStep, ProjectContext, project_files},
    cli::{output, table},
};

#[derive(serde::Serialize)]
struct ProjectFilesDocument<'a> {
    #[serde(flatten)]
    report: &'a project_files::ProjectFilesReport,
    workflow_state: project_files::ProjectFilesWorkflowState,
    required_missing: usize,
    next_steps: Vec<FollowUpStep>,
}

pub(super) fn run(context: ProjectContext<'_>) -> Result<bool> {
    let report = project_files::collect(&context)?;
    let next_steps = report.next_steps(&context)?;
    let document = ProjectFilesDocument {
        report: &report,
        workflow_state: report.workflow_state(),
        required_missing: report.required_missing(),
        next_steps,
    };
    output::render_report(&document, || render_human(&report, &document.next_steps));
    Ok(true)
}

fn render_human(report: &project_files::ProjectFilesReport, next_steps: &[FollowUpStep]) {
    outputln!("{}", output::heading("Project files"));
    outputln!("Project:  {}", report.project_id);
    outputln!("Manifest: {}", report.manifest.display());
    let outcome = match report.workflow_state() {
        project_files::ProjectFilesWorkflowState::Blocked => output::failure(format!(
            "BOOTSTRAP BLOCKED — {} required file(s) missing",
            report.required_missing()
        )),
        project_files::ProjectFilesWorkflowState::AnalysisPending => output::warning(format!(
            "READY TO ANALYZE — {} analysis output(s) pending",
            report.pending_analysis_outputs()
        )),
        project_files::ProjectFilesWorkflowState::ReviewOutputsPending => output::warning(format!(
            "ANALYSIS OUTPUTS PRESENT — {} review output(s) pending; freshness not validated",
            report.pending_review_outputs()
        )),
        project_files::ProjectFilesWorkflowState::ReviewConfigurationRequired => output::warning(
            "ANALYSIS OUTPUTS PRESENT — publication review is not configured; freshness not validated",
        ),
        project_files::ProjectFilesWorkflowState::PublicationPreflightRequired => output::warning(
            "ANALYSIS OUTPUTS PRESENT — publication preflight is required; freshness not validated",
        ),
        project_files::ProjectFilesWorkflowState::VerificationPending => output::warning(format!(
            "ANALYSIS OUTPUTS PRESENT — {} verification output(s) pending; freshness not validated",
            report.pending_verification_outputs()
        )),
        project_files::ProjectFilesWorkflowState::FilesPresent => {
            output::success("FILES PRESENT — verify current readiness with project status")
        }
    };
    outputln!("\n{outcome}");

    let mut ownership = BTreeMap::<(&str, &str), (usize, usize, usize)>::new();
    for file in &report.files {
        let counts = ownership
            .entry((file.layer.label(), file.ownership.label()))
            .or_default();
        match file.state {
            project_files::ProjectFileState::Present => counts.0 += 1,
            project_files::ProjectFileState::Missing => counts.1 += 1,
            project_files::ProjectFileState::Pending => counts.2 += 1,
        }
    }
    let attention = report
        .files
        .iter()
        .filter(|file| file.state != project_files::ProjectFileState::Present)
        .collect::<Vec<_>>();
    if !attention.is_empty() {
        outputln!("\n{}", output::heading("Needs attention"));
        for (index, file) in attention.iter().take(12).enumerate() {
            outputln!("{}. {} [{}]", index + 1, file.role, file.state.label());
            outputln!("   {}", display_path(&file.path));
        }
        if attention.len() > 12 && !output::details() {
            outputln!(
                "{} more item(s); rerun with --details for the complete map.",
                attention.len() - 12
            );
        }
    }

    if !next_steps.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, step) in next_steps.iter().enumerate() {
            outputln!("{}. {}", index + 1, step.instruction);
            for command in &step.commands {
                outputln!("   {}", command.render_posix());
            }
        }
    }

    outputln!("\n{}", output::heading("Workspace map"));
    outputln!(
        "{}",
        table::render(
            ["Layer", "Ownership", "Present", "Missing", "Pending"],
            ownership.into_iter().map(|((layer, owner), counts)| [
                layer.to_owned(),
                owner.to_owned(),
                counts.0.to_string(),
                counts.1.to_string(),
                counts.2.to_string(),
            ]),
        )
    );

    if output::details() {
        outputln!("\n{}", output::heading("All configured files"));
        outputln!(
            "{}",
            table::render(
                ["Layer", "Ownership", "Role", "State", "Path"],
                report.files.iter().map(|file| [
                    file.layer.label().to_owned(),
                    file.ownership.label().to_owned(),
                    file.role.clone(),
                    file.state.label().to_owned(),
                    file.path.display().to_string(),
                ]),
            )
        );
    }
}

fn display_path(path: &std::path::Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|directory| path.strip_prefix(directory).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MmioMap, TargetSpec,
        application::{ExplicitProjectContext, project_files::*},
        project::ProjectSpec,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn file_follow_ups_preserve_only_context_consumed_by_the_destination() {
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
        let entry = |role: &str, state, producer: Option<&str>| ProjectFileEntry {
            role: role.to_owned(),
            ownership: ProjectFileOwnership::Generated,
            layer: ProjectFileLayer::Generated,
            state,
            path: PathBuf::from(role),
            producer: producer.map(str::to_owned),
            consumers: Vec::new(),
            required: false,
            next_step: None,
        };
        let report = |files: Vec<ProjectFileEntry>| ProjectFilesReport {
            schema: 4,
            project_id: "fixture".to_owned(),
            manifest: manifest.clone(),
            present: files
                .iter()
                .filter(|file| file.state == ProjectFileState::Present)
                .count(),
            missing: 0,
            pending: files
                .iter()
                .filter(|file| file.state == ProjectFileState::Pending)
                .count(),
            files,
        };

        let analysis = report(vec![entry(
            "navigation-index",
            ProjectFileState::Pending,
            Some("project analyze"),
        )]);
        let analysis_action = &analysis.next_steps(&context).unwrap()[0].commands[0];
        assert_eq!(
            analysis_action.argv,
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

        let review = report(vec![entry(
            "function-review",
            ProjectFileState::Pending,
            Some("advanced functions review"),
        )]);
        let review_action = &review.next_steps(&context).unwrap()[0].commands[0];
        assert_eq!(
            review_action.argv,
            [
                "blobray",
                "advanced",
                "functions",
                "review",
                "--project",
                manifest.to_str().unwrap(),
                "--target-spec",
                explicit_target.to_str().unwrap(),
            ]
        );

        let publication = report(vec![
            entry(
                "published-svd",
                ProjectFileState::Pending,
                Some("project publish"),
            ),
            entry(
                "review-workspace",
                ProjectFileState::Present,
                Some("project analyze"),
            ),
        ]);
        let publication_action = &publication.next_steps(&context).unwrap()[0].commands[0];
        assert_eq!(
            publication_action.argv,
            [
                "blobray",
                "project",
                "publish",
                "--check",
                "--project",
                manifest.to_str().unwrap(),
            ]
        );

        let complete = report(vec![entry(
            "navigation-index",
            ProjectFileState::Present,
            Some("project analyze"),
        )]);
        let status_action = &complete.next_steps(&context).unwrap()[0].commands[0];
        assert_eq!(
            status_action.argv,
            [
                "blobray",
                "project",
                "status",
                "--project",
                manifest.to_str().unwrap(),
                "--target-spec",
                explicit_target.to_str().unwrap(),
                "--run-spec",
                explicit_run.to_str().unwrap(),
            ]
        );
    }
}
