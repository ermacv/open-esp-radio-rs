//! MMIO register index construction and linked-IR report assembly.

use super::*;

mod build;
mod evidence;

use build::build_mmio_registers;
#[cfg(test)]
pub(super) use evidence::{
    MmioRegisterAccumulator, SemanticFieldEvidence, candidate_bit_ranges, record_access_field_mask,
    record_predicate_field_mask, record_semantic_field_link,
};

#[cfg(test)]
pub(super) fn summarize_linked_ir(functions: Vec<LinkedIrFunction>) -> LinkedIrReport {
    summarize_linked_ir_with_jobs(functions, 1)
}

#[cfg(test)]
pub(super) fn summarize_linked_ir_with_jobs(
    functions: Vec<LinkedIrFunction>,
    jobs: usize,
) -> LinkedIrReport {
    summarize_linked_ir_with_options(functions, jobs, false)
}

pub(super) fn summarize_linked_ir_with_options(
    mut functions: Vec<LinkedIrFunction>,
    jobs: usize,
    compact_projected_actions: bool,
) -> LinkedIrReport {
    functions.sort_by(|left, right| left.identity.cmp(&right.identity));
    link_guard_result_mmio_sources(&mut functions);
    populate_effect_summaries(&mut functions, jobs, compact_projected_actions);
    let mmio_functions = functions
        .iter()
        .filter(|function| !function.mmio_accesses.is_empty())
        .count();
    let mmio_access_shapes = functions
        .iter()
        .map(|function| function.mmio_accesses.len())
        .sum();
    let delay_functions = functions
        .iter()
        .filter(|function| !function.delays.is_empty())
        .count();
    let delay_shapes = functions.iter().map(|function| function.delays.len()).sum();
    let mmio_registers = build_mmio_registers(&functions);
    let exported_functions = functions
        .iter()
        .filter(|function| function.binding == "global-or-weak")
        .count();
    let local_functions = functions
        .iter()
        .filter(|function| function.binding == "local")
        .count();
    let context_functions = functions
        .iter()
        .filter(|function| !function.context_accesses.is_empty())
        .count();
    let context_accesses = functions
        .iter()
        .map(|function| function.context_accesses.len())
        .sum();
    let context_fields = functions
        .iter()
        .map(|function| function.context_fields.len())
        .sum();
    let memory_functions = functions
        .iter()
        .filter(|function| !function.memory_accesses.is_empty())
        .count();
    let memory_accesses = functions
        .iter()
        .map(|function| function.memory_accesses.len())
        .sum();
    let memory_fields = functions
        .iter()
        .map(|function| function.memory_fields.len())
        .sum();
    let body_complete_functions = functions
        .iter()
        .filter(|function| function.completeness.body_complete)
        .count();
    let call_targets_complete_functions = functions
        .iter()
        .filter(|function| function.completeness.call_targets_complete)
        .count();
    let transitive_effects_complete_functions = functions
        .iter()
        .filter(|function| function.completeness.transitive_effects_complete)
        .count();
    let executable_complete_functions = functions
        .iter()
        .filter(|function| function.completeness.executable_complete)
        .count();
    let structured_functions = functions
        .iter()
        .filter(|function| function.flow_kind == "structured")
        .count();
    let loop_functions = functions
        .iter()
        .filter(|function| !function.loops.is_empty())
        .count();
    let loop_regions = functions.iter().map(|function| function.loops.len()).sum();
    let counted_loop_candidates = functions
        .iter()
        .flat_map(|function| &function.loops)
        .filter(|region| region.counted.is_some())
        .count();
    let irreducible_loop_regions = functions
        .iter()
        .flat_map(|function| &function.loops)
        .filter(|region| region.kind == artifact::FunctionLoopKind::Irreducible)
        .count();
    let internal_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| matches!(call.kind, "internal" | "indexed-dispatch"))
        .count();
    let indexed_dispatch_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "indexed-dispatch")
        .count();
    let external_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| matches!(call.kind, "external" | "diagnostic"))
        .count();
    let call_argument_shapes = functions
        .iter()
        .flat_map(|function| &function.calls)
        .map(|call| call.argument_shapes)
        .sum();
    let project_linked_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "project-linked")
        .count();
    let ambiguous_project_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved" && call.project_candidates.len() > 1)
        .count();
    let unresolved_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved")
        .count();
    let closed_effect_summaries = functions
        .iter()
        .filter(|function| function.effect_summary.call_graph_closed)
        .count();
    let recursive_effect_summaries = functions
        .iter()
        .filter(|function| function.effect_summary.recursive)
        .count();
    let complete_context_projections = functions
        .iter()
        .filter(|function| function.effect_summary.context_projection_complete)
        .count();
    let projected_context_fields = functions
        .iter()
        .map(|function| function.effect_summary.context_fields.len())
        .sum();
    let projected_memory_fields = functions
        .iter()
        .map(|function| function.effect_summary.memory_fields.len())
        .sum();
    let scenario_suggestions = functions
        .iter()
        .map(|function| function.scenario_suggestions.len())
        .sum();
    let semantic_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.semantic_operation.is_some())
        .map(|call| call.argument_shapes)
        .sum();
    let mut semantic_index =
        BTreeMap::<String, (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(operation) = call.semantic_operation.as_ref() else {
                continue;
            };
            let entry = semantic_index.entry(operation.clone()).or_default();
            entry.0 += call.argument_shapes;
            entry.1.insert(function.identity.clone());
            entry.2.insert(call.target.clone());
            if let Some(replacement) = call.replacement_hint.as_ref() {
                entry.3.insert(replacement.clone());
            }
        }
    }
    let semantic_boundaries = semantic_index
        .into_iter()
        .map(
            |(operation, (call_shapes, functions, targets, replacement_hints))| SemanticBoundary {
                operation,
                call_shapes,
                functions: functions.into_iter().collect(),
                targets: targets.into_iter().collect(),
                replacement_hints: replacement_hints.into_iter().collect(),
            },
        )
        .collect();
    let mut trampoline_index =
        BTreeMap::<LinkedTrampoline, (Vec<LinkedCallArgument>, usize, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(trampoline) = call.trampoline.as_ref() else {
                continue;
            };
            let abi_arguments = call
                .typed_arguments
                .iter()
                .cloned()
                .map(|mut argument| {
                    argument.value.clear();
                    argument
                })
                .collect::<Vec<_>>();
            let entry = trampoline_index
                .entry(trampoline.clone())
                .or_insert_with(|| (abi_arguments, 0, BTreeSet::new()));
            entry.1 += call.argument_shapes;
            entry.2.insert(function.identity.clone());
        }
    }
    let trampoline_calls = trampoline_index.values().map(|entry| entry.1).sum();
    let trampoline_slots = trampoline_index
        .into_iter()
        .map(
            |(trampoline, (arguments, call_shapes, functions))| LinkedTrampolineSlot {
                trampoline,
                arguments,
                call_shapes,
                functions: functions.into_iter().collect(),
            },
        )
        .collect();

    LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions,
        mmio_access_shapes,
        delay_functions,
        delay_shapes,
        semantic_boundaries,
        semantic_calls,
        trampoline_slots,
        trampoline_calls,
        exported_functions,
        local_functions,
        context_functions,
        context_accesses,
        context_fields,
        memory_functions,
        memory_accesses,
        memory_fields,
        body_complete_functions,
        call_targets_complete_functions,
        transitive_effects_complete_functions,
        executable_complete_functions,
        structured_functions,
        loop_functions,
        loop_regions,
        counted_loop_candidates,
        irreducible_loop_regions,
        internal_calls,
        indexed_dispatch_calls,
        external_calls,
        call_argument_shapes,
        project_linked_calls,
        ambiguous_project_calls,
        unresolved_calls,
        closed_effect_summaries,
        recursive_effect_summaries,
        complete_context_projections,
        projected_context_fields,
        projected_memory_fields,
        scenario_suggestions,
    }
}
