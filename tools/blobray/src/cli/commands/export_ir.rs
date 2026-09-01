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
    if !(1..=8).contains(&arguments.jobs) {
        return Err(crate::Error::invalid("ir export --jobs accepts 1..=8"));
    }
    if arguments.pseudo_rust.is_some() && arguments.symbol_prefix.is_empty() {
        return Err(crate::Error::invalid(
            "--pseudo-rust requires a non-empty --symbol-prefix; use `inspect function` for artifact-wide projects",
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
    let interface_origins = project
        .map(crate::linked_ir_export::load_project_interface_origins)
        .transpose()?
        .unwrap_or_default();
    let reviewed_bindings = project
        .map(|project| {
            open_radio_vendor_review::ReviewKnowledge::load_all(&project.reviewed_knowledge)
                .and_then(|knowledge| knowledge.select_for(&project.review_context))
                .map(|knowledge| {
                    knowledge
                        .bindings()
                        .values()
                        .map(|binding| (binding.occurrence.clone(), binding.semantic.clone()))
                        .collect::<std::collections::BTreeMap<_, _>>()
                })
        })
        .transpose()
        .map_err(|error| {
            crate::Error::invalid(format!(
                "cannot select reviewed entity bindings for linked-IR export: {error}"
            ))
        })?
        .unwrap_or_default();
    if let Some(catalog) = &effective_code {
        for artifact in &mut artifacts {
            artifact.reviewed_code = catalog.reviewed_ranges(&artifact.source, &artifact.path)?;
        }
    }
    let inventories = Vec::new();
    let (entry_contract, report) = analyze(
        crate::linked_ir_export::LinkedIrAnalysisRequest {
            artifacts: &artifacts,
            inventories: &inventories,
            companions: &arguments.companion,
            symbol_prefix: &arguments.symbol_prefix,
            include_reachable: arguments.include_reachable,
            entry_contract_id: &arguments.entry_contract,
            svd,
            target,
            interfaces: interfaces.as_ref(),
            interface_origins: &interface_origins,
            jobs: usize::from(arguments.jobs),
            compact_projected_actions: arguments.pseudo_rust.is_none(),
        },
        None,
    )?;

    let publications = arguments
        .output
        .iter()
        .chain(arguments.pseudo_rust.iter())
        .map(|path| crate::cli::output::Publication::new(path, "written"))
        .collect::<Vec<_>>();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &inventories,
        &arguments.companion,
        &arguments.symbol_prefix,
        entry_contract,
        crate::artifacts::LinkedIrPublication {
            report: &report,
            reviewed_bindings: &reviewed_bindings,
        },
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
    if let Some(path) = arguments.output.as_deref() {
        crate::artifacts::stage_linked_ir_bundle(path, &document)?.publish(path)?;
    }
    if !crate::cli::output::structured(&document) {
        print_report(&artifacts, &report, arguments.include_reachable);
        for publication in publications {
            outputln!("\nOutput {}: {}", publication.status, publication.path);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
