//! Human and machine project-file inventory.

use std::collections::BTreeMap;

use crate::{
    Result,
    application::{ProjectContext, project_files},
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
    let report = project_files::collect(context)?;
    let document = ProjectFilesDocument {
        report: &report,
        workflow_state: report.workflow_state(),
        required_missing: report.required_missing(),
        next_actions: report
            .next_actions()
            .iter()
            .map(|action| with_project(action, &report.manifest))
            .collect(),
    };
    output::render_report(&document, || render_human(&report));
    Ok(true)
}

fn render_human(report: &project_files::ProjectFilesReport) {
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

    let mut ownership = BTreeMap::<&str, (usize, usize, usize)>::new();
    for file in &report.files {
        let counts = ownership.entry(file.ownership.label()).or_default();
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

    let actions = report.next_actions();
    if !actions.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, action) in actions.iter().enumerate() {
            outputln!("{}. {}", index + 1, with_project(action, &report.manifest));
        }
    }

    outputln!("\n{}", output::heading("Workspace map"));
    outputln!(
        "{}",
        table::render(
            ["Ownership", "Present", "Missing", "Pending"],
            ownership.into_iter().map(|(owner, counts)| [
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
                ["Ownership", "Role", "State", "Path"],
                report.files.iter().map(|file| [
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

fn with_project(action: &str, manifest: &std::path::Path) -> String {
    if action.contains("--project")
        || (!action.starts_with("blobray ") && !action.starts_with("cargo blobray "))
    {
        action.to_owned()
    } else if let Some(command) = action.strip_suffix(" --help") {
        format!("{command} --project {} --help", output::shell_arg(manifest))
    } else {
        format!("{action} --project {}", output::shell_arg(manifest))
    }
}
