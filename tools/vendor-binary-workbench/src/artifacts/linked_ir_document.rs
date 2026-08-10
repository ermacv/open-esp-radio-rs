//! Typed JSON linked-IR report rendering.

use std::{path::Path, path::PathBuf};

use serde::Serialize;

use crate::{
    EntryContractRef, LinkedIrFunction, LinkedIrReport, LinkedMmioRegister, LinkedTrampolineSlot,
    Result, SemanticBoundary,
    linked_ir_export::{IrArtifactInput, field_candidate_summary, provenance_summary},
};

#[derive(Serialize)]
struct ArtifactIdentity {
    path: String,
    sha256: String,
}

impl ArtifactIdentity {
    fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.display().to_string(),
            sha256: crate::artifact_sha256(path)?,
        })
    }
}

#[derive(Serialize)]
struct SourceArtifact<'a> {
    source: &'a str,
    artifact: ArtifactIdentity,
    reviewed_code_boundaries: Vec<ReviewedCodeBoundaryDocument<'a>>,
}

#[derive(Serialize)]
struct ReviewedCodeBoundaryDocument<'a> {
    member: &'a Option<String>,
    section: &'a str,
    name: &'a str,
    start_offset: String,
    end_offset: String,
}

#[derive(Serialize)]
struct ReportSummary {
    artifacts: usize,
    reviewed_code_boundaries: usize,
    functions: usize,
    decode_blocker_functions: usize,
    decode_blockers: usize,
    root_functions: usize,
    included_reachable_functions: usize,
    exported: usize,
    local: usize,
    mmio_registers: usize,
    mmio_functions: usize,
    mmio_access_shapes: usize,
    mmio_field_candidate_registers: usize,
    mmio_field_candidates: usize,
    direct_mmio_predicates: usize,
    direct_mmio_predicate_sources: usize,
    delay_functions: usize,
    delay_shapes: usize,
    context_functions: usize,
    context_fields: usize,
    context_accesses: usize,
    memory_functions: usize,
    memory_fields: usize,
    memory_accesses: usize,
    semantic_operations: usize,
    semantic_calls: usize,
    trampoline_slots: usize,
    trampoline_calls: usize,
    complete: usize,
    structured: usize,
    internal_calls: usize,
    external_calls: usize,
    call_argument_shapes: usize,
    project_linked_calls: usize,
    ambiguous_project_calls: usize,
    unresolved_calls: usize,
    closed_effect_summaries: usize,
    recursive_effect_summaries: usize,
    complete_context_projections: usize,
    projected_context_fields: usize,
    projected_memory_fields: usize,
    exact_return_functions: usize,
    return_source_ranges: usize,
    mmio_return_sources: usize,
    guard_mmio_links: usize,
    transitive_guard_mmio_links: usize,
    scenario_suggestions: usize,
}

impl ReportSummary {
    fn new(artifacts: &[IrArtifactInput], report: &LinkedIrReport) -> Self {
        let root_functions = report
            .functions
            .iter()
            .filter(|function| function.selection == "symbol-prefix-root")
            .count();
        let (
            exact_return_functions,
            return_source_ranges,
            mmio_return_sources,
            guard_mmio_links,
            transitive_guard_mmio_links,
        ) = provenance_summary(report);
        let (
            mmio_field_candidate_registers,
            mmio_field_candidates,
            direct_mmio_predicates,
            direct_mmio_predicate_sources,
        ) = field_candidate_summary(report);
        Self {
            artifacts: artifacts.len(),
            reviewed_code_boundaries: artifacts
                .iter()
                .map(|artifact| artifact.reviewed_code.len())
                .sum(),
            functions: report.functions.len(),
            decode_blocker_functions: report
                .functions
                .iter()
                .filter(|function| !function.decode_blockers.is_empty())
                .count(),
            decode_blockers: report
                .functions
                .iter()
                .map(|function| function.decode_blockers.len())
                .sum(),
            root_functions,
            included_reachable_functions: report.functions.len() - root_functions,
            exported: report.exported_functions,
            local: report.local_functions,
            mmio_registers: report.mmio_registers.len(),
            mmio_functions: report.mmio_functions,
            mmio_access_shapes: report.mmio_access_shapes,
            mmio_field_candidate_registers,
            mmio_field_candidates,
            direct_mmio_predicates,
            direct_mmio_predicate_sources,
            delay_functions: report.delay_functions,
            delay_shapes: report.delay_shapes,
            context_functions: report.context_functions,
            context_fields: report.context_fields,
            context_accesses: report.context_accesses,
            memory_functions: report.memory_functions,
            memory_fields: report.memory_fields,
            memory_accesses: report.memory_accesses,
            semantic_operations: report.semantic_boundaries.len(),
            semantic_calls: report.semantic_calls,
            trampoline_slots: report.trampoline_slots.len(),
            trampoline_calls: report.trampoline_calls,
            complete: report.complete_functions,
            structured: report.structured_functions,
            internal_calls: report.internal_calls,
            external_calls: report.external_calls,
            call_argument_shapes: report.call_argument_shapes,
            project_linked_calls: report.project_linked_calls,
            ambiguous_project_calls: report.ambiguous_project_calls,
            unresolved_calls: report.unresolved_calls,
            closed_effect_summaries: report.closed_effect_summaries,
            recursive_effect_summaries: report.recursive_effect_summaries,
            complete_context_projections: report.complete_context_projections,
            projected_context_fields: report.projected_context_fields,
            projected_memory_fields: report.projected_memory_fields,
            exact_return_functions,
            return_source_ranges,
            mmio_return_sources,
            guard_mmio_links,
            transitive_guard_mmio_links,
            scenario_suggestions: report.scenario_suggestions,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct LinkedIrDocument<'a> {
    schema_version: u32,
    command: &'static str,
    analysis_mode: &'static str,
    linkage_mode: &'static str,
    project_call_linkage: &'static str,
    selection_mode: &'static str,
    include_reachable: bool,
    effect_summary_mode: &'static str,
    context_projection_mode: &'static str,
    memory_object_mode: &'static str,
    semantic_action_mode: &'static str,
    event_dispatch_mode: &'static str,
    event_dispatch_effect_completeness_claim: bool,
    event_dispatch_receiver_inference_mode: &'static str,
    mmio_field_candidate_mode: &'static str,
    direct_mmio_predicate_completeness_claim: bool,
    scenario_suggestion_mode: &'static str,
    scenario_suggestion_proof_claim: bool,
    mmio_field_semantics_claim: bool,
    cfg_guard_completeness_claim: bool,
    completeness_claim: bool,
    artifacts: Vec<SourceArtifact<'a>>,
    companions: Vec<ArtifactIdentity>,
    symbol_prefix: &'a str,
    entry_contract: &'a str,
    summary: ReportSummary,
    mmio_registers: &'a [LinkedMmioRegister],
    semantic_boundaries: &'a [SemanticBoundary],
    trampoline_slots: &'a [LinkedTrampolineSlot],
    functions: &'a [LinkedIrFunction],
}

pub(crate) fn build_linked_ir_document<'a>(
    artifacts: &'a [IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &'a str,
    entry_contract: EntryContractRef,
    report: &'a LinkedIrReport,
    include_reachable: bool,
) -> Result<LinkedIrDocument<'a>> {
    Ok(LinkedIrDocument {
        schema_version: crate::artifacts::LINKED_IR.version,
        command: crate::artifacts::LINKED_IR.command,
        analysis_mode: "best-effort",
        linkage_mode: if artifacts.len() > 1 {
            "independent-artifacts"
        } else {
            "primary-with-companions"
        },
        project_call_linkage: if artifacts.len() > 1 {
            "unique-exported-symbol-only"
        } else {
            "primary-resolver"
        },
        selection_mode: match (symbol_prefix.is_empty(), include_reachable) {
            (true, true) => "all-symbols-with-reachable-internal-callees",
            (true, false) => "all-symbols-only",
            (false, true) => "symbol-prefix-with-reachable-internal-callees",
            (false, false) => "symbol-prefix-only",
        },
        include_reachable,
        effect_summary_mode: "reachable-inventory-origin-preserving",
        context_projection_mode: "affine-simple-call-paths",
        memory_object_mode: "affine-argument-and-relocated-symbols",
        semantic_action_mode: "lexical-site-paths-factorized-cfg-guards-affine-root-bindings",
        event_dispatch_mode: "reviewed-contract-declared-role-projection",
        event_dispatch_effect_completeness_claim: false,
        event_dispatch_receiver_inference_mode: "none",
        mmio_field_candidate_mode: "contiguous-subregister-write-poll-and-direct-guard-evidence",
        direct_mmio_predicate_completeness_claim: false,
        scenario_suggestion_mode: "structural-candidates-require-concrete-replay",
        scenario_suggestion_proof_claim: false,
        mmio_field_semantics_claim: false,
        cfg_guard_completeness_claim: false,
        completeness_claim: false,
        artifacts: artifacts
            .iter()
            .map(|artifact| {
                Ok(SourceArtifact {
                    source: &artifact.source,
                    artifact: ArtifactIdentity::load(&artifact.path)?,
                    reviewed_code_boundaries: artifact
                        .reviewed_code
                        .iter()
                        .map(|range| ReviewedCodeBoundaryDocument {
                            member: &range.member,
                            section: &range.section,
                            name: &range.name,
                            start_offset: format!("{:#x}", range.start_offset),
                            end_offset: format!("{:#x}", range.end_offset),
                        })
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        companions: companions
            .iter()
            .map(|path| ArtifactIdentity::load(path))
            .collect::<Result<Vec<_>>>()?,
        symbol_prefix,
        entry_contract: entry_contract.id(),
        summary: ReportSummary::new(artifacts, report),
        mmio_registers: &report.mmio_registers,
        semantic_boundaries: &report.semantic_boundaries,
        trampoline_slots: &report.trampoline_slots,
        functions: &report.functions,
    })
}

pub(crate) fn render_linked_ir(document: &LinkedIrDocument<'_>) -> Result<String> {
    Ok(serde_json::to_string(document)? + "\n")
}

pub(crate) fn write_linked_ir(path: &Path, document: &LinkedIrDocument<'_>) -> Result<()> {
    std::fs::write(path, render_linked_ir(document)?)?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn render_linked_ir_fixture(
    functions: Vec<LinkedIrFunction>,
    mmio_registers: Vec<LinkedMmioRegister>,
) -> String {
    use crate::EntryContractSpec;

    static ENTRY: EntryContractSpec = EntryContractSpec {
        id: "none",
        function_table: None,
        pointer_symbols: &[],
        data_pointer_binding: None,
    };
    let report = LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions: 0,
        mmio_access_shapes: 0,
        delay_functions: 0,
        delay_shapes: 0,
        semantic_boundaries: Vec::new(),
        semantic_calls: 0,
        trampoline_slots: Vec::new(),
        trampoline_calls: 0,
        exported_functions: 0,
        local_functions: 0,
        context_functions: 0,
        context_accesses: 0,
        context_fields: 0,
        memory_functions: 0,
        memory_accesses: 0,
        memory_fields: 0,
        complete_functions: 0,
        structured_functions: 0,
        internal_calls: 0,
        external_calls: 0,
        call_argument_shapes: 0,
        project_linked_calls: 0,
        ambiguous_project_calls: 0,
        unresolved_calls: 0,
        closed_effect_summaries: 0,
        recursive_effect_summaries: 0,
        complete_context_projections: 0,
        projected_context_fields: 0,
        projected_memory_fields: 0,
        scenario_suggestions: 0,
    };
    let document = build_linked_ir_document(
        &[],
        &[],
        "",
        crate::EntryContractRef::new(&ENTRY),
        &report,
        false,
    )
    .unwrap();
    render_linked_ir(&document).unwrap()
}
