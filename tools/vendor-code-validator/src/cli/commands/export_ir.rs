//! Linked best-effort function/call IR export.

use std::{collections::BTreeSet, path::Path};

use super::super::json::{write_artifact, write_string, write_strings};
use super::super::*;

mod input;

use input::*;

mod human;

use human::{field_candidate_summary, print_report, provenance_summary};
mod pseudo;

use pseudo::write_pseudo;
mod render_common;

use render_common::*;

mod json_report;

use json_report::write_json_report;
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
    let riscv_harness = harnesses::riscv(&target.harness)?;
    let mut entry_contract = harnesses::entry_contract(&target.harness, "none")?;
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
                entry_contract = harnesses::entry_contract(
                    &target.harness,
                    &take_value(&mut arguments, "--entry-contract")?,
                )?;
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
    let namespace_identities = validate_artifact_inputs(&artifacts, &companions)?;
    let mut reports = Vec::with_capacity(artifacts.len());
    for artifact in &artifacts {
        let resolver = ReferenceResolver::load_all_code_with_entry_contract(
            &artifact.path,
            &companions,
            riscv_harness,
            entry_contract,
        )?;
        reports.push(build_linked_ir_for_source(
            &resolver,
            &symbol_prefix,
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

#[cfg(test)]
mod tests;
