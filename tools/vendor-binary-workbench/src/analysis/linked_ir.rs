//! Best-effort linked function/call IR for manual vendor-code analysis.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use indicatif::ProgressStyle;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::{
    BitSource, BranchCondition, BranchOperation, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, ExpressionOperation, ExternalReturnModel, FunctionAnalysis,
    MemoryAccess, MmioMap, ObservableEvent, ReferenceResolver, SymbolicValue, artifact, direct,
};

const MAX_CALL_GRAPH_STATES: usize = 127;
const MAX_CALL_GRAPH_BRANCH_DECISIONS: usize = 12;
const MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE: usize = 4_096;
const MAX_CALL_GRAPH_EVENTS_PER_TRACE: usize = 1_024;
const MAX_CONTEXT_PROJECTION_STATES: usize = 4_096;
const LINKED_CONTEXT_ARGUMENTS: u8 = 16;

mod model;

pub(crate) use model::*;

mod identity;

use identity::*;
mod pseudo;

use pseudo::*;
mod calls;

use calls::*;
pub(crate) use calls::{effective_branch_operation, format_guard_path, format_guard_paths};
mod direct_trace;

use direct_trace::*;
mod scenario_suggestions;

use scenario_suggestions::*;
fn nested_path(path: &str, scope: &str) -> String {
    format!("{path} / {scope}")
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xff,
        16 => 0xffff,
        32 => u32::MAX,
        _ => 0,
    }
}

fn bit_range_mask(lsb: u8, width: u8) -> u32 {
    if width == 32 {
        u32::MAX
    } else {
        ((1_u32 << width) - 1) << lsb
    }
}

mod provenance;

use provenance::*;

mod effects;

use effects::*;
fn calls_for_trace(
    trace: &FunctionAnalysis,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) -> Vec<LinkedCall> {
    let mut calls = BTreeSet::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_calls_from_flow(flow, resolver, identities, &mut calls);
    } else {
        for event in &trace.reference_events {
            collect_calls_from_event(event, resolver, identities, &mut calls);
        }
    }
    compact_calls(calls)
}

fn annotate_direct_semantic_calls(
    calls: &mut [LinkedCall],
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) {
    let Some(hooks) = resolver.pointer_context.summary_hooks else {
        return;
    };
    for call in calls
        .iter_mut()
        .filter(|call| call.kind == "internal" && call.semantic_operation.is_none())
    {
        let Some(symbol) = identities.selectable_symbol(&call.target) else {
            continue;
        };
        let Some(function) = (hooks.direct_semantic)(symbol) else {
            continue;
        };
        debug_assert_eq!(
            function.semantic.arguments.len(),
            usize::from(function.argument_count),
            "direct semantic ABI arity must match its typed arguments"
        );
        call.semantics = Some(format!(
            "reviewed direct semantic function={} args={} operation={}",
            function.c_name, function.argument_count, function.semantic.operation,
        ));
        call.semantic_operation = Some(function.semantic.operation.to_owned());
        call.semantic_contract = Some(LinkedSemanticContract {
            source: "reviewed-internal-function",
            id: function.id.to_owned(),
            evidence: function.evidence.to_owned(),
            event_dispatch: linked_event_dispatch_contract(function.semantic),
        });
        call.replacement_hint = function.semantic.replacement.map(str::to_owned);
        call.typed_arguments = direct_semantic_typed_arguments(function, &call.arguments);
    }
}

pub(crate) fn build_linked_ir_for_source(
    resolver: &ReferenceResolver,
    symbol_prefix: &str,
    svd: &MmioMap,
    source: &str,
    namespace_identities: bool,
    include_reachable: bool,
) -> LinkedIrReport {
    let mut functions = Vec::new();
    let identities = IrIdentityCatalog::new(resolver, namespace_identities.then_some(source));
    let mut scheduled = BTreeSet::<SymbolKey>::new();
    let mut pending = VecDeque::new();
    for symbol in resolver
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with(symbol_prefix))
    {
        if scheduled.insert(symbol_key(symbol)) {
            pending.push_back(symbol.clone());
        }
    }

    let progress = linked_ir_progress_span(source, scheduled.len());

    while let Some(symbol) = pending.pop_front() {
        let selection = if symbol.name.starts_with(symbol_prefix) {
            "symbol-prefix-root"
        } else {
            "reachable-internal"
        };
        let function_identity = identities.symbol(&symbol);
        let binding = if resolver.symbol_is_exported(&symbol) {
            "global-or-weak"
        } else {
            "local"
        };
        let DirectCallGraph {
            calls: direct_calls,
            direct_mmio_predicates,
            blockers,
        } = explore_direct_calls(&symbol, resolver, &identities, svd);
        let direct_mmio_predicates = direct_mmio_predicates.into_iter().collect::<Vec<_>>();
        if include_reachable {
            for call in direct_calls.iter().filter(|call| call.kind == "internal") {
                let Some(callee) = identities.selectable_symbol(&call.target) else {
                    continue;
                };
                if scheduled.insert(symbol_key(callee)) {
                    pending.push_back(callee.clone());
                    progress.pb_inc_length(1);
                }
            }
        }
        let call_graph_messages = blockers.into_iter().collect::<Vec<_>>();
        let call_graph_diagnostics = compact_diagnostics(&call_graph_messages);
        let call_graph_blockers = rendered_diagnostics(&call_graph_diagnostics);
        match resolver.trace_symbol_bounded(
            &symbol,
            svd,
            direct::StructuralTraceBudget {
                max_instruction_steps: MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE,
                max_events: MAX_CALL_GRAPH_EVENTS_PER_TRACE,
            },
        ) {
            Ok(trace) => {
                let memory_accesses = memory_object_accesses_for_trace(&trace);
                let memory_fields = memory_object_fields_for_accesses(&memory_accesses);
                let context_accesses = context_accesses_for_memory_objects(&memory_accesses);
                let context_fields = context_fields_for_accesses(&context_accesses);
                let mmio_accesses = mmio_accesses_for_trace(&trace);
                let delays = delays_for_trace(&trace);
                let scenario_suggestions =
                    scenario_suggestions(Some(&trace), &direct_mmio_predicates, &mmio_accesses);
                let return_call_results = trace_call_results(&trace, &identities);
                let return_provenance =
                    return_provenance(&trace.return_value, &return_call_results, svd);
                let mut calls = if direct_calls.is_empty() {
                    calls_for_trace(&trace, resolver, &identities)
                } else {
                    compact_calls(direct_calls)
                };
                annotate_direct_semantic_calls(&mut calls, resolver, &identities);
                let flow_kind = if trace.reference_flow.is_some() {
                    "structured"
                } else if trace.is_reference_eligible() {
                    "linear"
                } else {
                    "partial"
                };
                let direct_diagnostics = compact_diagnostics(&trace.blockers);
                let reference_diagnostics = compact_diagnostics(&trace.reference_blockers);
                let direct_blockers = rendered_diagnostics(&direct_diagnostics);
                let reference_blockers = rendered_diagnostics(&reference_diagnostics);
                let pseudo = render_pseudo(
                    &function_identity,
                    &trace,
                    &calls,
                    &direct_blockers,
                    &reference_blockers,
                    &call_graph_blockers,
                );
                functions.push(LinkedIrFunction {
                    source: source.to_owned(),
                    identity: function_identity.clone(),
                    selection,
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                    binding,
                    address: symbol.addresses_resolved.then_some(symbol.address as u32),
                    object_offset: symbol.address as u32,
                    size: symbol.bytes.len(),
                    flow_kind,
                    complete: trace.is_reference_eligible(),
                    exact: trace.is_exact(),
                    return_value: trace.return_value.canonical(),
                    return_provenance,
                    dependencies: trace
                        .reference_dependencies
                        .iter()
                        .map(|dependency| {
                            if namespace_identities {
                                format!("{source}::{dependency}")
                            } else {
                                dependency.clone()
                            }
                        })
                        .collect(),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses,
                    delays,
                    context_accesses,
                    context_fields,
                    memory_accesses,
                    memory_fields,
                    scenario_suggestions,
                    effect_summary: LinkedEffectSummary::default(),
                    call_graph_diagnostics,
                    direct_diagnostics,
                    reference_diagnostics,
                    call_graph_blockers,
                    direct_blockers,
                    reference_blockers,
                    pseudo,
                });
            }
            Err(error) => {
                let direct_diagnostics = vec![compact_diagnostic(&error.to_string())];
                let direct_blockers = rendered_diagnostics(&direct_diagnostics);
                let mut calls = compact_calls(direct_calls);
                annotate_direct_semantic_calls(&mut calls, resolver, &identities);
                let scenario_suggestions = scenario_suggestions(None, &direct_mmio_predicates, &[]);
                functions.push(LinkedIrFunction {
                    source: source.to_owned(),
                    identity: function_identity.clone(),
                    selection,
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                    binding,
                    address: symbol.addresses_resolved.then_some(symbol.address as u32),
                    object_offset: symbol.address as u32,
                    size: symbol.bytes.len(),
                    flow_kind: "unavailable",
                    complete: false,
                    exact: false,
                    return_value: "unknown".to_owned(),
                    return_provenance: return_provenance(
                        &SymbolicValue::Unknown,
                        &BTreeMap::new(),
                        svd,
                    ),
                    dependencies: Vec::new(),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses: Vec::new(),
                    delays: Vec::new(),
                    context_accesses: Vec::new(),
                    context_fields: Vec::new(),
                    memory_accesses: Vec::new(),
                    memory_fields: Vec::new(),
                    scenario_suggestions,
                    effect_summary: LinkedEffectSummary::default(),
                    call_graph_diagnostics,
                    direct_diagnostics,
                    reference_diagnostics: Vec::new(),
                    call_graph_blockers,
                    direct_blockers,
                    reference_blockers: Vec::new(),
                    pseudo: format!(
                        "// vendor symbol: {function_identity}\n// DECODE-BLOCKER: {error}\nfn {}(args: [u32; 16]) -> u32 {{ unknown }}\n",
                        pseudo_identifier(&function_identity)
                    ),
                });
            }
        }
        progress.pb_inc(1);
        progress.pb_set_message(&format!("{source}: completed {function_identity}"));
    }

    progress.pb_set_finish_message(&format!("{source}: analyzed {} functions", functions.len()));

    summarize_linked_ir(functions)
}

fn linked_ir_progress_span(source: &str, functions: usize) -> tracing::Span {
    let span = tracing::info_span!(
        "linked_ir_source",
        indicatif.pb_show = tracing::field::Empty,
        source,
        functions,
    );
    span.pb_set_style(
        &ProgressStyle::with_template(
            "{spinner:.cyan} [{elapsed_precise}] {pos:>5}/{len:<5} {msg}",
        )
        .expect("static linked-IR progress template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    span.pb_set_length(functions as u64);
    span.pb_set_message(&format!("{source}: analyzing functions"));
    span.pb_start();
    span
}

pub(crate) fn merge_linked_ir(reports: Vec<LinkedIrReport>) -> LinkedIrReport {
    let functions = reports
        .into_iter()
        .flat_map(|report| report.functions)
        .collect();
    summarize_linked_ir(functions)
}

pub(crate) fn link_project_calls(reports: &mut [LinkedIrReport]) {
    let mut exported_definitions = BTreeMap::<String, BTreeSet<String>>::new();
    for function in reports.iter().flat_map(|report| &report.functions) {
        if function.binding == "global-or-weak" {
            exported_definitions
                .entry(function.symbol.clone())
                .or_default()
                .insert(function.identity.clone());
        }
    }

    for function in reports.iter_mut().flat_map(|report| &mut report.functions) {
        let mut project_notes = Vec::new();
        let mut linked_dependencies = Vec::new();
        for call in &mut function.calls {
            let Some(symbol) = call.project_symbol.as_ref() else {
                continue;
            };
            let candidates = exported_definitions
                .get(symbol)
                .map(|definitions| definitions.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            call.project_candidates = candidates.clone();
            match candidates.as_slice() {
                [target] => {
                    call.kind = "project-linked";
                    call.target = target.clone();
                    call.semantics = Some(
                        "unique exported project definition; edge linked without substituting callee arguments, returns or addresses"
                            .to_owned(),
                    );
                    linked_dependencies.push(target.clone());
                    project_notes.push(format!(
                        "// PROJECT-LINKED-CALL: {symbol} -> {target}; reachable effects are inventoried without argument substitution"
                    ));
                }
                [] => {
                    call.semantics = Some(
                        "unresolved project call; no exported definition was found".to_owned(),
                    );
                }
                _ => {
                    call.semantics = Some(
                        "ambiguous project call; multiple exported definitions were found"
                            .to_owned(),
                    );
                    project_notes.push(format!(
                        "// PROJECT-AMBIGUOUS-CALL: {symbol} -> {}",
                        candidates.join(" | ")
                    ));
                }
            }
        }
        function.dependencies.extend(linked_dependencies);
        function.dependencies.sort();
        function.dependencies.dedup();
        function.calls.sort();
        if !project_notes.is_empty() {
            project_notes.sort();
            project_notes.dedup();
            function.pseudo = format!("{}\n{}", project_notes.join("\n"), function.pseudo);
        }
    }
}

mod summary;

use summary::*;

mod register_index;

use register_index::*;

#[cfg(test)]
mod tests;
