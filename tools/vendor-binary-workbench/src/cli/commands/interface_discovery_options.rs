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
