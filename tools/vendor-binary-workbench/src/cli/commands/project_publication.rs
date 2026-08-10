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
    outputln!(
        "Project publication: {} ({})",
        document.status,
        document.mode
    );
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
}

#[cfg(test)]
mod tests;
