//! Input selection and options for generic interface discovery.

use std::{collections::BTreeSet, path::PathBuf};

use super::super::*;
use crate::run_spec::RunSpec;

#[derive(Default)]
pub(super) struct Options {
    pub(super) check: bool,
    pub(super) json_report: Option<PathBuf>,
    pub(super) name_prefix: String,
    pub(super) sources: BTreeSet<String>,
    pub(super) tables_only: bool,
}

pub(super) fn resolve_options(arguments: InterfaceDiscoverArgs) -> Options {
    Options {
        check: arguments.check,
        json_report: arguments.json_report,
        name_prefix: arguments.name_prefix,
        sources: arguments.source.into_iter().collect(),
        tables_only: arguments.tables_only,
    }
}

pub(super) fn selected_inputs(
    run_spec: &RunSpec,
    options: &Options,
) -> Result<Vec<(String, PathBuf)>> {
    let all_sources = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_scannable())
        .map(|input| input.role.source_id().to_owned())
        .collect::<BTreeSet<_>>();
    let unknown = options
        .sources
        .difference(&all_sources)
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(crate::Error::invalid(format!(
            "unknown interface source(s): {}; available: {}",
            unknown.join(", "),
            all_sources.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_scannable())
        .filter(|input| {
            options.sources.is_empty() || options.sources.contains(input.role.source_id())
        })
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect())
}
