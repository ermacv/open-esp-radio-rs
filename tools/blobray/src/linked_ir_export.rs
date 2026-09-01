//! CLI-independent linked-IR generation and rendering support.

mod input;
mod pseudo;
mod render_common;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[cfg(test)]
pub(crate) use input::named_artifact;
pub(crate) use input::{IrArtifactInput, named_artifact_path, validate_artifact_inputs};
pub(crate) use pseudo::write_pseudo;
pub(crate) use render_common::{
    format_site_path, guard_direct_mmio_links, guard_mmio_links, optional_hex_text,
};

use crate::{
    EntryContractRef, LinkedIrReport, LinkedIrSourceOptions, MmioMap, ReferenceResolver, Result,
    ReviewedExternalCall, ReviewedExternalCallEvidence, ReviewedExternalCallExecutionModel,
    StructuralCallSite, StructuralProjectedRelocation, TargetSpec, artifacts::LinkUnitOriginFact,
    build_linked_ir_for_source_with_cache, harnesses, interfaces::InterfaceWorkspace,
    link_project_calls, merge_linked_ir_with_options, project_ir::ProjectIrProfile,
};

#[derive(Debug)]
pub(crate) struct ProjectIrDocuments {
    pub(crate) bundle: crate::artifacts::StagedLinkedIrBundle,
    pub(crate) sources: usize,
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) struct ProjectProfileRequest<'a, 'cache> {
    pub(crate) inputs: Vec<(String, PathBuf)>,
    pub(crate) inventories: Vec<(String, PathBuf)>,
    pub(crate) companions: Vec<PathBuf>,
    pub(crate) profile: &'a ProjectIrProfile,
    pub(crate) svd: &'a MmioMap,
    pub(crate) target: &'a TargetSpec,
    pub(crate) effective_code: &'a crate::analysis::EffectiveCodeCatalog,
    pub(crate) interfaces: Option<&'a InterfaceWorkspace>,
    pub(crate) interface_origins: &'a [LinkUnitOriginFact],
    pub(crate) jobs: usize,
    pub(crate) function_fact_store: Option<&'cache mut dyn crate::analysis::FunctionFactStore>,
}

pub(crate) fn generate_project_profile(
    request: ProjectProfileRequest<'_, '_>,
) -> Result<ProjectIrDocuments> {
    let ProjectProfileRequest {
        inputs,
        inventories,
        companions,
        profile,
        svd,
        target,
        effective_code,
        interfaces,
        interface_origins,
        jobs,
        function_fact_store,
    } = request;
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
        LinkedIrAnalysisRequest {
            artifacts: &artifacts,
            inventories: &inventories,
            companions: &companions,
            symbol_prefix: profile.roots.symbol_prefix(),
            include_reachable: profile.include_reachable,
            entry_contract_id: &profile.entry_contract,
            svd,
            target,
            interfaces,
            interface_origins,
            jobs,
            compact_projected_actions: true,
        },
        function_fact_store,
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
        &inventories,
        &companions,
        profile.roots.symbol_prefix(),
        entry_contract,
        &report,
        profile.include_reachable,
    )?;
    let bundle = crate::artifacts::stage_linked_ir_bundle(&profile.output, &document)?;
    tracing::debug!(
        profile = profile.id,
        functions = report.functions.len(),
        analysis_ms = analysis_elapsed.as_millis(),
        render_ms = render_started.elapsed().as_millis(),
        "rendered project linked-IR profile"
    );
    Ok(ProjectIrDocuments {
        bundle,
        sources: artifacts.len(),
        functions: report.functions.len(),
        decode_blockers,
        registers: report.mmio_registers.len(),
        field_candidates,
    })
}

pub(crate) struct LinkedIrAnalysisRequest<'a> {
    pub(crate) artifacts: &'a [IrArtifactInput],
    pub(crate) inventories: &'a [(String, PathBuf)],
    pub(crate) companions: &'a [PathBuf],
    pub(crate) symbol_prefix: &'a str,
    pub(crate) include_reachable: bool,
    pub(crate) entry_contract_id: &'a str,
    pub(crate) svd: &'a MmioMap,
    pub(crate) target: &'a TargetSpec,
    pub(crate) interfaces: Option<&'a InterfaceWorkspace>,
    pub(crate) interface_origins: &'a [LinkUnitOriginFact],
    pub(crate) jobs: usize,
    /// Replace allocation-heavy projected-action structs with their validated
    /// raw JSON representation after semantic indexes have consumed them.
    pub(crate) compact_projected_actions: bool,
}

#[tracing::instrument(name = "build_linked_ir", skip(request, function_fact_store))]
pub(crate) fn analyze(
    request: LinkedIrAnalysisRequest<'_>,
    mut function_fact_store: Option<&mut dyn crate::analysis::FunctionFactStore>,
) -> Result<(EntryContractRef, LinkedIrReport)> {
    let LinkedIrAnalysisRequest {
        artifacts,
        inventories,
        companions,
        symbol_prefix,
        include_reachable,
        entry_contract_id,
        svd,
        target,
        interfaces,
        interface_origins,
        jobs,
        compact_projected_actions,
    } = request;
    let harness = target.knowledge_provider.as_deref();
    let riscv_harness = harnesses::riscv_or_neutral(harness)?;
    let entry_contract = harnesses::entry_contract_or_neutral(harness, entry_contract_id)?;
    validate_artifact_inputs(artifacts, companions)?;
    let mut reports = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let artifact_sha256 = crate::artifact_sha256(&artifact.path)?;
        let source_inventories = inventories
            .iter()
            .filter(|(source, _)| source == &artifact.source)
            .map(|(_, path)| path.as_path())
            .collect::<Vec<_>>();
        let mut resolver = ReferenceResolver::load_all_code_with_reviewed_ranges(
            &artifact.path,
            companions,
            riscv_harness,
            entry_contract,
            &artifact.reviewed_code,
        )?;
        tracing::debug!(
            source = artifact.source,
            rss_kib = ?crate::resource_usage::resident_set_kib(),
            "loaded linked-IR resolver"
        );
        register_projected_direct_semantics(
            &mut resolver,
            &artifact.source,
            &artifact.path,
            &source_inventories,
            interface_origins,
        )?;
        register_projected_origins(
            &mut resolver,
            &artifact.source,
            &artifact.path,
            &source_inventories,
            interface_origins,
        )?;
        tracing::debug!(
            source = artifact.source,
            rss_kib = ?crate::resource_usage::resident_set_kib(),
            "registered projected semantics and exact archive origins"
        );
        if let Some(interfaces) = interfaces {
            register_reviewed_external_calls(
                &mut resolver,
                interfaces,
                &artifact.source,
                interface_origins,
            );
            tracing::debug!(
                source = artifact.source,
                rss_kib = ?crate::resource_usage::resident_set_kib(),
                "registered reviewed interface calls"
            );
        }
        let store = function_fact_store
            .as_mut()
            .map(|store| &mut **store as &mut dyn crate::analysis::FunctionFactStore);
        reports.push(build_linked_ir_for_source_with_cache(
            &resolver,
            svd,
            LinkedIrSourceOptions {
                symbol_prefix,
                source: &artifact.source,
                artifact_sha256: &artifact_sha256,
                namespace_identities: true,
                include_reachable,
                jobs,
                compact_projected_actions,
            },
            store,
        ));
    }
    if artifacts.len() > 1 {
        link_project_calls(&mut reports);
    }
    let report = merge_linked_ir_with_options(reports, jobs, compact_projected_actions);
    if report.functions.is_empty() {
        return Err(crate::Error::invalid(if symbol_prefix.is_empty() {
            "no named code symbols were found in any IR artifact".to_owned()
        } else {
            format!("no named code symbols start with {symbol_prefix:?} in any IR artifact")
        }));
    }
    Ok((entry_contract, report))
}

fn register_projected_origins(
    resolver: &mut ReferenceResolver,
    source: &str,
    linked_artifact: &Path,
    inventories: &[&Path],
    origins: &[LinkUnitOriginFact],
) -> Result<()> {
    let linked_digest = crate::artifact_sha256(linked_artifact)?;
    let mut registered = 0usize;
    for inventory in inventories {
        let inventory_digest = crate::artifact_sha256(inventory)?;
        let candidates = crate::artifact::load_code_symbols(
            inventory,
            "",
            crate::artifact::CodeSymbolSelection::All,
        )?
        .into_iter()
        .map(|symbol| {
            (
                (symbol.member.clone(), symbol.name.clone(), symbol.address),
                symbol,
            )
        })
        .collect::<BTreeMap<_, _>>();
        for origin in origins.iter().filter(|origin| {
            origin.kind == "text"
                && origin.linked_artifact_sha256 == linked_digest
                && origin.origin_artifact_sha256 == inventory_digest
                && origin
                    .linked_sources
                    .iter()
                    .any(|candidate| candidate == source)
                && origin
                    .origin_sources
                    .iter()
                    .any(|candidate| candidate == source)
        }) {
            let Some(linked) = resolver.symbols.iter().find(|symbol| {
                symbol.name == origin.symbol
                    && symbol.member == origin.linked_member
                    && symbol.address == origin.linked_address
            }) else {
                continue;
            };
            let key = (
                origin.origin_member.clone(),
                origin.symbol.clone(),
                origin.origin_address,
            );
            let Some(archive) = candidates.get(&key) else {
                continue;
            };
            let linked = linked.clone();
            register_projected_relocations(resolver, &linked, archive)?;
            resolver.register_projected_origin(&linked, archive.clone());
            registered += 1;
        }
    }
    tracing::debug!(source, registered, "registered exact archive origins");
    Ok(())
}

fn register_projected_relocations(
    resolver: &mut ReferenceResolver,
    linked: &crate::artifact::ArtifactSymbolDefinition,
    origin: &crate::artifact::ArtifactSymbolDefinition,
) -> Result<()> {
    let origin_body = crate::artifact::inspect_function_definition(origin)?;
    let runtime_body = crate::artifact::inspect_function_definition(linked)?;
    let correspondence =
        crate::function_investigation::correspondence::origin_instruction_correspondence(
            &origin_body,
            &runtime_body,
        );

    for item in correspondence {
        let Ok(runtime_address) = u32::try_from(item.runtime_address) else {
            continue;
        };
        let origin_offsets = item
            .origin_offsets
            .iter()
            .filter_map(|offset| u32::try_from(*offset).ok())
            .collect::<Vec<_>>();
        if origin_offsets.len() != item.origin_offsets.len() {
            continue;
        }
        for instruction in origin_body
            .instructions
            .iter()
            .filter(|instruction| item.origin_offsets.contains(&instruction.offset))
        {
            for relocation in origin.relocations.iter().filter(|relocation| {
                u64::from(relocation.address) >= instruction.address
                    && u64::from(relocation.address)
                        < instruction.address + u64::from(instruction.width)
            }) {
                if matches!(
                    relocation.kind,
                    crate::artifact::RelocationKind::Call
                        | crate::artifact::RelocationKind::CallPlt
                ) {
                    resolver.register_projected_call_relocation(
                        linked,
                        runtime_address,
                        &relocation.symbol,
                        relocation.addend,
                    );
                }
                let projected = StructuralProjectedRelocation {
                    origin_member: origin.member.clone(),
                    origin_symbol: origin.name.clone(),
                    origin_offsets: origin_offsets.clone(),
                    kind: relocation.kind,
                    symbol: relocation.symbol.clone(),
                    addend: relocation.addend,
                    correspondence: item.kind,
                };
                let candidates = resolver
                    .pointer_context
                    .projected_relocations
                    .entry(StructuralCallSite::new(linked, runtime_address))
                    .or_default();
                if !candidates.contains(&projected) {
                    candidates.push(projected);
                }
            }
        }
    }
    Ok(())
}

fn register_projected_direct_semantics(
    resolver: &mut ReferenceResolver,
    source: &str,
    linked_artifact: &Path,
    inventories: &[&Path],
    origins: &[LinkUnitOriginFact],
) -> Result<()> {
    let Some(hooks) = resolver.pointer_context.summary_hooks else {
        return Ok(());
    };
    let linked_digest = crate::artifact_sha256(linked_artifact)?;
    for inventory in inventories {
        let inventory_digest = crate::artifact_sha256(inventory)?;
        for origin in origins.iter().filter(|origin| {
            origin.kind == "text"
                && origin.linked_artifact_sha256 == linked_digest
                && origin.origin_artifact_sha256 == inventory_digest
                && origin
                    .linked_sources
                    .iter()
                    .any(|candidate| candidate == source)
                && origin
                    .origin_sources
                    .iter()
                    .any(|candidate| candidate == source)
        }) {
            let Some(linked) = resolver
                .symbols
                .iter()
                .find(|symbol| {
                    symbol.name == origin.symbol
                        && symbol.member == origin.linked_member
                        && symbol.address == origin.linked_address
                })
                .cloned()
            else {
                continue;
            };
            let Some(archive) = crate::artifact::load_code_symbol_exact(
                inventory,
                origin.origin_member.as_deref(),
                &origin.symbol,
                origin.origin_address,
            )?
            else {
                continue;
            };
            let Some(semantic) = (hooks.direct_semantic)(&archive) else {
                continue;
            };
            resolver.register_projected_direct_semantic(&linked, semantic);
            tracing::debug!(
                source,
                symbol = origin.symbol,
                member = ?origin.origin_member,
                semantic = semantic.id,
                "projected exact reviewed semantic from unique archive origin"
            );
        }
    }
    Ok(())
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
    Ok(Some(InterfaceWorkspace::load_with_templates(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        &paths.interface_template_packs,
        target.calling_convention.label(),
        target
            .knowledge_provider
            .as_deref()
            .map(harnesses::contracts)
            .transpose()?,
    )?))
}

#[cfg(test)]
fn unique_observed_internal_target(
    symbols: &[crate::artifact::ArtifactSymbolDefinition],
    assignments: &[crate::interfaces::ResolvedInterfaceAssignment],
) -> Option<u32> {
    let addresses_by_name = observed_symbol_addresses(symbols);
    unique_observed_internal_target_in(&addresses_by_name, assignments)
}

fn observed_symbol_addresses(
    symbols: &[crate::artifact::ArtifactSymbolDefinition],
) -> BTreeMap<String, std::collections::BTreeSet<u32>> {
    let mut addresses_by_name = BTreeMap::<String, std::collections::BTreeSet<u32>>::new();
    for symbol in symbols.iter().filter(|symbol| symbol.addresses_resolved) {
        addresses_by_name
            .entry(symbol.name.clone())
            .or_default()
            .insert(symbol.address as u32);
    }
    addresses_by_name
}

fn unique_observed_internal_target_in(
    addresses_by_name: &BTreeMap<String, std::collections::BTreeSet<u32>>,
    assignments: &[crate::interfaces::ResolvedInterfaceAssignment],
) -> Option<u32> {
    if assignments.is_empty() {
        return None;
    }

    let mut targets = std::collections::BTreeSet::new();
    for assignment in assignments {
        if assignment.target_addend != 0 {
            return None;
        }
        let addresses = addresses_by_name.get(&assignment.target_symbol)?;
        if addresses.len() != 1 {
            return None;
        }
        targets.insert(*addresses.first().expect("one observed internal target"));
    }

    (targets.len() == 1).then(|| {
        *targets
            .first()
            .expect("one unique observed internal target")
    })
}

pub(crate) fn register_reviewed_external_calls(
    resolver: &mut ReferenceResolver,
    interfaces: &InterfaceWorkspace,
    source: &str,
    origins: &[LinkUnitOriginFact],
) {
    let addresses_by_name = observed_symbol_addresses(&resolver.symbols);
    let internal_targets = interfaces
        .bindings()
        .iter()
        .map(|binding| {
            (
                binding.id.as_str(),
                unique_observed_internal_target_in(&addresses_by_name, &binding.assignments),
            )
        })
        .collect::<BTreeMap<_, _>>();
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
                            .insert(definition.address, contract.id.clone());
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
        let internal_target = internal_targets.get(slot.id.as_str()).copied().flatten();
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
                    outputs: model.outputs.clone(),
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
            if let Some(target) = internal_target {
                resolver
                    .pointer_context
                    .reviewed_internal_slots
                    .insert((slot.contract.clone(), offset), target);
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
            if let Some(target) = internal_target {
                resolver.pointer_context.reviewed_internal_calls.insert(
                    StructuralCallSite::from_identity(
                        call.member.clone(),
                        call.function.clone(),
                        call.site,
                    ),
                    target,
                );
            }
        }
    }
    let projected_calls = interfaces.project_link_unit_calls(
        source,
        origins,
        &resolver.pointer_context.projected_relocations,
    );
    tracing::debug!(
        source,
        projected_calls = projected_calls.len(),
        "projected reviewed archive calls onto linked artifact"
    );
    for projected in projected_calls {
        let slot = projected.binding;
        let internal_target = internal_targets.get(slot.id.as_str()).copied().flatten();
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
                    outputs: model.outputs.clone(),
                }
            }),
            tail: projected.tail,
            evidence: ReviewedExternalCallEvidence::ArchiveOriginProjection,
            slot_load_site: projected.slot_load_site,
        };
        let call_site =
            StructuralCallSite::from_identity(projected.member, projected.function, projected.site);
        let candidates = resolver
            .pointer_context
            .reviewed_external_calls
            .entry(call_site.clone())
            .or_default();
        if !candidates.contains(&reviewed) {
            candidates.push(reviewed);
            candidates.sort();
        }
        if let Some(target) = internal_target {
            resolver
                .pointer_context
                .reviewed_internal_calls
                .insert(call_site, target);
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

#[cfg(test)]
mod observed_internal_target_tests {
    use super::*;

    fn symbol(name: &str, address: u64) -> crate::artifact::ArtifactSymbolDefinition {
        crate::artifact::ArtifactSymbolDefinition {
            member: None,
            name: name.to_owned(),
            address,
            bytes: vec![0x82, 0x80],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        }
    }

    fn assignment(target: &str) -> crate::interfaces::ResolvedInterfaceAssignment {
        crate::interfaces::ResolvedInterfaceAssignment {
            member: Some("initializer.o".to_owned()),
            producer: "install_callbacks".to_owned(),
            site: 0x20,
            target_member: Some("implementation.o".to_owned()),
            target_symbol: target.to_owned(),
            target_addend: 0,
        }
    }

    #[test]
    fn observed_target_resolution_uses_assignments_not_reviewed_slot_names() {
        let symbols = vec![symbol("actual_target", 0x2000)];
        assert_eq!(
            unique_observed_internal_target(&symbols, &[assignment("actual_target")]),
            Some(0x2000)
        );
    }

    #[test]
    fn observed_target_resolution_fails_closed_for_runtime_alternatives_or_aliases() {
        let symbols = vec![
            symbol("first", 0x2000),
            symbol("second", 0x3000),
            symbol("aliased", 0x4000),
            symbol("aliased", 0x5000),
        ];
        assert_eq!(
            unique_observed_internal_target(&symbols, &[assignment("first"), assignment("second")]),
            None
        );
        assert_eq!(
            unique_observed_internal_target(&symbols, &[assignment("aliased")]),
            None
        );
    }
}
