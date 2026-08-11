//! CLI presentation for application-owned project publication.

use super::Result;
use crate::{
    MemoryMap,
    application::project_publication::{ProjectPublicationReport, ProjectPublicationRequest},
    cli::CheckArgs,
    project::ProjectSpec,
};

pub(super) fn run(
    arguments: CheckArgs,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let report = crate::application::project_publication::execute(
        project,
        memory_map,
        ProjectPublicationRequest {
            check: arguments.check,
        },
    )?;
    render(&report);
    Ok(report.succeeded())
}

fn render(document: &ProjectPublicationReport) {
    crate::cli::output::render_report(document, || print_human(document));
}

fn print_human(document: &ProjectPublicationReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project publication"));
    outputln!("Mode: {}", document.mode);
    let outcome = if document.succeeded() {
        output::success(format!(
            "READY — {} written, {} verified",
            document.written, document.verified
        ))
    } else {
        output::failure(format!(
            "BLOCKED — {} failed, {} blocked",
            document.failed, document.blocked
        ))
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

    outputln!("\n{}", output::heading("Outputs"));
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
    if document.not_configured != 0 {
        outputln!(
            "{} optional output(s) are not configured.",
            document.not_configured
        );
    }
}

#[cfg(test)]
mod tests;
