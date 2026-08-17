//! Affine root-context and generalized memory-object projection.

use super::super::*;
use super::SummaryCallEdge;
use super::event_dispatch::project_event_dispatches;

pub(super) struct ProjectContextProjection {
    pub(super) complete: bool,
    pub(super) paths_materialized: bool,
    pub(super) blockers: Vec<String>,
    pub(super) fields: Vec<LinkedSummaryContextField>,
    pub(super) trampoline_calls: Vec<LinkedProjectedTrampolineCall>,
    pub(super) semantic_action_count: usize,
    pub(super) semantic_actions: Vec<LinkedProjectedSemanticAction>,
    pub(super) event_dispatches: Vec<LinkedEventDispatch>,
}
#[derive(Default)]
struct SummaryContextAccumulator {
    read_shapes: BTreeSet<String>,
    write_shapes: BTreeSet<ContextWriteShape>,
    write_mask: u32,
    origins: BTreeSet<String>,
    paths: BTreeSet<String>,
    write_values: BTreeSet<String>,
}
#[derive(Default)]
struct SummaryMemoryAccumulator {
    reads: usize,
    writes: usize,
    write_mask: u32,
    origins: BTreeSet<String>,
    paths: BTreeSet<String>,
    write_values: BTreeSet<String>,
}
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct ContextWriteShape {
    path: String,
    value: Option<String>,
    write_mask: Option<u32>,
    preserved_mask: Option<u32>,
    forced_zero_mask: Option<u32>,
    forced_one_mask: Option<u32>,
}
struct ContextProjectionState {
    function: usize,
    argument_map: Vec<Option<(u8, i32)>>,
    visited_functions: Vec<usize>,
    /// `(caller, edge index)` pairs are the compact authoritative path. Human
    /// strings, site paths and guard scopes are reconstructed only when an
    /// emitted fact needs them; retaining their complete prefixes in every
    /// queued state is quadratic on large linked images.
    edge_path: Vec<(usize, usize)>,
}

fn render_state_path(
    state: &ContextProjectionState,
    functions: &[LinkedIrFunction],
    call_edges: &[Vec<SummaryCallEdge>],
) -> String {
    let mut path = functions[state.visited_functions[0]].identity.clone();
    for &(caller, edge_index) in &state.edge_path {
        let edge = &call_edges[caller][edge_index];
        let site = edge
            .site
            .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
        path.push_str(&format!(
            " --call@{}--> {}",
            site, functions[edge.target].identity
        ));
    }
    path
}

fn state_site_path(
    state: &ContextProjectionState,
    call_edges: &[Vec<SummaryCallEdge>],
) -> Vec<Option<u32>> {
    state
        .edge_path
        .iter()
        .map(|&(caller, edge_index)| call_edges[caller][edge_index].site)
        .collect()
}

fn state_guard_scopes(
    state: &ContextProjectionState,
    functions: &[LinkedIrFunction],
    call_edges: &[Vec<SummaryCallEdge>],
) -> Option<Vec<LinkedCallGuardScope>> {
    let mut scopes = Some(Vec::new());
    for &(caller, edge_index) in &state.edge_path {
        let edge = &call_edges[caller][edge_index];
        scopes = extend_guard_scopes(
            scopes.as_deref(),
            &functions[caller].identity,
            edge.guard_paths.as_deref(),
        );
    }
    scopes
}
pub(super) fn project_memory_fields(
    reachable: &[(usize, usize)],
    functions: &[LinkedIrFunction],
    contexts: &[LinkedSummaryContextField],
) -> Vec<LinkedSummaryMemoryField> {
    let mut fields = BTreeMap::<(LinkedMemoryObject, i64, u8), SummaryMemoryAccumulator>::new();
    for context in contexts {
        let object = LinkedMemoryObject::Argument {
            index: context.argument,
        };
        let entry = fields
            .entry((object, i64::from(context.offset), context.width))
            .or_default();
        entry.reads += context.reads;
        entry.writes += context.writes;
        entry.write_mask |= context.write_mask;
        entry.origins.extend(context.origins.iter().cloned());
        entry.paths.extend(context.paths.iter().cloned());
        entry
            .write_values
            .extend(context.write_values.iter().cloned());
    }
    for (index, _) in reachable {
        let function = &functions[*index];
        for field in &function.memory_fields {
            if matches!(field.object, LinkedMemoryObject::Argument { .. }) {
                continue;
            }
            let entry = fields
                .entry((field.object.clone(), field.offset, field.width))
                .or_default();
            entry.reads += field.reads;
            entry.writes += field.writes;
            entry.write_mask |= field.write_mask;
            entry.origins.insert(function.identity.clone());
            entry.paths.extend(field.paths.iter().cloned());
            entry
                .write_values
                .extend(field.write_values.iter().cloned());
        }
    }
    fields
        .into_iter()
        .map(
            |((object, offset, width), entry)| LinkedSummaryMemoryField {
                object,
                offset,
                width,
                reads: entry.reads,
                writes: entry.writes,
                write_mask: entry.write_mask,
                origins: entry.origins.into_iter().collect(),
                paths: entry.paths.into_iter().collect(),
                write_values: entry.write_values.into_iter().collect(),
            },
        )
        .collect()
}
fn local_context_access(access: &ContextAccess) -> bool {
    !access
        .path
        .split(" / ")
        .any(|component| component.starts_with("call "))
}
fn project_call_arguments(
    call: &LinkedCall,
    argument_map: &[Option<(u8, i32)>],
    blockers: &mut BTreeSet<String>,
    boundary: &str,
    path: &str,
) -> Vec<LinkedProjectedCallArgument> {
    call.typed_arguments
        .iter()
        .map(|argument| {
            let affine = call
                .argument_bindings
                .iter()
                .find(|binding| binding.position == argument.position);
            let pointer = argument.c_type.contains('*');
            let (binding, root_argument, root_offset) = if !pointer {
                ("non-pointer", None, None)
            } else if let Some(affine) = affine {
                match argument_map
                    .get(usize::from(affine.caller_argument))
                    .copied()
                    .flatten()
                    .and_then(|(argument, offset)| {
                        offset
                            .checked_add(affine.offset)
                            .map(|offset| (argument, offset))
                    }) {
                    Some((argument, offset)) => {
                        ("affine-root-context", Some(argument), Some(offset))
                    }
                    None => {
                        blockers.insert(format!(
                            "semantic pointer binding cannot reach root: {boundary} arg{} along {path}",
                            argument.position
                        ));
                        ("affine-origin-context-unavailable", None, None)
                    }
                }
            } else {
                ("not-affine-caller-context", None, None)
            };
            LinkedProjectedCallArgument {
                position: argument.position,
                name: argument.name.clone(),
                c_type: argument.c_type.clone(),
                direction: argument.direction,
                value: argument.value.clone(),
                binding,
                root_argument,
                root_offset,
            }
        })
        .collect()
}
fn extend_guard_scopes(
    scopes: Option<&[LinkedCallGuardScope]>,
    function: &str,
    guard_paths: Option<&[LinkedCallGuardPath]>,
) -> Option<Vec<LinkedCallGuardScope>> {
    let mut scopes = scopes?.to_vec();
    let paths = guard_paths?;
    if paths.iter().any(|path| path.guards.is_empty()) {
        return Some(scopes);
    }
    scopes.push(LinkedCallGuardScope {
        function: function.to_owned(),
        paths: paths.to_vec(),
    });
    Some(scopes)
}
pub(super) fn project_context_fields(
    root: usize,
    functions: &[LinkedIrFunction],
    call_edges: &[Vec<SummaryCallEdge>],
    projection_reachable: &[bool],
    call_graph_closed: bool,
    materialize_semantic_actions: bool,
) -> ProjectContextProjection {
    let mut root_arguments = vec![None; usize::from(LINKED_CONTEXT_ARGUMENTS)];
    for argument in 0..LINKED_CONTEXT_ARGUMENTS {
        root_arguments[usize::from(argument)] = Some((argument, 0));
    }
    let mut queue = VecDeque::from([ContextProjectionState {
        function: root,
        argument_map: root_arguments,
        visited_functions: vec![root],
        edge_path: Vec::new(),
    }]);
    let mut blockers = BTreeSet::new();
    let mut fields = BTreeMap::<(u8, i32, u8), SummaryContextAccumulator>::new();
    let mut trampoline_calls = BTreeSet::new();
    let mut semantic_actions = BTreeSet::new();
    let mut semantic_action_count = 0_usize;
    let mut explored = 0_usize;
    let mut scheduled = 1_usize;
    let mut projection_budget_exhausted = false;

    while let Some(state) = queue.pop_front() {
        if explored >= MAX_CONTEXT_PROJECTION_STATES {
            blockers.insert(format!(
                "context projection exceeds {MAX_CONTEXT_PROJECTION_STATES} simple-path states"
            ));
            break;
        }
        explored += 1;
        let function = &functions[state.function];
        for call in function
            .calls
            .iter()
            .filter(|call| call.semantic_operation.is_some())
        {
            let operation = call
                .semantic_operation
                .as_ref()
                .expect("filtered semantic call")
                .clone();
            let mut site_path = state_site_path(&state, call_edges);
            site_path.push(call.site);
            let guard_scopes = state_guard_scopes(&state, functions, call_edges);
            let action_guard_scopes = extend_guard_scopes(
                guard_scopes.as_deref(),
                &function.identity,
                call.guard_paths.as_deref(),
            );
            semantic_action_count += 1;
            let event_dispatch = call
                .semantic_contract
                .as_ref()
                .and_then(|contract| contract.event_dispatch.as_ref())
                .is_some();
            if !materialize_semantic_actions
                && !action_guard_scopes
                    .as_ref()
                    .is_some_and(|scopes| !scopes.is_empty())
                && !event_dispatch
            {
                continue;
            }
            let state_path = render_state_path(&state, functions, call_edges);
            let site = call
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            let boundary = format!("{operation} via {}", call.target);
            semantic_actions.insert(LinkedProjectedSemanticAction {
                site_path,
                path: format!("{state_path} --semantic@{site}--> {}", call.target),
                operation,
                target: call.target.clone(),
                contract: call.semantic_contract.clone(),
                replacement_hint: call.replacement_hint.clone(),
                origin: function.identity.clone(),
                site: call.site,
                argument_shapes: call.argument_shapes,
                arguments: project_call_arguments(
                    call,
                    &state.argument_map,
                    &mut blockers,
                    &boundary,
                    &state_path,
                ),
                guard_scopes: action_guard_scopes,
            });
        }
        for call in function
            .calls
            .iter()
            .filter(|call| call.trampoline.is_some())
        {
            let state_path = render_state_path(&state, functions, call_edges);
            let trampoline = call
                .trampoline
                .as_ref()
                .expect("filtered trampoline call")
                .clone();
            let boundary = format!("{} {}", trampoline.table, trampoline.c_name);
            let arguments = project_call_arguments(
                call,
                &state.argument_map,
                &mut blockers,
                &boundary,
                &state_path,
            );
            trampoline_calls.insert(LinkedProjectedTrampolineCall {
                path: format!(
                    "{} --trampoline@{}+{:#x}--> {}",
                    state_path, trampoline.table, trampoline.slot, trampoline.c_name
                ),
                trampoline,
                origin: function.identity.clone(),
                argument_shapes: call.argument_shapes,
                arguments,
            });
        }
        for access in function
            .context_accesses
            .iter()
            .filter(|access| local_context_access(access))
        {
            let Some((root_argument, base_offset)) = state
                .argument_map
                .get(usize::from(access.argument))
                .copied()
                .flatten()
            else {
                if materialize_semantic_actions {
                    blockers.insert(format!(
                        "no affine binding for {} arg{} along {}",
                        function.identity,
                        access.argument,
                        render_state_path(&state, functions, call_edges)
                    ));
                } else {
                    blockers.insert(format!(
                        "no affine binding for {} arg{}; run a focused query for path evidence",
                        function.identity, access.argument
                    ));
                }
                continue;
            };
            let Some(offset) = base_offset.checked_add(access.offset) else {
                if materialize_semantic_actions {
                    blockers.insert(format!(
                        "context offset overflow for {} arg{} along {}",
                        function.identity,
                        access.argument,
                        render_state_path(&state, functions, call_edges)
                    ));
                } else {
                    blockers.insert(format!(
                        "context offset overflow for {} arg{}; run a focused query for path evidence",
                        function.identity, access.argument
                    ));
                }
                continue;
            };
            let field = fields
                .entry((root_argument, offset, access.width))
                .or_default();
            match access.access {
                "read" => {
                    field.read_shapes.insert(access.path.clone());
                }
                "write" => {
                    field.write_shapes.insert(ContextWriteShape {
                        path: access.path.clone(),
                        value: access.value.clone(),
                        write_mask: access.write_mask,
                        preserved_mask: access.preserved_mask,
                        forced_zero_mask: access.forced_zero_mask,
                        forced_one_mask: access.forced_one_mask,
                    });
                    field.write_mask |= access.write_mask.unwrap_or_default();
                    if let Some(value) = access.value_pseudo.as_ref() {
                        field.write_values.insert(value.clone());
                    }
                }
                _ => unreachable!("context access has a closed access vocabulary"),
            }
            field.origins.insert(function.identity.clone());
            if materialize_semantic_actions {
                field.paths.insert(format!(
                    "{} / {}",
                    render_state_path(&state, functions, call_edges),
                    access.path
                ));
            }
        }

        if projection_budget_exhausted {
            continue;
        }
        for (edge_index, edge) in call_edges[state.function].iter().enumerate() {
            if !projection_reachable[edge.target] {
                continue;
            }
            if state.visited_functions.contains(&edge.target) {
                blockers.insert(format!(
                    "recursive context projection stopped: {} -> {}",
                    function.identity, functions[edge.target].identity
                ));
                continue;
            }
            if scheduled >= MAX_CONTEXT_PROJECTION_STATES {
                blockers.insert(format!(
                    "context projection exceeds {MAX_CONTEXT_PROJECTION_STATES} scheduled simple-path states"
                ));
                projection_budget_exhausted = true;
                break;
            }
            let mut argument_map = vec![None; usize::from(LINKED_CONTEXT_ARGUMENTS)];
            for binding in &edge.bindings {
                if binding.position >= argument_map.len() {
                    continue;
                }
                let Some((root_argument, caller_offset)) = state
                    .argument_map
                    .get(usize::from(binding.caller_argument))
                    .copied()
                    .flatten()
                else {
                    continue;
                };
                let Some(offset) = caller_offset.checked_add(binding.offset) else {
                    blockers.insert(format!(
                        "call argument offset overflow: {} -> {} arg{}",
                        function.identity, functions[edge.target].identity, binding.position
                    ));
                    continue;
                };
                argument_map[binding.position] = Some((root_argument, offset));
            }
            let mut visited_functions = state.visited_functions.clone();
            visited_functions.push(edge.target);
            let mut edge_path = state.edge_path.clone();
            edge_path.push((state.function, edge_index));
            queue.push_back(ContextProjectionState {
                function: edge.target,
                argument_map,
                visited_functions,
                edge_path,
            });
            scheduled += 1;
        }
    }

    let fields = fields
        .into_iter()
        .map(
            |((argument, offset, width), field)| LinkedSummaryContextField {
                argument,
                offset,
                width,
                reads: field.read_shapes.len(),
                writes: field.write_shapes.len(),
                write_mask: field.write_mask,
                origins: field.origins.into_iter().collect(),
                paths: field.paths.into_iter().collect(),
                write_values: field.write_values.into_iter().collect(),
            },
        )
        .collect();
    let semantic_actions = semantic_actions.into_iter().collect::<Vec<_>>();
    let event_dispatches = project_event_dispatches(&semantic_actions);
    ProjectContextProjection {
        complete: call_graph_closed && blockers.is_empty(),
        paths_materialized: materialize_semantic_actions,
        blockers: blockers.into_iter().collect(),
        fields,
        trampoline_calls: trampoline_calls.into_iter().collect(),
        semantic_action_count,
        semantic_actions,
        event_dispatches,
    }
}
pub(super) fn projection_reachability(
    functions: &[LinkedIrFunction],
    adjacency: &[Vec<usize>],
) -> Vec<bool> {
    let mut reverse = vec![Vec::new(); functions.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }
    let mut reachable = functions
        .iter()
        .map(|function| {
            function.context_accesses.iter().any(local_context_access)
                || function.calls.iter().any(|call| call.trampoline.is_some())
                || function
                    .calls
                    .iter()
                    .any(|call| call.semantic_operation.is_some())
        })
        .collect::<Vec<_>>();
    let mut queue = reachable
        .iter()
        .enumerate()
        .filter_map(|(index, reachable)| reachable.then_some(index))
        .collect::<VecDeque<_>>();
    while let Some(target) = queue.pop_front() {
        for &source in &reverse[target] {
            if !reachable[source] {
                reachable[source] = true;
                queue.push_back(source);
            }
        }
    }
    reachable
}
