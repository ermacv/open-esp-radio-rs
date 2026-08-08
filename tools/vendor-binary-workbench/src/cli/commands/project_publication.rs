//! CLI adapter and presentation for application-owned project publication.

use super::{Result, registers};
use crate::{
    MemoryMap,
    application::project_publication::{
        ProjectPublicationOperations, ProjectPublicationReport, ProjectPublicationRequest,
    },
    cli::{CheckArgs, ValidationArgs, resolver::RegisterWorkspaceCommand},
    project::ProjectSpec,
};

pub(super) fn run(
    arguments: CheckArgs,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or("project publish requires a [registers] workspace")
        .map_err(crate::Error::invalid)?;
    let mut operations = CliProjectPublicationOperations {
        project,
        memory_map,
    };
    let report = crate::cli::output::suppress(|| {
        crate::application::project_publication::run(
            paths,
            ProjectPublicationRequest {
                check: arguments.check,
            },
            &mut operations,
        )
    })?;
    render(&report);
    Ok(report.succeeded())
}

struct CliProjectPublicationOperations<'a> {
    project: &'a ProjectSpec,
    memory_map: Option<&'a MemoryMap>,
}

impl ProjectPublicationOperations for CliProjectPublicationOperations<'_> {
    type Prepared = registers::PreparedPublication;

    fn validate_registers(&mut self) -> Result<bool> {
        registers::run(
            RegisterWorkspaceCommand::Validate(ValidationArgs {
                deny_unreviewed: true,
            }),
            self.project,
            self.memory_map,
        )
    }

    fn prepare_svd(&mut self) -> Result<Self::Prepared> {
        registers::prepare_project_svd(
            self.project
                .registers
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?,
        )
    }

    fn prepare_pac(&mut self) -> Result<Self::Prepared> {
        registers::prepare_project_pac(
            self.project
                .registers
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?,
        )
    }

    fn prepare_bindings(&mut self) -> Result<Self::Prepared> {
        registers::prepare_project_bindings(
            self.project
                .registers
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?,
        )
    }

    fn publish(&mut self, publication: &Self::Prepared, check: bool) -> Result<bool> {
        registers::write_prepared_publication(publication, check)
    }
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
