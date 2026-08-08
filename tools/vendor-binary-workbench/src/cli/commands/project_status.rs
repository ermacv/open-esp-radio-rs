//! Read-only machine-readable project lifecycle inventory.

use std::path::PathBuf;

use super::{ProjectContext, Result};
use crate::cli::ProjectStatusArgs;
use model::{Readiness, StatusReport, TargetIdentity};

mod analysis;
mod configuration_inputs;
mod model;
mod publication;
mod render;
mod review;
mod verification;

#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    json_report: Option<PathBuf>,
    check: bool,
    deny_incomplete: bool,
}

pub(super) fn run(arguments: ProjectStatusArgs, context: ProjectContext<'_>) -> Result<bool> {
    let options = Options {
        json_report: arguments.json_report,
        check: arguments.check,
        deny_incomplete: arguments.deny_incomplete,
    };
    let report = StatusReport::new(
        context.project.id.clone(),
        context.project_path.display().to_string(),
        TargetIdentity {
            id: context.target.id.clone(),
            architecture: context.target.architecture.label().to_owned(),
            calling_convention: context.target.calling_convention.label().to_owned(),
            harness: context.target.harness.clone(),
        },
        vec![
            configuration_inputs::configuration(&context),
            configuration_inputs::inputs(&context),
            analysis::collect(&context),
            review::collect(&context),
            verification::collect(&context),
            publication::collect(&context),
        ],
    );
    let publication = options.json_report.as_deref().map(|path| {
        crate::cli::output::Publication::new(
            path,
            if options.check { "verified" } else { "written" },
        )
    });
    if let Some(path) = options.json_report.as_deref() {
        let stored_document = render::document(&report, None);
        let rendered = render::json_document(&stored_document)?;
        super::super::generated_output::write_or_check(
            path,
            &rendered,
            options.check,
            "project status",
        )?;
    }
    let document = render::document(&report, publication.clone());
    if !crate::cli::output::structured("project-status", &document) {
        render::print_text(&report);
        if let Some(publication) = publication {
            outputln!(
                "PUBLICATION\tstatus={}\tpath={}",
                publication.status,
                publication.path
            );
        }
    }
    Ok(report.overall != Readiness::Invalid
        && (!options.deny_incomplete || report.overall == Readiness::Ready))
}

#[cfg(test)]
mod tests;
