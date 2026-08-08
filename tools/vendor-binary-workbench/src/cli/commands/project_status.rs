//! Read-only machine-readable project lifecycle inventory.

use std::path::PathBuf;

use super::Result;
use crate::{
    application::{ProjectContext, status},
    cli::ProjectStatusArgs,
};
use status::model::Readiness;

mod render;

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
    let report = status::collect(&context);
    let publication = options.json_report.as_deref().map(|path| {
        crate::cli::output::Publication::new(
            path,
            if options.check { "verified" } else { "written" },
        )
    });
    if let Some(path) = options.json_report.as_deref() {
        let stored_document = render::document(&report, None);
        let rendered = render::json_document(&stored_document)?;
        crate::application::generated_file::write_or_check(
            path,
            &rendered,
            options.check,
            "project status",
        )?;
    }
    let document = render::document(&report, publication.clone());
    if !crate::cli::output::structured(&document) {
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
