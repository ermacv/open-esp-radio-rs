//! Aggregate linked-IR summary section.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render(
    mut output: &mut String,
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
) {
    let root_functions = report
        .functions
        .iter()
        .filter(|function| function.selection == "symbol-prefix-root")
        .count();
    let included_reachable_functions = report.functions.len() - root_functions;
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
    let _ = writeln!(
        &mut output,
        "SUMMARY\tartifacts={}\tfunctions={}\troot-functions={}\tincluded-reachable-functions={}\texported={}\tlocal={}\tmmio-registers={}\tmmio-functions={}\tmmio-access-shapes={}\tmmio-field-candidate-registers={}\tmmio-field-candidates={}\tdirect-mmio-predicates={}\tdirect-mmio-predicate-sources={}\tdelay-functions={}\tdelay-shapes={}\tcontext-functions={}\tcontext-fields={}\tcontext-accesses={}\tmemory-functions={}\tmemory-fields={}\tmemory-accesses={}\tsemantic-operations={}\tsemantic-calls={}\ttrampoline-slots={}\ttrampoline-calls={}\tcomplete={}\tstructured={}\tinternal-calls={}\texternal-calls={}\tcall-argument-shapes={}\tproject-linked-calls={}\tambiguous-project-calls={}\tunresolved-calls={}\tclosed-effect-summaries={}\trecursive-effect-summaries={}\tcomplete-context-projections={}\tprojected-context-fields={}\tprojected-memory-fields={}\texact-return-functions={}\treturn-source-ranges={}\tmmio-return-sources={}\tguard-mmio-links={}\ttransitive-guard-mmio-links={}\tscenario-suggestions={}",
        artifacts.len(),
        report.functions.len(),
        root_functions,
        included_reachable_functions,
        report.exported_functions,
        report.local_functions,
        report.mmio_registers.len(),
        report.mmio_functions,
        report.mmio_access_shapes,
        mmio_field_candidate_registers,
        mmio_field_candidates,
        direct_mmio_predicates,
        direct_mmio_predicate_sources,
        report.delay_functions,
        report.delay_shapes,
        report.context_functions,
        report.context_fields,
        report.context_accesses,
        report.memory_functions,
        report.memory_fields,
        report.memory_accesses,
        report.semantic_boundaries.len(),
        report.semantic_calls,
        report.trampoline_slots.len(),
        report.trampoline_calls,
        report.complete_functions,
        report.structured_functions,
        report.internal_calls,
        report.external_calls,
        report.call_argument_shapes,
        report.project_linked_calls,
        report.ambiguous_project_calls,
        report.unresolved_calls,
        report.closed_effect_summaries,
        report.recursive_effect_summaries,
        report.complete_context_projections,
        report.projected_context_fields,
        report.projected_memory_fields,
        exact_return_functions,
        return_source_ranges,
        mmio_return_sources,
        guard_mmio_links,
        transitive_guard_mmio_links,
        report.scenario_suggestions,
    );
}
