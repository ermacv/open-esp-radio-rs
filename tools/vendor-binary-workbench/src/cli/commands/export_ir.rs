//! Linked best-effort function/call IR export.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::super::*;

mod input;

use input::*;

mod human;

use human::{field_candidate_summary, print_report, provenance_summary};
mod pseudo;

use pseudo::{render_pseudo, write_pseudo};
mod render_common;

use render_common::*;

mod json_report;

use json_report::{document, render_document, write_json_report};

#[derive(Debug)]
pub(super) struct ProjectIrDocuments {
    pub(super) json: String,
    pub(super) pseudo: Option<String>,
    pub(super) sources: usize,
    pub(super) functions: usize,
    pub(super) registers: usize,
    pub(super) field_candidates: usize,
}

pub(super) fn run(
    arguments: IrExportArgs,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let artifacts = arguments
        .artifact
        .into_iter()
        .map(IrArtifactInput::from)
        .collect::<Vec<_>>();
    let (entry_contract, report) = analyze(
        &artifacts,
        &arguments.companion,
        &arguments.symbol_prefix,
        arguments.include_reachable,
        &arguments.entry_contract,
        svd,
        target,
    )?;

    let publications = arguments
        .json_report
        .iter()
        .chain(arguments.pseudo_rust.iter())
        .map(|path| crate::cli::output::Publication::new(path, "written"))
        .collect::<Vec<_>>();
    let document = document(
        &artifacts,
        &arguments.companion,
        &arguments.symbol_prefix,
        entry_contract,
        &report,
        arguments.include_reachable,
        publications.clone(),
    )?;
    if let Some(path) = arguments.pseudo_rust.as_deref() {
        write_pseudo(path, &artifacts, &report, arguments.include_reachable)?;
    }
    if let Some(path) = arguments.json_report.as_deref() {
        write_json_report(path, &document)?;
    }
    if !crate::cli::output::structured("linked-ir", &document) {
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

pub(super) fn generate_project_profile(
    inputs: Vec<(String, PathBuf)>,
    companions: Vec<PathBuf>,
    profile: &crate::project_ir::ProjectIrProfile,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<ProjectIrDocuments> {
    let artifacts = inputs
        .into_iter()
        .map(|(source, path)| named_artifact_path(&source, path))
        .collect::<Result<Vec<_>>>()?;
    let (entry_contract, report) = analyze(
        &artifacts,
        &companions,
        &profile.symbol_prefix,
        profile.include_reachable,
        &profile.entry_contract,
        svd,
        target,
    )?;
    let (_, field_candidates, _, _) = field_candidate_summary(&report);
    let document = document(
        &artifacts,
        &companions,
        &profile.symbol_prefix,
        entry_contract,
        &report,
        profile.include_reachable,
        Vec::new(),
    )?;
    let json = render_document(&document)?;
    let pseudo = profile
        .pseudo_rust
        .as_ref()
        .map(|_| render_pseudo(&artifacts, &report, profile.include_reachable));
    Ok(ProjectIrDocuments {
        json,
        pseudo,
        sources: artifacts.len(),
        functions: report.functions.len(),
        registers: report.mmio_registers.len(),
        field_candidates,
    })
}

#[tracing::instrument(
    name = "build_linked_ir",
    skip(artifacts, companions, svd, target),
    fields(artifacts = artifacts.len(), companions = companions.len(), symbol_prefix, include_reachable)
)]
fn analyze(
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &str,
    include_reachable: bool,
    entry_contract_id: &str,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<(EntryContractRef, LinkedIrReport)> {
    let harness = target.harness.as_deref();
    let riscv_harness = harnesses::riscv_or_neutral(harness)?;
    let entry_contract = harnesses::entry_contract_or_neutral(harness, entry_contract_id)?;
    validate_artifact_inputs(artifacts, companions)?;
    let mut reports = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let resolver = ReferenceResolver::load_all_code_with_entry_contract(
            &artifact.path,
            companions,
            riscv_harness,
            entry_contract,
        )?;
        reports.push(build_linked_ir_for_source(
            &resolver,
            symbol_prefix,
            svd,
            &artifact.source,
            true,
            include_reachable,
        ));
    }
    if artifacts.len() > 1 {
        link_project_calls(&mut reports);
    }
    let report = merge_linked_ir(reports);
    if report.functions.is_empty() {
        return Err(format!(
            "no named code symbols start with {symbol_prefix:?} in any IR artifact"
        )
        .into());
    }

    Ok((entry_contract, report))
}

#[cfg(test)]
mod tests;
