//! Read-only machine-readable project lifecycle inventory.

use std::path::PathBuf;

use super::{ProjectContext, Result};
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

pub(super) fn run(arguments: Vec<String>, context: ProjectContext<'_>) -> Result<bool> {
    let options = parse_options(arguments)?;
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
    render::print_text(&report);
    if let Some(path) = options.json_report.as_deref() {
        let document = render::json_document(&report)?;
        super::super::generated_output::write_or_check(
            path,
            &document,
            options.check,
            "project status",
        )?;
        println!(
            "PROJECT-STATUS-JSON\tstatus={}\tpath={}",
            if options.check { "verified" } else { "written" },
            path.display()
        );
    }
    Ok(report.overall != Readiness::Invalid
        && (!options.deny_incomplete || report.overall == Readiness::Ready))
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json-report" => {
                let path = PathBuf::from(arguments.next().ok_or("--json-report requires a path")?);
                if options.json_report.replace(path).is_some() {
                    return Err("duplicate --json-report".into());
                }
            }
            "--check" if !options.check => options.check = true,
            "--check" => return Err("duplicate --check".into()),
            "--deny-incomplete" if !options.deny_incomplete => options.deny_incomplete = true,
            "--deny-incomplete" => return Err("duplicate --deny-incomplete".into()),
            _ => return Err(format!("unknown project status option: {argument}").into()),
        }
    }
    if options.check && options.json_report.is_none() {
        return Err("project status --check requires --json-report PATH".into());
    }
    Ok(options)
}

#[cfg(test)]
mod tests;
