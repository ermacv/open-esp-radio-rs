//! Linked best-effort function/call IR export.

use std::{collections::BTreeSet, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
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

use json_report::{render_json_report, write_json_report};

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
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut artifacts = Vec::new();
    let mut companions = Vec::new();
    let mut symbol_prefix = String::new();
    let mut include_reachable = false;
    let mut pseudo_path = None;
    let mut json_report = None;
    let mut entry_contract_id = "none".to_owned();
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        if let Some(source) = source_artifact_option(&argument) {
            artifacts.push(named_artifact(
                source,
                &take_value(&mut arguments, &argument)?,
            )?);
            continue;
        }
        match argument.as_str() {
            "--artifact" => {
                artifacts.push(parse_artifact(&take_value(&mut arguments, "--artifact")?)?);
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--symbol-prefix" => {
                symbol_prefix = take_value(&mut arguments, "--symbol-prefix")?;
            }
            "--include-reachable" => include_reachable = true,
            "--entry-contract" => {
                entry_contract_id = take_value(&mut arguments, "--entry-contract")?;
            }
            "--pseudo-rust" => {
                pseudo_path = Some(PathBuf::from(take_value(&mut arguments, "--pseudo-rust")?));
            }
            "--json-report" => {
                json_report = Some(PathBuf::from(take_value(&mut arguments, "--json-report")?));
            }
            _ => return Err(format!("unknown ir export option: {argument}").into()),
        }
    }
    let (entry_contract, report) = analyze(
        &artifacts,
        &companions,
        &symbol_prefix,
        include_reachable,
        &entry_contract_id,
        svd,
        target,
    )?;

    print_report(&artifacts, &report, include_reachable);
    if let Some(path) = pseudo_path.as_deref() {
        write_pseudo(path, &artifacts, &report, include_reachable)?;
    }
    if let Some(path) = json_report.as_deref() {
        write_json_report(
            path,
            &artifacts,
            &companions,
            &symbol_prefix,
            entry_contract,
            &report,
            include_reachable,
        )?;
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
    let json = render_json_report(
        &artifacts,
        &companions,
        &profile.symbol_prefix,
        entry_contract,
        &report,
        profile.include_reachable,
    )?;
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
    let namespace_identities = validate_artifact_inputs(artifacts, companions)?;
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
            namespace_identities,
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
