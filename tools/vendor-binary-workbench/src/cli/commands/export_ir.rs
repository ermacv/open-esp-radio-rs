//! Linked best-effort function/call IR export.

use super::super::*;

mod human;

use crate::linked_ir_export::{IrArtifactInput, analyze, write_pseudo};
#[cfg(test)]
use crate::linked_ir_export::{named_artifact, validate_artifact_inputs};
use human::print_report;

pub(super) fn run(
    arguments: IrExportArgs,
    svd: &MmioMap,
    target: &TargetSpec,
    project: Option<&crate::project::ProjectSpec>,
) -> Result<bool> {
    if arguments.jobs > 8 {
        return Err(crate::Error::invalid(
            "ir export --jobs accepts 0 (safe automatic mode) or 1..=8",
        ));
    }
    let mut artifacts = arguments
        .artifact
        .into_iter()
        .map(|value| IrArtifactInput {
            source: value.source.into_string(),
            path: value.path,
            reviewed_code: Vec::new(),
        })
        .collect::<Vec<_>>();
    let effective_code = project
        .map(crate::analysis::EffectiveCodeCatalog::load)
        .transpose()?;
    let interfaces = project
        .map(|project| crate::linked_ir_export::load_project_interfaces(project, target))
        .transpose()?
        .flatten();
    if let Some(catalog) = &effective_code {
        for artifact in &mut artifacts {
            artifact.reviewed_code = catalog.reviewed_ranges(&artifact.source, &artifact.path)?;
        }
    }
    let (entry_contract, report) = analyze(
        &artifacts,
        &arguments.companion,
        &arguments.symbol_prefix,
        arguments.include_reachable,
        &arguments.entry_contract,
        svd,
        target,
        interfaces.as_ref(),
        usize::from(arguments.jobs),
    )?;

    let publications = arguments
        .json_report
        .iter()
        .chain(arguments.pseudo_rust.iter())
        .map(|path| crate::cli::output::Publication::new(path, "written"))
        .collect::<Vec<_>>();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &arguments.companion,
        &arguments.symbol_prefix,
        entry_contract,
        &report,
        arguments.include_reachable,
    )?;
    if let Some(path) = arguments.pseudo_rust.as_deref() {
        write_pseudo(
            path,
            &artifacts,
            &report,
            &arguments.symbol_prefix,
            arguments.include_reachable,
        )?;
    }
    if let Some(path) = arguments.json_report.as_deref() {
        crate::artifacts::write_linked_ir(path, &document)?;
    }
    if !crate::cli::output::structured(&document) {
        print_report(&artifacts, &report, arguments.include_reachable);
        for publication in publications {
            outputln!(
                "PUBLICATION\tstatus={}\tpath={}",
                publication.status,
                publication.path
            );
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
