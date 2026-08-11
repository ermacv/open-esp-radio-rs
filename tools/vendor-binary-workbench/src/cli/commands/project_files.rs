//! Human and machine project-file inventory.

use std::collections::BTreeMap;

use crate::{
    Result,
    application::{ProjectContext, project_files},
    cli::{output, table},
};

pub(super) fn run(context: ProjectContext<'_>) -> Result<bool> {
    let report = project_files::collect(context)?;
    output::render_report(&report, || render_human(&report));
    Ok(true)
}

fn render_human(report: &project_files::ProjectFilesReport) {
    outputln!("{}", output::heading("Project files"));
    outputln!("Project:  {}", report.project_id);
    outputln!("Manifest: {}", report.manifest.display());
    let outcome = if report.missing != 0 {
        output::failure(format!(
            "BLOCKED — {} required file(s) missing",
            report.missing
        ))
    } else if report.pending != 0 {
        output::warning(format!(
            "READY TO ANALYZE — {} generated output(s) pending",
            report.pending
        ))
    } else {
        output::success("READY — all configured files are present")
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
        let mut actions = attention
            .iter()
            .filter_map(|file| file.next_action.as_deref())
            .collect::<Vec<_>>();
        actions.sort_unstable();
        actions.dedup();
        if !actions.is_empty() {
            outputln!("\n{}", output::heading("Next"));
            for (index, action) in actions.into_iter().enumerate() {
                outputln!("{}. {}", index + 1, with_project(action, &report.manifest));
            }
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
    if action.contains("--project") {
        action.to_owned()
    } else {
        format!("{action} --project {}", manifest.display())
    }
}
