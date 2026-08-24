//! Human and machine project-file inventory.

use std::collections::BTreeMap;

use crate::{
    Result,
    application::{FollowUpRequirements, ProjectContext, project_files},
    cli::{output, table},
};

#[derive(serde::Serialize)]
struct ProjectFilesDocument<'a> {
    #[serde(flatten)]
    report: &'a project_files::ProjectFilesReport,
    workflow_state: project_files::ProjectFilesWorkflowState,
    required_missing: usize,
    next_actions: Vec<String>,
}

pub(super) fn run(context: ProjectContext<'_>) -> Result<bool> {
    let report = project_files::collect(&context)?;
    let next_actions = report
        .next_actions()
        .iter()
        .map(|action| contextualize_action(action, &context))
        .collect();
    let document = ProjectFilesDocument {
        report: &report,
        workflow_state: report.workflow_state(),
        required_missing: report.required_missing(),
        next_actions,
    };
    output::render_report(&document, || render_human(&report, &document.next_actions));
    Ok(true)
}

fn render_human(report: &project_files::ProjectFilesReport, next_actions: &[String]) {
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

    if !next_actions.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, action) in next_actions.iter().enumerate() {
            outputln!("{}. {}", index + 1, action);
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

fn contextualize_action(action: &str, context: &ProjectContext<'_>) -> String {
    let Some(command) = action.strip_prefix("blobray ") else {
        return action.to_owned();
    };
    match command {
        "project inputs init --help" => context.inputs_init_help_command(),
        "project doctor" | "project analyze" | "project verify" | "advanced ir build" => {
            context.follow_up_command(command, FollowUpRequirements::ANALYSIS)
        }
        "advanced symbols inventory" | "advanced interfaces discover" | "project status" => {
            context.follow_up_command(command, FollowUpRequirements::RUN_SPEC)
        }
        "advanced functions init-pack"
        | "advanced interfaces init-pack"
        | "advanced functions review" => {
            context.follow_up_command(command, FollowUpRequirements::TARGET)
        }
        "advanced code init-pack"
        | "advanced code review"
        | "registers review"
        | "project publish --check" => {
            context.follow_up_command(command, FollowUpRequirements::PROJECT_ONLY)
        }
        _ => action.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MmioMap, TargetSpec, application::ExplicitProjectContext, project::ProjectSpec};
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
        };
        let arg = |path: &Path| crate::shell::arg(path.as_os_str());

        assert_eq!(
            contextualize_action("blobray project inputs init --help", &context),
            format!(
                "blobray project inputs init --project {} --output {} --help",
                arg(&manifest),
                arg(&explicit_run),
            )
        );
        assert_eq!(
            contextualize_action("blobray project analyze", &context),
            format!(
                "blobray project analyze --project {} --target-spec {} --run-spec {} --svd {} --svd {}",
                arg(&manifest),
                arg(&explicit_target),
                arg(&explicit_run),
                arg(&explicit_svds[0]),
                arg(&explicit_svds[1]),
            )
        );
        assert_eq!(
            contextualize_action("blobray project status", &context),
            format!(
                "blobray project status --project {} --target-spec {} --run-spec {}",
                arg(&manifest),
                arg(&explicit_target),
                arg(&explicit_run),
            )
        );
        assert_eq!(
            contextualize_action("blobray advanced functions init-pack", &context),
            format!(
                "blobray advanced functions init-pack --project {} --target-spec {}",
                arg(&manifest),
                arg(&explicit_target),
            )
        );
        assert_eq!(
            contextualize_action("blobray project publish --check", &context),
            format!(
                "blobray project publish --check --project {}",
                arg(&manifest)
            )
        );
        assert_eq!(
            contextualize_action("restore the vendor archive", &context),
            "restore the vendor archive"
        );
    }
}
