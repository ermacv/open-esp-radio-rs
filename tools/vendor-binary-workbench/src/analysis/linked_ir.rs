//! Best-effort linked function/call IR for manual vendor-code analysis.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::mpsc::sync_channel,
    thread,
};

use indicatif::ProgressStyle;
use tracing_indicatif::span_ext::IndicatifSpanExt;

use crate::{
    BitSource, BranchCondition, BranchOperation, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, ExpressionOperation, ExternalOutputModel, ExternalReturnModel,
    FunctionAnalysis, MemoryAccess, MmioMap, ObservableEvent, ReferenceResolver,
    ReviewedExternalCall, ReviewedExternalCallExecutionModel, SymbolicValue, artifact, direct,
    external_result_call_token,
};

const MAX_CALL_GRAPH_STATES: usize = 127;
const MAX_CALL_GRAPH_BRANCH_DECISIONS: usize = 12;
const MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE: usize = 4_096;
const MAX_CALL_GRAPH_EVENTS_PER_TRACE: usize = 1_024;
const MAX_CONTEXT_PROJECTION_STATES: usize = 4_096;
const LINKED_CONTEXT_ARGUMENTS: u8 = 16;
const MAX_LINKED_IR_JOBS: usize = 8;
const LINKED_IR_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

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

fn relocation_kind(kind: artifact::RelocationKind) -> &'static str {
    match kind {
        artifact::RelocationKind::GotHi20 => "got-hi20",
        artifact::RelocationKind::Hi20 => "hi20",
        artifact::RelocationKind::Lo12I => "lo12-i",
        artifact::RelocationKind::Lo12S => "lo12-s",
        artifact::RelocationKind::PcRelHi20 => "pc-relative-hi20",
        artifact::RelocationKind::PcRelLo12I => "pc-relative-lo12-i",
        artifact::RelocationKind::PcRelLo12S => "pc-relative-lo12-s",
        artifact::RelocationKind::GotPcRelLo12I => "got-pc-relative-lo12-i",
        artifact::RelocationKind::Call => "call",
        artifact::RelocationKind::CallPlt => "call-plt",
    }
}

fn projected_relocations(
    owner: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
) -> Vec<LinkedProjectedRelocation> {
    let mut facts = resolver
        .pointer_context
        .projected_relocations
        .iter()
        .filter(|(site, _)| site.belongs_to(owner))
        .flat_map(|(site, relocations)| {
            relocations
                .iter()
                .map(move |relocation| LinkedProjectedRelocation {
                    site: site.address(),
                    origin_member: relocation.origin_member.clone(),
                    origin_symbol: relocation.origin_symbol.clone(),
                    origin_offsets: relocation.origin_offsets.clone(),
                    kind: relocation_kind(relocation.kind),
                    symbol: relocation.symbol.clone(),
                    addend: relocation.addend,
                    correspondence: relocation.correspondence,
                })
        })
        .collect::<Vec<_>>();
    facts.sort();
    facts.dedup();
    facts
}

fn add_lossless_relocation_calls(
    calls: &mut BTreeSet<LinkedCall>,
    owner: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) {
    let existing_sites = calls
        .iter()
        .filter_map(|call| call.site)
        .collect::<BTreeSet<_>>();
    for (site, (symbol, target)) in resolver
        .relocated_calls
        .iter()
        .filter(|(site, _)| site.belongs_to(owner) && !existing_sites.contains(&site.address()))
    {
        calls.insert(LinkedCall {
            kind: "structural-relocation",
            target: target.map_or_else(|| symbol.clone(), |target| identities.target(target)),
            site: Some(site.address()),
            tail: false,
            result_modeled: false,
            execution_model: None,
            semantics: Some(
                "lossless direct-call relocation; structural reachability only".to_owned(),
            ),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: target.is_none().then(|| symbol.clone()),
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 0,
            arguments: Vec::new(),
            argument_bindings: Vec::new(),
            typed_arguments: Vec::new(),
            guard_paths: None,
        });
    }
}

fn add_projected_origin_calls(
    calls: &mut BTreeSet<LinkedCall>,
    owner: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) -> crate::Result<()> {
    let Some(origin) = resolver.projected_origin(owner) else {
        return Ok(());
    };
    let origin_body = artifact::inspect_function_definition(origin)?;
    let runtime_body = artifact::inspect_function_definition(owner)?;
    let correspondence =
        crate::function_investigation::correspondence::origin_instruction_correspondence(
            &origin_body,
            &runtime_body,
        );
    let mut existing_sites = calls
        .iter()
        .filter_map(|call| call.site)
        .collect::<BTreeSet<_>>();
    for instruction in &origin_body.instructions {
        for relocation in instruction
            .relocations
            .iter()
            .filter(|relocation| matches!(relocation.kind.as_str(), "call" | "call-plt"))
        {
            let sites = correspondence
                .iter()
                .filter(|item| {
                    item.origin_offsets.contains(&instruction.offset)
                        && item.relocation_symbols.contains(&relocation.symbol)
                })
                .filter_map(|item| u32::try_from(item.runtime_address).ok())
                .collect::<BTreeSet<_>>();
            if sites.len() != 1 {
                continue;
            }
            let site = *sites.first().expect("one correspondence site");
            if !existing_sites.insert(site) {
                continue;
            }
            let targets = resolver
                .symbols
                .iter()
                .filter(|candidate| {
                    candidate.addresses_resolved && candidate.name == relocation.symbol
                })
                .collect::<Vec<_>>();
            let target = match targets.as_slice() {
                [target] => identities.symbol(target),
                _ => relocation.symbol.clone(),
            };
            calls.insert(LinkedCall {
                kind: "structural-relocation",
                target,
                site: Some(site),
                tail: false,
                result_modeled: false,
                execution_model: None,
                semantics: Some(
                    "archive direct-call relocation projected through conservative instruction correspondence; structural reachability only"
                        .to_owned(),
                ),
                semantic_operation: None,
                semantic_contract: None,
                replacement_hint: None,
                project_symbol: (targets.len() != 1).then(|| relocation.symbol.clone()),
                project_candidates: Vec::new(),
                trampoline: None,
                argument_shapes: 0,
                arguments: Vec::new(),
                argument_bindings: Vec::new(),
                typed_arguments: Vec::new(),
                guard_paths: None,
            });
        }
    }
    Ok(())
}

fn indexed_dispatch_calls(
    owner: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) -> crate::Result<Vec<LinkedCall>> {
    let dispatches =
        artifact::recover_indexed_dispatches(owner, &resolver.data_objects, &resolver.symbols)?;
    let mut calls = Vec::new();
    for dispatch in dispatches {
        for entry in dispatch.entries {
            for callee in entry.callees {
                let definition = callee
                    .address
                    .and_then(|address| resolver.symbols_by_address.get(&address))
                    .or_else(|| {
                        let mut matches = resolver.symbols.iter().filter(|candidate| {
                            candidate.name == callee.symbol
                                && (candidate.member == owner.member
                                    || candidate.member.is_none()
                                    || owner.member.is_none())
                        });
                        let selected = matches.next()?;
                        matches.next().is_none().then_some(selected)
                    });
                let (kind, target) = definition.map_or_else(
                    || ("indexed-dispatch-unresolved", callee.symbol.clone()),
                    |definition| ("indexed-dispatch", identities.symbol(definition)),
                );
                calls.push(LinkedCall {
                    kind,
                    target,
                    site: Some(dispatch.site),
                    tail: true,
                    result_modeled: false,
                    execution_model: None,
                    semantics: Some(format!(
                        "bounded indexed dispatch; table={}; selector={}; stride={}; case={}@{:#010x}; handler-call={:#010x}",
                        dispatch.table,
                        entry.selector,
                        dispatch.stride,
                        entry.case_target,
                        entry.case_address,
                        callee.site,
                    )),
                    semantic_operation: None,
                    semantic_contract: None,
                    replacement_hint: None,
                    project_symbol: None,
                    project_candidates: Vec::new(),
                    trampoline: None,
                    argument_shapes: 1,
                    arguments: vec![format!("selector={}", entry.selector)],
                    argument_bindings: Vec::new(),
                    typed_arguments: Vec::new(),
                    guard_paths: None,
                });
            }
        }
    }
    Ok(calls)
}

fn annotate_direct_semantic_calls(
    calls: &mut [LinkedCall],
    owner: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) {
    let Some(hooks) = resolver.pointer_context.summary_hooks else {
        return;
    };
    for call in calls.iter_mut().filter(|call| {
        matches!(call.kind, "internal" | "structural-relocation")
            && call.semantic_operation.is_none()
    }) {
        let (function, contract_source) =
            if let Some(symbol) = identities.selectable_symbol(&call.target) {
                let Some(function) = (hooks.direct_semantic)(symbol)
                    .or_else(|| resolver.projected_direct_semantic(symbol))
                else {
                    continue;
                };
                let source = if (hooks.direct_semantic)(symbol).is_some() {
                    function.source
                } else {
                    "unique-reviewed-archive-origin"
                };
                (function, source)
            } else {
                let Some(site) = call.site else {
                    continue;
                };
                let Some((symbol, Some(_))) = resolver
                    .relocated_calls
                    .get(&crate::StructuralCallSite::new(owner, site))
                else {
                    continue;
                };
                let Some(function) = (hooks.direct_external_semantic)(symbol) else {
                    continue;
                };
                call.kind = "external";
                call.target = symbol.clone();
                (function, "authoritative-link-unit-symbol")
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
            source: contract_source,
            id: function.id.to_owned(),
            evidence: function.evidence.to_owned(),
            body_policy: function.body_policy.label(),
            event_dispatch: linked_event_dispatch_contract(function.semantic),
        });
        if function.body_policy == crate::SemanticFunctionBodyPolicy::OpaqueBoundary
            && matches!(call.kind, "internal" | "structural-relocation")
        {
            call.kind = "semantic-boundary";
        }
        call.replacement_hint = function.semantic.replacement.map(str::to_owned);
        call.typed_arguments = direct_semantic_typed_arguments(function, &call.arguments);
    }
}

fn reachable_decode_blockers(
    symbol: &artifact::ArtifactSymbolDefinition,
) -> Vec<LinkedDecodeBlocker> {
    artifact::reachable_unsupported_instructions(symbol)
        .map(|blockers| {
            blockers
                .into_iter()
                .map(|blocker| LinkedDecodeBlocker {
                    address: blocker.address,
                    width: blocker.width,
                    raw: blocker.raw,
                    class: blocker.class.as_str(),
                    linear_control_flow: blocker.linear_control_flow,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn function_completeness(
    trace: &FunctionAnalysis,
    calls: &[LinkedCall],
    direct_diagnostics: &[LinkedDiagnostic],
) -> LinkedFunctionCompleteness {
    let call_targets_complete = calls.iter().all(|call| {
        !matches!(
            call.kind,
            "unresolved" | "ambiguous-project" | "indexed-dispatch-unresolved"
        )
    });
    let body_diagnostics_complete = direct_diagnostics.is_empty()
        || (call_targets_complete
            && direct_diagnostics
                .iter()
                .all(|diagnostic| matches!(diagnostic.kind, "call-boundary" | "unresolved-call")));
    LinkedFunctionCompleteness {
        body_complete: trace.unresolved_branch.is_none()
            && trace.reference_observables_are_mapped()
            && body_diagnostics_complete,
        call_targets_complete,
        transitive_effects_complete: false,
        executable_complete: trace.is_reference_eligible(),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct LinkedIrSourceOptions<'a> {
    pub(crate) symbol_prefix: &'a str,
    pub(crate) source: &'a str,
    pub(crate) namespace_identities: bool,
    pub(crate) include_reachable: bool,
    pub(crate) jobs: usize,
    pub(crate) compact_projected_actions: bool,
}

pub(crate) fn build_linked_ir_for_source(
    resolver: &ReferenceResolver,
    svd: &MmioMap,
    options: LinkedIrSourceOptions<'_>,
) -> LinkedIrReport {
    let LinkedIrSourceOptions {
        symbol_prefix,
        source,
        namespace_identities,
        include_reachable,
        jobs,
        compact_projected_actions,
    } = options;
    let started = std::time::Instant::now();
    let mut root_keys = BTreeSet::new();
    let roots = resolver
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.name.starts_with(symbol_prefix) && root_keys.insert(symbol_key(symbol))
        })
        .collect::<Vec<_>>();
    let jobs = linked_ir_worker_count(jobs, roots.len());
    let functions = if jobs > 1 && symbol_prefix.is_empty() {
        build_all_linked_functions_parallel(
            resolver,
            roots,
            svd,
            source,
            namespace_identities,
            jobs,
        )
    } else {
        build_linked_functions_for_roots(
            LinkedFunctionBuild {
                resolver,
                symbol_prefix,
                svd,
                source,
                progress_label: source,
                namespace_identities,
                include_reachable,
            },
            roots,
        )
    };
    let function_analysis_elapsed = started.elapsed();
    tracing::debug!(
        source,
        functions = functions.len(),
        rss_kib = crate::resource_usage::resident_set_kib(),
        function_analysis_ms = function_analysis_elapsed.as_millis(),
        "completed direct linked-IR function analysis"
    );
    let summary_started = std::time::Instant::now();
    let report = summarize_linked_ir_with_options(functions, jobs, compact_projected_actions);
    tracing::debug!(
        source,
        functions = report.functions.len(),
        function_analysis_ms = function_analysis_elapsed.as_millis(),
        effect_summary_ms = summary_started.elapsed().as_millis(),
        "completed linked-IR source analysis"
    );
    report
}

#[derive(Clone, Copy)]
struct LinkedFunctionBuild<'a> {
    resolver: &'a ReferenceResolver,
    symbol_prefix: &'a str,
    svd: &'a MmioMap,
    source: &'a str,
    progress_label: &'a str,
    namespace_identities: bool,
    include_reachable: bool,
}

fn build_linked_functions_for_roots(
    build: LinkedFunctionBuild<'_>,
    roots: Vec<&artifact::ArtifactSymbolDefinition>,
) -> Vec<LinkedIrFunction> {
    let LinkedFunctionBuild {
        resolver,
        symbol_prefix,
        svd,
        source,
        progress_label,
        namespace_identities,
        include_reachable,
    } = build;
    let mut functions = Vec::new();
    let identities = IrIdentityCatalog::new(resolver, namespace_identities.then_some(source));
    let mut scheduled = BTreeSet::<SymbolKey>::new();
    let mut pending = VecDeque::new();
    for symbol in roots {
        if scheduled.insert(symbol_key(symbol)) {
            pending.push_back(symbol);
        }
    }

    let progress = linked_ir_progress_span(progress_label, scheduled.len());

    while let Some(symbol) = pending.pop_front() {
        let selection = if symbol.name.starts_with(symbol_prefix) {
            "symbol-prefix-root"
        } else {
            "reachable-internal"
        };
        let function_identity = identities.symbol(symbol);
        let binding = if resolver.symbol_is_exported(symbol) {
            "global-or-weak"
        } else {
            "local"
        };
        let decode_blockers = reachable_decode_blockers(symbol);
        let DirectCallGraph {
            calls: mut direct_calls,
            direct_mmio_predicates,
            mut blockers,
            site_effects,
        } = explore_direct_calls(symbol, resolver, &identities, svd);
        match indexed_dispatch_calls(symbol, resolver, &identities) {
            Ok(calls) => direct_calls.extend(calls),
            Err(error) => {
                blockers.insert(format!(
                    "indexed-dispatch recovery failed for {function_identity}: {error}"
                ));
            }
        }
        add_lossless_relocation_calls(&mut direct_calls, symbol, resolver, &identities);
        if !blockers.is_empty()
            && let Err(error) =
                add_projected_origin_calls(&mut direct_calls, symbol, resolver, &identities)
        {
            blockers.insert(format!(
                "archive-origin call projection failed for {function_identity}: {error}"
            ));
        }
        let direct_mmio_predicates = direct_mmio_predicates.into_iter().collect::<Vec<_>>();
        let mut direct_calls = compact_calls(direct_calls);
        annotate_direct_semantic_calls(&mut direct_calls, symbol, resolver, &identities);
        if include_reachable {
            for call in direct_calls.iter().filter(|call| {
                matches!(
                    call.kind,
                    "internal" | "indexed-dispatch" | "structural-relocation"
                )
            }) {
                let Some(callee) = identities.selectable_symbol(&call.target) else {
                    continue;
                };
                if scheduled.insert(symbol_key(callee)) {
                    pending.push_back(callee);
                    progress.pb_inc_length(1);
                }
            }
        }
        let call_graph_messages = blockers.into_iter().collect::<Vec<_>>();
        let call_graph_diagnostics = compact_diagnostics(&call_graph_messages);
        match resolver.trace_symbol_bounded(
            symbol,
            svd,
            direct::StructuralTraceBudget {
                max_instruction_steps: MAX_CALL_GRAPH_INSTRUCTION_STEPS_PER_TRACE,
                max_events: MAX_CALL_GRAPH_EVENTS_PER_TRACE,
            },
        ) {
            Ok(trace) => {
                let mut memory_accesses = memory_object_accesses_for_trace(&trace);
                attribute_data_symbols(&mut memory_accesses, resolver);
                let mut memory_fields = memory_object_fields_for_accesses(&memory_accesses);
                let context_accesses = context_accesses_for_memory_objects(&memory_accesses);
                let context_fields = context_fields_for_accesses(&context_accesses);
                let mmio_accesses = mmio_accesses_for_trace(&trace);
                let mut instruction_effects = instruction_effects_for_trace(
                    &trace,
                    resolver,
                    &mmio_accesses,
                    &memory_accesses,
                );
                instruction_effects.extend(site_effects);
                instruction_effects.sort();
                instruction_effects.dedup();
                merge_instruction_memory_fields(&mut memory_fields, &instruction_effects);
                let effect_sites = instruction_effects
                    .iter()
                    .map(LinkedInstructionEffect::site)
                    .collect::<BTreeSet<_>>();
                let effect_blocks =
                    artifact::basic_block_ids_for_sites(symbol, &effect_sites).unwrap_or_default();
                for effect in &mut instruction_effects {
                    effect.set_block(effect_blocks.get(&effect.site()).copied());
                }
                let delays = delays_for_trace(&trace);
                let scenario_suggestions =
                    scenario_suggestions(Some(&trace), &direct_mmio_predicates, &mmio_accesses);
                let return_call_results = trace_call_results(&trace, &identities);
                let return_provenance =
                    return_provenance(&trace.return_value, &return_call_results, svd);
                let mut calls = if direct_calls.is_empty() {
                    calls_for_trace(&trace, resolver, &identities)
                } else {
                    direct_calls
                };
                annotate_direct_semantic_calls(&mut calls, symbol, resolver, &identities);
                let flow_kind = if trace.reference_flow.is_some() {
                    "structured"
                } else if trace.is_reference_eligible() {
                    "linear"
                } else {
                    "partial"
                };
                let direct_diagnostics = compact_diagnostics(&trace.blockers);
                let reference_diagnostics = compact_diagnostics(&trace.reference_blockers);
                let completeness = function_completeness(&trace, &calls, &direct_diagnostics);
                let pseudo = render_pseudo(
                    &function_identity,
                    &trace,
                    &calls,
                    &direct_diagnostics,
                    &reference_diagnostics,
                    &call_graph_diagnostics,
                    Some(resolver),
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
                    completeness,
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
                    projected_relocations: projected_relocations(symbol, resolver),
                    local_value_flow: local_value_flow(&trace),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses,
                    instruction_effects,
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
                    decode_blockers,
                    pseudo,
                });
            }
            Err(error) => {
                let direct_diagnostics = vec![compact_diagnostic(&error.to_string())];
                let mut calls = direct_calls;
                annotate_direct_semantic_calls(&mut calls, symbol, resolver, &identities);
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
                    completeness: LinkedFunctionCompleteness {
                        body_complete: false,
                        call_targets_complete: false,
                        transitive_effects_complete: false,
                        executable_complete: false,
                    },
                    exact: false,
                    return_value: "unknown".to_owned(),
                    return_provenance: return_provenance(
                        &SymbolicValue::Unknown,
                        &BTreeMap::new(),
                        svd,
                    ),
                    dependencies: Vec::new(),
                    projected_relocations: projected_relocations(symbol, resolver),
                    local_value_flow: Vec::new(),
                    calls,
                    direct_mmio_predicates,
                    mmio_accesses: Vec::new(),
                    instruction_effects: Vec::new(),
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
                    decode_blockers,
                    pseudo: format!(
                        "// vendor symbol: {function_identity}\n// DECODE-BLOCKER: {error}\nfn {}(args: [u32; 16]) -> u32 {{ unknown }}\n",
                        pseudo_identifier(&function_identity)
                    ),
                });
            }
        }
        progress.pb_inc(1);
        progress.pb_set_message(&format!("{source}: completed {function_identity}"));
        if functions.len().is_multiple_of(64)
            && crate::resource_usage::resident_set_kib().is_some_and(|rss| rss >= 256 * 1024)
        {
            // Some branch-heavy functions temporarily materialize hundreds
            // of MiB of trace state. The state is dropped before this point,
            // but glibc may retain those pages in its arenas. Returning fully
            // free pages here keeps the next function's peak independent of
            // the previous function without changing any analysis budget or
            // result.
            crate::resource_usage::release_unused_memory("linked-IR function batch");
        }
        if functions.len().is_multiple_of(128) {
            tracing::debug!(
                source,
                functions = functions.len(),
                function = function_identity,
                rss_kib = ?crate::resource_usage::resident_set_kib(),
                "linked-IR function analysis checkpoint"
            );
        }
    }

    progress.pb_set_finish_message(&format!("{source}: analyzed {} functions", functions.len()));

    functions
}

fn linked_ir_worker_count(requested: usize, functions: usize) -> usize {
    let available = thread::available_parallelism().map_or(1, usize::from);
    let requested = requested.clamp(1, MAX_LINKED_IR_JOBS).min(available);
    requested.max(1).min(functions.max(1))
}

fn build_all_linked_functions_parallel(
    resolver: &ReferenceResolver,
    mut roots: Vec<&artifact::ArtifactSymbolDefinition>,
    svd: &MmioMap,
    source: &str,
    namespace_identities: bool,
    jobs: usize,
) -> Vec<LinkedIrFunction> {
    // Long ROM routines dominate short thunks. Greedily balancing byte counts
    // gives every worker comparable input while retaining deterministic
    // partitioning and final identity order.
    roots.sort_by(|left, right| {
        right
            .bytes
            .len()
            .cmp(&left.bytes.len())
            .then_with(|| symbol_key(left).cmp(&symbol_key(right)))
    });
    let mut buckets = (0..jobs).map(|_| (0_usize, Vec::new())).collect::<Vec<_>>();
    for root in roots {
        let bucket = buckets
            .iter()
            .enumerate()
            .min_by_key(|(index, (bytes, _))| (*bytes, *index))
            .map(|(index, _)| index)
            .expect("at least one linked-IR worker bucket");
        buckets[bucket].0 += root.bytes.len();
        buckets[bucket].1.push(root);
    }

    let (sender, receiver) = sync_channel::<(usize, Vec<LinkedIrFunction>)>(jobs);
    thread::scope(|scope| {
        for (worker, (_, roots)) in buckets.into_iter().enumerate() {
            let sender = sender.clone();
            let progress_label = format!("{source} worker {}", worker + 1);
            thread::Builder::new()
                .name(format!("linked-ir-{worker}"))
                .stack_size(LINKED_IR_WORKER_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    let functions = build_linked_functions_for_roots(
                        LinkedFunctionBuild {
                            resolver,
                            symbol_prefix: "",
                            svd,
                            source,
                            progress_label: &progress_label,
                            namespace_identities,
                            include_reachable: false,
                        },
                        roots,
                    );
                    sender
                        .send((worker, functions))
                        .expect("linked-IR result receiver remains alive");
                })
                .expect("spawning a bounded linked-IR worker");
        }
        drop(sender);
        let mut chunks = (0..jobs)
            .map(|_| {
                receiver
                    .recv()
                    .expect("every linked-IR worker publishes one result")
            })
            .collect::<Vec<_>>();
        chunks.sort_by_key(|(worker, _)| *worker);
        chunks
            .into_iter()
            .flat_map(|(_, functions)| functions)
            .collect()
    })
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

#[cfg(test)]
pub(crate) fn merge_linked_ir(reports: Vec<LinkedIrReport>) -> LinkedIrReport {
    merge_linked_ir_with_jobs(reports, 1)
}

#[cfg(test)]
pub(crate) fn merge_linked_ir_with_jobs(
    reports: Vec<LinkedIrReport>,
    jobs: usize,
) -> LinkedIrReport {
    merge_linked_ir_with_options(reports, jobs, false)
}

pub(crate) fn merge_linked_ir_with_options(
    mut reports: Vec<LinkedIrReport>,
    jobs: usize,
    compact_projected_actions: bool,
) -> LinkedIrReport {
    if reports.len() == 1 {
        return reports.pop().expect("one linked-IR report is present");
    }
    let mut functions = reports
        .into_iter()
        .flat_map(|report| report.functions)
        .collect::<Vec<_>>();
    for function in &mut functions {
        function.effect_summary = LinkedEffectSummary::default();
    }
    summarize_linked_ir_with_options(functions, jobs, compact_projected_actions)
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
