//! Typed JSON linked-IR report rendering.

use std::{path::Path, path::PathBuf};

use serde::Serialize;

use super::*;

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
}

#[derive(Serialize)]
struct ReportSummary {
    artifacts: usize,
    functions: usize,
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
    exact_return_functions: usize,
    return_source_ranges: usize,
    mmio_return_sources: usize,
    guard_mmio_links: usize,
    transitive_guard_mmio_links: usize,
}

impl ReportSummary {
    fn new(artifacts: usize, report: &LinkedIrReport) -> Self {
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
            artifacts,
            functions: report.functions.len(),
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
            exact_return_functions,
            return_source_ranges,
            mmio_return_sources,
            guard_mmio_links,
            transitive_guard_mmio_links,
        }
    }
}

#[derive(Serialize)]
pub(super) struct LinkedIrDocument<'a> {
    schema_version: u32,
    command: &'static str,
    analysis_mode: &'static str,
    linkage_mode: &'static str,
    project_call_linkage: &'static str,
    selection_mode: &'static str,
    include_reachable: bool,
    effect_summary_mode: &'static str,
    context_projection_mode: &'static str,
    semantic_action_mode: &'static str,
    event_dispatch_mode: &'static str,
    event_dispatch_effect_completeness_claim: bool,
    event_dispatch_receiver_inference_mode: &'static str,
    mmio_field_candidate_mode: &'static str,
    direct_mmio_predicate_completeness_claim: bool,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    publications: Vec<crate::cli::output::Publication>,
}

pub(super) fn document<'a>(
    artifacts: &'a [IrArtifactInput],
    companions: &[PathBuf],
    symbol_prefix: &'a str,
    entry_contract: EntryContractRef,
    report: &'a LinkedIrReport,
    include_reachable: bool,
    publications: Vec<crate::cli::output::Publication>,
) -> Result<LinkedIrDocument<'a>> {
    Ok(LinkedIrDocument {
        schema_version: 32,
        command: "ir export",
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
        selection_mode: if include_reachable {
            "symbol-prefix-with-reachable-internal-callees"
        } else {
            "symbol-prefix-only"
        },
        include_reachable,
        effect_summary_mode: "reachable-inventory-origin-preserving",
        context_projection_mode: "affine-simple-call-paths",
        semantic_action_mode: "lexical-site-paths-factorized-cfg-guards-affine-root-bindings",
        event_dispatch_mode: "reviewed-contract-declared-role-projection",
        event_dispatch_effect_completeness_claim: false,
        event_dispatch_receiver_inference_mode: "none",
        mmio_field_candidate_mode: "contiguous-subregister-write-poll-and-direct-guard-evidence",
        direct_mmio_predicate_completeness_claim: false,
        mmio_field_semantics_claim: false,
        cfg_guard_completeness_claim: false,
        completeness_claim: false,
        artifacts: artifacts
            .iter()
            .map(|artifact| {
                Ok(SourceArtifact {
                    source: &artifact.source,
                    artifact: ArtifactIdentity::load(&artifact.path)?,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        companions: companions
            .iter()
            .map(|path| ArtifactIdentity::load(path))
            .collect::<Result<Vec<_>>>()?,
        symbol_prefix,
        entry_contract: entry_contract.id(),
        summary: ReportSummary::new(artifacts.len(), report),
        mmio_registers: &report.mmio_registers,
        semantic_boundaries: &report.semantic_boundaries,
        trampoline_slots: &report.trampoline_slots,
        functions: &report.functions,
        publications,
    })
}

pub(super) fn render_document(document: &LinkedIrDocument<'_>) -> Result<String> {
    Ok(serde_json::to_string_pretty(document)? + "\n")
}

pub(super) fn write_json_report(path: &Path, document: &LinkedIrDocument<'_>) -> Result<()> {
    fs::write(path, render_document(document)?)?;
    Ok(())
}
