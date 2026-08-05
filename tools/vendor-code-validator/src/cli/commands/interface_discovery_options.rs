//! Input selection and options for generic interface discovery.

use std::{collections::BTreeSet, path::PathBuf};

use super::super::*;
use crate::run_spec::RunSpec;

#[derive(Default)]
pub(super) struct Options {
    pub(super) json_report: Option<PathBuf>,
    pub(super) name_prefix: String,
    pub(super) sources: BTreeSet<String>,
    pub(super) tables_only: bool,
}

pub(super) fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json-report" => {
                if options.json_report.is_some() {
                    return Err("duplicate --json-report".into());
                }
                options.json_report =
                    Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            "--name-prefix" => {
                if !options.name_prefix.is_empty() {
                    return Err("duplicate --name-prefix".into());
                }
                options.name_prefix = take_value(&mut arguments, "--name-prefix")?;
            }
            "--source" => {
                options
                    .sources
                    .insert(take_value(&mut arguments, "--source")?);
            }
            "--tables-only" => options.tables_only = true,
            _ => return Err(format!("unknown interfaces discover option: {argument}").into()),
        }
    }
    Ok(options)
}

fn is_scannable_role(role: &str) -> bool {
    role == "artifact"
        || role.ends_with("-artifact")
        || role.ends_with("-inventory")
        || role.starts_with("source-artifact:")
        || role.starts_with("source-inventory:")
}

pub(super) fn selected_inputs(
    run_spec: &RunSpec,
    options: &Options,
) -> Result<Vec<(String, PathBuf)>> {
    let all_sources = run_spec
        .inputs()
        .iter()
        .filter(|(role, _)| is_scannable_role(role))
        .map(|(role, _)| crate::analysis::source_id(role))
        .collect::<BTreeSet<_>>();
    let unknown = options
        .sources
        .difference(&all_sources)
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown interface source(s): {}; available: {}",
            unknown.join(", "),
            all_sources.into_iter().collect::<Vec<_>>().join(", ")
        )
        .into());
    }
    Ok(run_spec
        .inputs()
        .iter()
        .filter(|(role, _)| is_scannable_role(role))
        .filter(|(role, _)| {
            options.sources.is_empty()
                || options.sources.contains(&crate::analysis::source_id(role))
        })
        .cloned()
        .collect())
}
