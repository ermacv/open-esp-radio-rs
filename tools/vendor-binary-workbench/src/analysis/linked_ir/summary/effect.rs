//! Transitive call-graph traversal and reachable effect aggregation.

use super::super::*;
use super::SummaryCallEdge;
use super::projection::{project_context_fields, project_memory_fields, projection_reachability};
use petgraph::{algo::kosaraju_scc, graph::DiGraph};

fn recursive_call_graph_nodes(adjacency: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut graph =
        DiGraph::<(), ()>::with_capacity(adjacency.len(), adjacency.iter().map(Vec::len).sum());
    let nodes = (0..adjacency.len())
        .map(|_| graph.add_node(()))
        .collect::<Vec<_>>();
    for (source, targets) in adjacency.iter().enumerate() {
        for target in targets {
            graph.add_edge(nodes[source], nodes[*target], ());
        }
    }

    let mut recursive = BTreeSet::new();
    for component in kosaraju_scc(&graph) {
        let self_recursive = component.len() == 1
            && adjacency[component[0].index()]
                .iter()
                .any(|target| *target == component[0].index());
        if component.len() > 1 || self_recursive {
            recursive.extend(component.into_iter().map(|node| node.index()));
        }
    }
    recursive
}
#[derive(Default)]
struct SummaryMmioAccumulator {
    access_shapes: usize,
    accesses: BTreeSet<&'static str>,
    modes: BTreeSet<&'static str>,
    origins: BTreeSet<String>,
}
#[derive(Default)]
struct SummaryDelayAccumulator {
    delay_shapes: usize,
    origins: BTreeSet<String>,
}
#[derive(Default)]
struct SummarySemanticAccumulator {
    call_shapes: usize,
    targets: BTreeSet<String>,
    replacement_hints: BTreeSet<String>,
    origins: BTreeSet<String>,
}
pub(in crate::analysis::linked_ir) fn populate_effect_summaries(
    functions: &mut [LinkedIrFunction],
    jobs: usize,
) {
    let identities = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.identity.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut source_symbols = BTreeMap::<(String, String), Vec<usize>>::new();
    for (index, function) in functions.iter().enumerate() {
        source_symbols
            .entry((function.source.clone(), function.symbol.clone()))
            .or_default()
            .push(index);
    }

    let mut adjacency = vec![Vec::new(); functions.len()];
    let mut call_edges = vec![Vec::new(); functions.len()];
    let mut local_blockers = vec![BTreeSet::<String>::new(); functions.len()];
    for (index, function) in functions.iter().enumerate() {
        if !function.complete {
            local_blockers[index]
                .insert(format!("incomplete function body: {}", function.identity));
        }
        for blocker in &function.call_graph_blockers {
            local_blockers[index].insert(format!("{}: {blocker}", function.identity));
        }
        for call in &function.calls {
            match call.kind {
                "internal" | "project-linked" => {
                    let target = identities.get(&call.target).copied().or_else(|| {
                        let candidates =
                            source_symbols.get(&(function.source.clone(), call.target.clone()))?;
                        (candidates.len() == 1).then_some(candidates[0])
                    });
                    if let Some(target) = target {
                        adjacency[index].push(target);
                        call_edges[index].push(SummaryCallEdge {
                            target,
                            site: call.site,
                            bindings: call.argument_bindings.clone(),
                            guard_paths: call.guard_paths.clone(),
                        });
                    } else {
                        local_blockers[index].insert(format!(
                            "callee is outside the exported IR: {} -> {}",
                            function.identity, call.target
                        ));
                    }
                }
                "unresolved" => {
                    local_blockers[index].insert(format!(
                        "unresolved call edge: {} -> {}",
                        function.identity, call.target
                    ));
                }
                "external" | "diagnostic" if call.semantic_operation.is_none() => {
                    local_blockers[index].insert(format!(
                        "opaque semantic boundary: {} -> {}",
                        function.identity, call.target
                    ));
                }
                "external" | "diagnostic" => {}
                kind => {
                    local_blockers[index].insert(format!(
                        "unsupported call edge {kind}: {} -> {}",
                        function.identity, call.target
                    ));
                }
            }
        }
        adjacency[index].sort_unstable();
        adjacency[index].dedup();
    }
    let recursive_nodes = recursive_call_graph_nodes(&adjacency);
    let projection_reachable = projection_reachability(functions, &adjacency);
    let function_count = functions.len();
    let functions_view: &[LinkedIrFunction] = functions;

    let build_summary = |root: usize| {
        let mut depths = vec![None; function_count];
        depths[root] = Some(0);
        let mut queue = VecDeque::from([root]);
        while let Some(source) = queue.pop_front() {
            let next_depth = depths[source].expect("queued function has a depth") + 1;
            for &target in &adjacency[source] {
                if depths[target].is_none() {
                    depths[target] = Some(next_depth);
                    queue.push_back(target);
                }
            }
        }
        let reachable = depths
            .iter()
            .enumerate()
            .filter_map(|(index, depth)| depth.map(|depth| (index, depth)))
            .collect::<Vec<_>>();
        let mut blockers = BTreeSet::new();
        let mut mmio = BTreeMap::<(u32, u8), SummaryMmioAccumulator>::new();
        let mut delays = BTreeMap::<(String, Option<u32>), SummaryDelayAccumulator>::new();
        let mut semantics = BTreeMap::<String, SummarySemanticAccumulator>::new();
        for &(index, _) in &reachable {
            let function = &functions_view[index];
            blockers.extend(local_blockers[index].iter().cloned());
            for access in &function.mmio_accesses {
                let entry = mmio.entry((access.address, access.width)).or_default();
                entry.access_shapes += 1;
                entry.accesses.insert(access.access);
                entry.modes.insert(access.mode);
                entry.origins.insert(function.identity.clone());
            }
            for delay in &function.delays {
                let entry = delays
                    .entry((delay.micros.clone(), delay.constant_micros))
                    .or_default();
                entry.delay_shapes += 1;
                entry.origins.insert(function.identity.clone());
            }
            for call in &function.calls {
                let Some(operation) = call.semantic_operation.as_ref() else {
                    continue;
                };
                let entry = semantics.entry(operation.clone()).or_default();
                entry.call_shapes += call.argument_shapes;
                entry.targets.insert(call.target.clone());
                if let Some(replacement) = call.replacement_hint.as_ref() {
                    entry.replacement_hints.insert(replacement.clone());
                }
                entry.origins.insert(function.identity.clone());
            }
        }
        let reachable_functions = reachable
            .iter()
            .filter(|(index, _)| *index != root)
            .map(|(index, _)| functions_view[*index].identity.clone())
            .collect();
        let recursive_functions = reachable
            .iter()
            .filter(|(index, _)| recursive_nodes.contains(index))
            .map(|(index, _)| functions_view[*index].identity.clone())
            .collect();
        let call_graph_closed = blockers.is_empty();
        let context_projection = project_context_fields(
            root,
            functions_view,
            &call_edges,
            &projection_reachable,
            call_graph_closed,
        );
        let memory_fields =
            project_memory_fields(&reachable, functions_view, &context_projection.fields);
        LinkedEffectSummary {
            call_graph_closed,
            max_depth: reachable
                .iter()
                .map(|(_, depth)| *depth)
                .max()
                .unwrap_or_default(),
            reachable_functions,
            recursive_functions,
            blockers: blockers.into_iter().collect(),
            mmio_registers: mmio
                .into_iter()
                .map(|((address, width), entry)| LinkedSummaryMmio {
                    address,
                    width,
                    access_shapes: entry.access_shapes,
                    accesses: entry.accesses.into_iter().collect(),
                    modes: entry.modes.into_iter().collect(),
                    origins: entry.origins.into_iter().collect(),
                })
                .collect(),
            delays: delays
                .into_iter()
                .map(|((micros, constant_micros), entry)| LinkedSummaryDelay {
                    micros,
                    constant_micros,
                    delay_shapes: entry.delay_shapes,
                    origins: entry.origins.into_iter().collect(),
                })
                .collect(),
            semantic_operations: semantics
                .into_iter()
                .map(|(operation, entry)| LinkedSummarySemantic {
                    operation,
                    call_shapes: entry.call_shapes,
                    targets: entry.targets.into_iter().collect(),
                    replacement_hints: entry.replacement_hints.into_iter().collect(),
                    origins: entry.origins.into_iter().collect(),
                })
                .collect(),
            context_projection_complete: context_projection.complete,
            context_projection_blockers: context_projection.blockers,
            context_fields: context_projection.fields,
            memory_fields,
            trampoline_calls: context_projection.trampoline_calls,
            semantic_actions: context_projection.semantic_actions,
            event_dispatches: context_projection.event_dispatches,
        }
    };
    let jobs = linked_ir_worker_count(jobs, function_count);
    let summaries = if jobs == 1 || function_count < 2 {
        (0..function_count).map(build_summary).collect::<Vec<_>>()
    } else {
        let next_root = std::sync::atomic::AtomicUsize::new(0);
        let (sender, receiver) = sync_channel::<Vec<(usize, LinkedEffectSummary)>>(jobs);
        thread::scope(|scope| {
            for worker in 0..jobs {
                let sender = sender.clone();
                let next_root = &next_root;
                let build_summary = &build_summary;
                thread::Builder::new()
                    .name(format!("linked-ir-summary-{worker}"))
                    .stack_size(LINKED_IR_WORKER_STACK_BYTES)
                    .spawn_scoped(scope, move || {
                        let mut output = Vec::new();
                        loop {
                            let root = next_root.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if root >= function_count {
                                break;
                            }
                            output.push((root, build_summary(root)));
                        }
                        sender
                            .send(output)
                            .expect("linked-IR summary receiver remains alive");
                    })
                    .expect("spawning a bounded linked-IR summary worker");
            }
            drop(sender);
            let mut indexed = (0..jobs)
                .flat_map(|_| {
                    receiver
                        .recv()
                        .expect("every linked-IR summary worker publishes one result")
                })
                .collect::<Vec<_>>();
            indexed.sort_by_key(|(root, _)| *root);
            indexed
                .into_iter()
                .map(|(_, summary)| summary)
                .collect::<Vec<_>>()
        })
    };
    for (function, summary) in functions.iter_mut().zip(summaries) {
        function.effect_summary = summary;
    }
}
