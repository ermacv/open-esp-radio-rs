//! CLI-independent linked-IR generation and rendering support.

mod input;
mod pseudo;
mod render_common;

use std::path::PathBuf;

#[cfg(test)]
pub(crate) use input::named_artifact;
pub(crate) use input::{IrArtifactInput, named_artifact_path, validate_artifact_inputs};
pub(crate) use pseudo::{render_pseudo, write_pseudo};
pub(crate) use render_common::{
    format_site_path, guard_direct_mmio_links, guard_mmio_links, optional_hex_text,
};

use crate::{
    EntryContractRef, LinkedIrReport, MmioMap, ReferenceResolver, Result, TargetSpec,
    build_linked_ir_for_source, harnesses, link_project_calls, merge_linked_ir,
    project_ir::ProjectIrProfile,
};

#[derive(Debug)]
pub(crate) struct ProjectIrDocuments {
    pub(crate) json: String,
    pub(crate) pseudo: Option<String>,
    pub(crate) sources: usize,
    pub(crate) functions: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) fn generate_project_profile(
    inputs: Vec<(String, PathBuf)>,
    companions: Vec<PathBuf>,
    profile: &ProjectIrProfile,
    svd: &MmioMap,
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
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &companions,
        &profile.symbol_prefix,
        entry_contract,
        &report,
        profile.include_reachable,
    )?;
    let json = crate::artifacts::render_linked_ir(&document)?;
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
pub(crate) fn analyze(
    artifacts: &[IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &str,
    include_reachable: bool,
    entry_contract_id: &str,
    svd: &MmioMap,
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
        return Err(crate::Error::invalid(format!(
            "no named code symbols start with {symbol_prefix:?} in any IR artifact"
        )));
    }
    Ok((entry_contract, report))
}

pub(crate) fn provenance_summary(report: &LinkedIrReport) -> (usize, usize, usize, usize, usize) {
    let exact_return_functions = report
        .functions
        .iter()
        .filter(|function| function.return_provenance.exact)
        .count();
    let return_source_ranges = report
        .functions
        .iter()
        .map(|function| function.return_provenance.sources.len())
        .sum();
    let mmio_return_sources = report
        .functions
        .iter()
        .flat_map(|function| &function.return_provenance.sources)
        .filter(|source| source.kind == "mmio-read")
        .count();
    let guard_mmio_links = report
        .functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter_map(|call| call.guard_paths.as_deref())
        .flatten()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| &guard.result_sources)
        .map(|source| source.mmio_sources.len())
        .sum();
    let transitive_guard_mmio_links = report
        .functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter_map(|call| call.guard_paths.as_deref())
        .flatten()
        .flat_map(|path| &path.guards)
        .flat_map(|guard| &guard.result_sources)
        .flat_map(|source| &source.mmio_sources)
        .filter(|source| source.producer_path.len() > 1)
        .count();
    (
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
    )
}

pub(crate) fn field_candidate_summary(report: &LinkedIrReport) -> (usize, usize, usize, usize) {
    let registers = report
        .mmio_registers
        .iter()
        .filter(|register| !register.field_candidates.is_empty())
        .count();
    let candidates = report
        .mmio_registers
        .iter()
        .map(|register| register.field_candidates.len())
        .sum();
    let direct_predicates = report
        .functions
        .iter()
        .map(|function| function.direct_mmio_predicates.len())
        .sum();
    let direct_sources = report
        .functions
        .iter()
        .flat_map(|function| &function.direct_mmio_predicates)
        .map(|predicate| predicate.sources.len())
        .sum();
    (registers, candidates, direct_predicates, direct_sources)
}
