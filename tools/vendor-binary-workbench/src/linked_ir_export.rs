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
    EntryContractRef, LinkedIrReport, MmioMap, ReferenceResolver, Result, ReviewedExternalCall,
    ReviewedExternalCallEvidence, ReviewedExternalCallExecutionModel, StructuralCallSite,
    TargetSpec, artifacts::LinkUnitOriginFact, build_linked_ir_for_source, harnesses,
    interfaces::InterfaceWorkspace, link_project_calls, merge_linked_ir_with_jobs,
    project_ir::ProjectIrProfile,
};

#[derive(Debug)]
pub(crate) struct ProjectIrDocuments {
    pub(crate) json: String,
    pub(crate) pseudo: Option<String>,
    pub(crate) sources: usize,
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) fn generate_project_profile(
    inputs: Vec<(String, PathBuf)>,
    companions: Vec<PathBuf>,
    profile: &ProjectIrProfile,
    svd: &MmioMap,
    target: &TargetSpec,
    effective_code: &crate::analysis::EffectiveCodeCatalog,
    interfaces: Option<&InterfaceWorkspace>,
    interface_origins: &[LinkUnitOriginFact],
    jobs: usize,
) -> Result<ProjectIrDocuments> {
    let started = std::time::Instant::now();
    let mut artifacts = inputs
        .into_iter()
        .map(|(source, path)| named_artifact_path(&source, path))
        .collect::<Result<Vec<_>>>()?;
    for artifact in &mut artifacts {
        artifact.reviewed_code =
            effective_code.reviewed_ranges(&artifact.source, &artifact.path)?;
    }
    let (entry_contract, report) = analyze(
        &artifacts,
        &companions,
        profile.roots.symbol_prefix(),
        profile.include_reachable,
        &profile.entry_contract,
        svd,
        target,
        interfaces,
        interface_origins,
        jobs,
    )?;
    let (_, field_candidates, _, _) = field_candidate_summary(&report);
    let decode_blockers = report
        .functions
        .iter()
        .map(|function| function.decode_blockers.len())
        .sum();
    let analysis_elapsed = started.elapsed();
    let render_started = std::time::Instant::now();
    let document = crate::artifacts::build_linked_ir_document(
        &artifacts,
        &companions,
        profile.roots.symbol_prefix(),
        entry_contract,
        &report,
        profile.include_reachable,
    )?;
    let json = crate::artifacts::render_linked_ir(&document)?;
    let pseudo = profile.pseudo_rust.as_ref().map(|_| {
        render_pseudo(
            &artifacts,
            &report,
            profile.roots.symbol_prefix(),
            profile.include_reachable,
        )
    });
    tracing::debug!(
        profile = profile.id,
        functions = report.functions.len(),
        analysis_ms = analysis_elapsed.as_millis(),
        render_ms = render_started.elapsed().as_millis(),
        "rendered project linked-IR profile"
    );
    Ok(ProjectIrDocuments {
        json,
        pseudo,
        sources: artifacts.len(),
        functions: report.functions.len(),
        decode_blockers,
        registers: report.mmio_registers.len(),
        field_candidates,
    })
}

#[tracing::instrument(
    name = "build_linked_ir",
    skip(
        artifacts,
        companions,
        svd,
        target,
        interfaces,
        interface_origins
    ),
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
    interfaces: Option<&InterfaceWorkspace>,
    interface_origins: &[LinkUnitOriginFact],
    jobs: usize,
) -> Result<(EntryContractRef, LinkedIrReport)> {
    let harness = target.harness.as_deref();
    let riscv_harness = harnesses::riscv_or_neutral(harness)?;
    let entry_contract = harnesses::entry_contract_or_neutral(harness, entry_contract_id)?;
    validate_artifact_inputs(artifacts, companions)?;
    let mut reports = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let mut resolver = ReferenceResolver::load_all_code_with_reviewed_ranges(
            &artifact.path,
            companions,
            riscv_harness,
            entry_contract,
            &artifact.reviewed_code,
        )?;
        if let Some(interfaces) = interfaces {
            register_reviewed_external_calls(
                &mut resolver,
                interfaces,
                &artifact.source,
                interface_origins,
            );
        }
        reports.push(build_linked_ir_for_source(
            &resolver,
            symbol_prefix,
            svd,
            &artifact.source,
            true,
            include_reachable,
            jobs,
        ));
    }
    if artifacts.len() > 1 {
        link_project_calls(&mut reports);
    }
    let report = merge_linked_ir_with_jobs(reports, jobs);
    if report.functions.is_empty() {
        return Err(crate::Error::invalid(if symbol_prefix.is_empty() {
            "no named code symbols were found in any IR artifact".to_owned()
        } else {
            format!("no named code symbols start with {symbol_prefix:?} in any IR artifact")
        }));
    }
    Ok((entry_contract, report))
}

pub(crate) fn load_project_interface_origins(
    project: &crate::project::ProjectSpec,
) -> Result<Vec<LinkUnitOriginFact>> {
    let Some(symbols) = project.symbol_inventory.as_ref() else {
        return Ok(Vec::new());
    };
    if !symbols.output.is_file() {
        tracing::debug!(
            path = %symbols.output.display(),
            "symbol inventory is unavailable; linked interface projection is disabled"
        );
        return Ok(Vec::new());
    }
    crate::artifacts::load_link_unit_origins(&symbols.output)
}

pub(crate) fn load_project_interfaces(
    project: &crate::project::ProjectSpec,
    target: &TargetSpec,
) -> Result<Option<InterfaceWorkspace>> {
    let Some(paths) = project.interfaces.as_ref() else {
        return Ok(None);
    };
    let Some(pack) = paths.pack.as_deref() else {
        return Ok(None);
    };
    Ok(Some(InterfaceWorkspace::load(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        target.calling_convention.label(),
        target
            .harness
            .as_deref()
            .map(harnesses::contracts)
            .transpose()?,
    )?))
}

pub(crate) fn register_reviewed_external_calls(
    resolver: &mut ReferenceResolver,
    interfaces: &InterfaceWorkspace,
    source: &str,
    origins: &[LinkUnitOriginFact],
) {
    for contract in interfaces.contracts() {
        let container_offset = match contract.container_path.as_slice() {
            [] => 0,
            [step] if step.width == contract.pointer_width && step.selector.is_none() => {
                step.offset
            }
            _ => continue,
        };
        if container_offset != 0
            && !matches!(
                contract.root,
                crate::interfaces::InterfaceRootSelector::AbsoluteAddress { .. }
            )
        {
            continue;
        }
        let value = crate::SymbolicValue::ReviewedExternalTable(contract.id.clone());
        match &contract.root {
            crate::interfaces::InterfaceRootSelector::RelocatedSymbol {
                member,
                symbol,
                addend,
                ..
            } if *addend == 0 => {
                resolver
                    .pointer_context
                    .relocated_pointer_symbols
                    .insert(symbol.clone(), value.clone());
                for definition in &resolver.data_symbols {
                    if definition.name == *symbol && definition.member == *member {
                        resolver
                            .pointer_context
                            .reviewed_external_pointer_cells
                            .insert(definition.address as u32, contract.id.clone());
                    }
                }
            }
            crate::interfaces::InterfaceRootSelector::AbsoluteAddress { address } => {
                let Some(address) = address.checked_add_signed(container_offset) else {
                    continue;
                };
                resolver
                    .pointer_context
                    .reviewed_external_pointer_cells
                    .insert(address, contract.id.clone());
            }
            _ => {}
        }
    }
    for slot in interfaces.bindings() {
        let reviewed = ReviewedExternalCall {
            id: slot.id.clone(),
            contract: slot.contract.clone(),
            name: slot.name.clone(),
            argument_types: slot.arguments.clone(),
            return_type: slot.return_type.clone(),
            variadic: slot.variadic,
            semantic_operation: slot
                .semantic_annotation
                .as_ref()
                .map(|semantic| semantic.operation.clone()),
            replacement_hint: slot
                .semantic_annotation
                .as_ref()
                .and_then(|semantic| semantic.replacement.clone()),
            execution_model: slot.execution_model.as_ref().map(|model| {
                ReviewedExternalCallExecutionModel {
                    id: model.id.clone(),
                    return_model: model.return_model,
                }
            }),
            tail: false,
            evidence: ReviewedExternalCallEvidence::ObservedCallSite,
            slot_load_site: None,
        };
        if let Ok(offset) = u32::try_from(slot.offset) {
            let candidates = resolver
                .pointer_context
                .reviewed_external_slots
                .entry((slot.contract.clone(), offset))
                .or_default();
            if !candidates.contains(&reviewed) {
                candidates.push(reviewed.clone());
                candidates.sort();
            }
        }
        for call in &slot.calls {
            let mut reviewed = reviewed.clone();
            reviewed.tail = call.kind == "tail-jump";
            reviewed.slot_load_site = call.slot_load_site;
            let candidates = resolver
                .pointer_context
                .reviewed_external_calls
                .entry(StructuralCallSite::from_identity(
                    call.member.clone(),
                    call.function.clone(),
                    call.site,
                ))
                .or_default();
            if !candidates.contains(&reviewed) {
                candidates.push(reviewed);
                candidates.sort();
            }
        }
    }
    for projected in interfaces.project_link_unit_calls(source, origins) {
        let slot = projected.binding;
        let reviewed = ReviewedExternalCall {
            id: slot.id.clone(),
            contract: slot.contract.clone(),
            name: slot.name.clone(),
            argument_types: slot.arguments.clone(),
            return_type: slot.return_type.clone(),
            variadic: slot.variadic,
            semantic_operation: slot
                .semantic_annotation
                .as_ref()
                .map(|semantic| semantic.operation.clone()),
            replacement_hint: slot
                .semantic_annotation
                .as_ref()
                .and_then(|semantic| semantic.replacement.clone()),
            execution_model: slot.execution_model.as_ref().map(|model| {
                ReviewedExternalCallExecutionModel {
                    id: model.id.clone(),
                    return_model: model.return_model,
                }
            }),
            tail: projected.tail,
            evidence: ReviewedExternalCallEvidence::ArchiveOriginProjection,
            slot_load_site: projected.slot_load_site,
        };
        let candidates = resolver
            .pointer_context
            .reviewed_external_calls
            .entry(StructuralCallSite::from_identity(
                projected.member,
                projected.function,
                projected.site,
            ))
            .or_default();
        if !candidates.contains(&reviewed) {
            candidates.push(reviewed);
            candidates.sort();
        }
    }
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
