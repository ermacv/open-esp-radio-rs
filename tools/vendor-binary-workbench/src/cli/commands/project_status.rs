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
    let document = render::document(&report);
    if !crate::cli::output::structured("project-status", &document) {
        render::print_text(&report);
    }
    if let Some(path) = options.json_report.as_deref() {
        let document = render::json_document(&report)?;
        super::super::generated_output::write_or_check(
            path,
            &document,
            options.check,
            "project status",
        )?;
        let status = if options.check { "verified" } else { "written" };
        if !crate::cli::output::file("project-status-file", path, status) {
            outputln!(
                "PROJECT-STATUS-JSON\tstatus={status}\tpath={}",
                path.display()
            );
        }
    }
    Ok(report.overall != Readiness::Invalid
        && (!options.deny_incomplete || report.overall == Readiness::Ready))
}

#[cfg(test)]
mod tests;
