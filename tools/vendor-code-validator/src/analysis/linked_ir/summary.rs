//! Reachable effect summaries, context projection, and event dispatch projection.

use super::*;

fn recursive_call_graph_nodes(adjacency: &[Vec<usize>]) -> BTreeSet<usize> {
    let mut visited = vec![false; adjacency.len()];
    let mut finished = Vec::with_capacity(adjacency.len());
    for start in 0..adjacency.len() {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut stack = vec![(start, 0_usize)];
        while let Some((node, next_target)) = stack.last_mut() {
            if *next_target < adjacency[*node].len() {
                let target = adjacency[*node][*next_target];
                *next_target += 1;
                if !visited[target] {
                    visited[target] = true;
                    stack.push((target, 0));
                }
            } else {
                let (node, _) = stack.pop().expect("DFS stack is non-empty");
                finished.push(node);
            }
        }
    }

    let mut reverse = vec![Vec::new(); adjacency.len()];
    for (source, targets) in adjacency.iter().enumerate() {
        for &target in targets {
            reverse[target].push(source);
        }
    }
    let mut assigned = vec![false; adjacency.len()];
    let mut recursive = BTreeSet::new();
    for &start in finished.iter().rev() {
        if assigned[start] {
            continue;
        }
        assigned[start] = true;
        let mut component = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            component.push(node);
            for &target in &reverse[node] {
                if !assigned[target] {
                    assigned[target] = true;
                    stack.push(target);
                }
            }
        }
        let self_recursive = component.len() == 1
            && adjacency[component[0]]
                .iter()
                .any(|target| *target == component[0]);
        if component.len() > 1 || self_recursive {
            recursive.extend(component);
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

#[derive(Clone)]
struct SummaryCallEdge {
    target: usize,
    site: Option<u32>,
    bindings: Vec<LinkedArgumentBinding>,
    guard_paths: Option<Vec<LinkedCallGuardPath>>,
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
    site_path: Vec<Option<u32>>,
    guard_scopes: Option<Vec<LinkedCallGuardScope>>,
    path: String,
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

pub(super) fn project_event_dispatches(
    actions: &[LinkedProjectedSemanticAction],
) -> Vec<LinkedEventDispatch> {
    actions
        .iter()
        .enumerate()
        .filter_map(|(semantic_action_index, action)| {
            let spec = action.contract.as_ref()?.event_dispatch.as_ref()?;
            let mut blockers = BTreeSet::new();
            if spec.mechanism.is_empty() {
                blockers.insert("event dispatch mechanism is empty".to_owned());
            }
            if spec.execution_context.is_empty() {
                blockers.insert("event dispatch execution context is empty".to_owned());
            }
            let expected_names = spec
                .argument_roles
                .iter()
                .map(|binding| binding.argument)
                .collect::<BTreeSet<_>>();
            for argument in &action.arguments {
                if !expected_names.contains(argument.name.as_str()) {
                    blockers.insert(format!(
                        "unexpected semantic argument {} at position {}",
                        argument.name, argument.position
                    ));
                }
            }
            let mut bindings = Vec::new();
            let mut declared_roles = BTreeSet::new();
            let mut declared_arguments = BTreeSet::new();
            for binding in &spec.argument_roles {
                let role = binding.role;
                let name = binding.argument;
                if !declared_roles.insert(role) {
                    blockers.insert(format!("duplicate event role {role}"));
                }
                if !declared_arguments.insert(name) {
                    blockers.insert(format!("duplicate event argument {name}"));
                }
                if role.is_empty() {
                    blockers.insert(format!("semantic argument {name} has an empty event role"));
                }
                if name.is_empty() {
                    blockers.insert(format!("event role {role} has an empty semantic argument"));
                }
                let matching = action
                    .arguments
                    .iter()
                    .filter(|argument| argument.name == name)
                    .collect::<Vec<_>>();
                match matching.as_slice() {
                    [argument] => bindings.push(LinkedEventDispatchBinding {
                        role,
                        argument: (*argument).clone(),
                    }),
                    [] => {
                        blockers
                            .insert(format!("missing semantic argument {name} for role {role}"));
                    }
                    _ => {
                        blockers.insert(format!(
                            "ambiguous semantic argument {name} for role {role}"
                        ));
                    }
                }
            }
            Some(LinkedEventDispatch {
                semantic_action_index,
                mechanism: spec.mechanism,
                execution_context: spec.execution_context,
                receiver: spec.receiver.map(str::to_owned),
                interface_complete: blockers.is_empty(),
                blockers: blockers.into_iter().collect(),
                bindings,
            })
        })
        .collect()
}

fn project_context_fields(
    root: usize,
    functions: &[LinkedIrFunction],
    call_edges: &[Vec<SummaryCallEdge>],
    projection_reachable: &[bool],
    call_graph_closed: bool,
) -> (
    bool,
    Vec<String>,
    Vec<LinkedSummaryContextField>,
    Vec<LinkedProjectedTrampolineCall>,
    Vec<LinkedProjectedSemanticAction>,
    Vec<LinkedEventDispatch>,
) {
    let mut root_arguments = vec![None; usize::from(LINKED_CONTEXT_ARGUMENTS)];
    for argument in 0..LINKED_CONTEXT_ARGUMENTS {
        root_arguments[usize::from(argument)] = Some((argument, 0));
    }
    let mut queue = VecDeque::from([ContextProjectionState {
        function: root,
        argument_map: root_arguments,
        visited_functions: vec![root],
        site_path: Vec::new(),
        guard_scopes: Some(Vec::new()),
        path: functions[root].identity.clone(),
    }]);
    let mut blockers = BTreeSet::new();
    let mut fields = BTreeMap::<(u8, i32, u8), SummaryContextAccumulator>::new();
    let mut trampoline_calls = BTreeSet::new();
    let mut semantic_actions = BTreeSet::new();
    let mut explored = 0_usize;

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
            let site = call
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            let boundary = format!("{operation} via {}", call.target);
            let mut site_path = state.site_path.clone();
            site_path.push(call.site);
            semantic_actions.insert(LinkedProjectedSemanticAction {
                site_path,
                path: format!("{} --semantic@{}--> {}", state.path, site, call.target),
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
                    &state.path,
                ),
                guard_scopes: extend_guard_scopes(
                    state.guard_scopes.as_deref(),
                    &function.identity,
                    call.guard_paths.as_deref(),
                ),
            });
        }
        for call in function
            .calls
            .iter()
            .filter(|call| call.trampoline.is_some())
        {
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
                &state.path,
            );
            trampoline_calls.insert(LinkedProjectedTrampolineCall {
                path: format!(
                    "{} --trampoline@{}+{:#x}--> {}",
                    state.path, trampoline.table, trampoline.slot, trampoline.c_name
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
                blockers.insert(format!(
                    "no affine binding for {} arg{} along {}",
                    function.identity, access.argument, state.path
                ));
                continue;
            };
            let Some(offset) = base_offset.checked_add(access.offset) else {
                blockers.insert(format!(
                    "context offset overflow for {} arg{} along {}",
                    function.identity, access.argument, state.path
                ));
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
            field
                .paths
                .insert(format!("{} / {}", state.path, access.path));
        }

        for edge in &call_edges[state.function] {
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
            let mut site_path = state.site_path.clone();
            site_path.push(edge.site);
            let guard_scopes = extend_guard_scopes(
                state.guard_scopes.as_deref(),
                &function.identity,
                edge.guard_paths.as_deref(),
            );
            let site = edge
                .site
                .map_or_else(|| "composed".to_owned(), |site| format!("{site:#010x}"));
            queue.push_back(ContextProjectionState {
                function: edge.target,
                argument_map,
                visited_functions,
                site_path,
                guard_scopes,
                path: format!(
                    "{} --call@{}--> {}",
                    state.path, site, functions[edge.target].identity
                ),
            });
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
    (
        call_graph_closed && blockers.is_empty(),
        blockers.into_iter().collect(),
        fields,
        trampoline_calls.into_iter().collect(),
        semantic_actions,
        event_dispatches,
    )
}

fn projection_reachability(functions: &[LinkedIrFunction], adjacency: &[Vec<usize>]) -> Vec<bool> {
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

pub(super) fn populate_effect_summaries(functions: &mut [LinkedIrFunction]) {
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

    let summaries = (0..functions.len())
        .map(|root| {
            let mut depths = vec![None; functions.len()];
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
                let function = &functions[index];
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
                .map(|(index, _)| functions[*index].identity.clone())
                .collect();
            let recursive_functions = reachable
                .iter()
                .filter(|(index, _)| recursive_nodes.contains(index))
                .map(|(index, _)| functions[*index].identity.clone())
                .collect();
            let call_graph_closed = blockers.is_empty();
            let (
                context_projection_complete,
                context_projection_blockers,
                context_fields,
                trampoline_calls,
                semantic_actions,
                event_dispatches,
            ) = project_context_fields(
                root,
                functions,
                &call_edges,
                &projection_reachable,
                call_graph_closed,
            );
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
                context_projection_complete,
                context_projection_blockers,
                context_fields,
                trampoline_calls,
                semantic_actions,
                event_dispatches,
            }
        })
        .collect::<Vec<_>>();
    for (function, summary) in functions.iter_mut().zip(summaries) {
        function.effect_summary = summary;
    }
}
