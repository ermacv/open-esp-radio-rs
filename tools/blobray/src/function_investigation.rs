//! Project-aware, lossless function investigation.
//!
//! This report deliberately combines two different truth domains without
//! conflating them: a complete container/decode/CFG view and the best
//! available semantic linked-IR view.  Semantic incompleteness never removes
//! raw instructions or basic blocks.

pub(crate) mod correspondence;
mod model;
mod origin;
mod replacement;

use std::collections::{BTreeMap, BTreeSet};

use crate::{ProjectSpec, Result, artifact, artifacts};
pub use model::*;
use origin::origin_evidence;
use petgraph::{algo::astar, graph::DiGraph};
pub use replacement::{ReplacementEvidence, ReplacementProofEvidence, ReviewedEffectRuleEvidence};
pub(crate) use replacement::{replacement_evidence, reviewed_effect_rules};

const MAX_GRAPH_NODES: usize = 4_096;
const MAX_GRAPH_EDGES: usize = 32_768;

pub(crate) fn investigate(
    request: FunctionInvestigationRequest<'_>,
    project: &ProjectSpec,
) -> Result<FunctionInvestigationReport> {
    let runtime = artifact::inspect_function_body_at(
        request.artifact,
        request.member,
        request.symbol,
        request.runtime_address,
    )?;
    let (origin, origin_ledger) = origin_evidence(&request, &runtime, project)?;
    let semantics = semantic_evidence(SemanticEvidenceRequest {
        source: request.source,
        symbol: request.symbol,
        runtime: &runtime,
        graph_depth: request.graph_depth,
        include_callers: request.include_callers,
        include_linked_ir_record: request.include_linked_ir_record,
        origin: origin.as_ref(),
        project,
    })?;
    let reviewed_address = origin
        .as_ref()
        .and_then(|origin| origin.linked_address)
        .unwrap_or(runtime.address);
    let (reviewed_preconditions, reviewed_paths) =
        reviewed_path_knowledge(request.source, &runtime.symbol, reviewed_address, project)?;
    let replacements = replacement_evidence(request.source, request.symbol, project)?;
    let cfg_path = request
        .cfg_path
        .map(|selection| structural_cfg_path(&runtime, selection))
        .transpose()?;
    let unsupported = runtime
        .instructions
        .iter()
        .filter(|instruction| !instruction.supported)
        .count();
    let reachable_blocks = runtime
        .basic_blocks
        .iter()
        .filter(|block| block.reachable)
        .count();
    let semantic_status = if semantics.is_empty() {
        InvestigationLedgerEntry {
            layer: "semantics",
            status: "unavailable",
            detail: "no generated linked-IR profile contains this exact source/symbol".to_owned(),
        }
    } else if semantics.iter().all(|semantic| semantic.complete) {
        InvestigationLedgerEntry {
            layer: "semantics",
            status: "complete",
            detail: format!("{} linked-IR projection(s) are complete", semantics.len()),
        }
    } else {
        InvestigationLedgerEntry {
            layer: "semantics",
            status: "incomplete",
            detail: format!(
                "{} linked-IR projection(s); raw body remains available below blockers",
                semantics.len()
            ),
        }
    };
    let report = FunctionInvestigationReport {
        schema_version: 15,
        command: "inspect function",
        source: request.source.to_owned(),
        symbol: request.symbol.to_owned(),
        proof_ledger: vec![
            InvestigationLedgerEntry {
                layer: "container",
                status: if runtime.accounted_bytes == runtime.size {
                    "complete"
                } else {
                    "incomplete"
                },
                detail: format!(
                    "{} of {} symbol bytes accounted for",
                    runtime.accounted_bytes, runtime.size
                ),
            },
            InvestigationLedgerEntry {
                layer: "decode",
                status: if unsupported == 0 {
                    "complete"
                } else {
                    "partial"
                },
                detail: format!(
                    "{} instructions, {} explicit unsupported instruction(s)",
                    runtime.instructions.len(),
                    unsupported
                ),
            },
            InvestigationLedgerEntry {
                layer: "cfg",
                status: "conservative",
                detail: format!(
                    "{} basic blocks, {} entry-reachable, {} loop region(s)",
                    runtime.basic_blocks.len(),
                    reachable_blocks,
                    runtime.loops.len(),
                ),
            },
            origin_ledger,
            semantic_status,
        ],
        runtime,
        origin,
        semantics,
        reviewed_preconditions,
        reviewed_paths,
        cfg_path,
        replacements,
    };
    validate_report(&report)?;
    Ok(report)
}

fn validate_report(report: &FunctionInvestigationReport) -> Result<()> {
    if report.schema_version != 15 || report.command != "inspect function" {
        return Err(crate::Error::invalid(
            "function investigation report uses an unsupported schema or command",
        ));
    }
    for semantic in &report.semantics {
        for blocker in &semantic.blockers {
            blocker.resolution_route.validate(&blocker.root_id)?;
        }
    }
    Ok(())
}

fn structural_cfg_path(
    runtime: &artifact::FunctionBody,
    selection: &str,
) -> Result<CfgPathEvidence> {
    let (from, to) = selection.split_once(':').ok_or_else(|| {
        crate::Error::invalid("CFG path must be FROM:TO; use +OFFSET for function offsets")
    })?;
    if from.is_empty() || to.is_empty() || to.contains(':') {
        return Err(crate::Error::invalid(
            "CFG path must contain exactly two non-empty locations",
        ));
    }
    let from_address = cfg_location(runtime, from)?;
    let to_address = cfg_location(runtime, to)?;
    let from_block = cfg_block(runtime, from_address)?;
    let to_block = cfg_block(runtime, to_address)?;

    let mut graph = DiGraph::<usize, ()>::new();
    let nodes = runtime
        .basic_blocks
        .iter()
        .map(|block| (block.id, graph.add_node(block.id)))
        .collect::<BTreeMap<_, _>>();
    for block in &runtime.basic_blocks {
        for successor in &block.successors {
            if let Some(target) = successor.block {
                let Some((&from, &to)) = nodes.get(&block.id).zip(nodes.get(&target)) else {
                    continue;
                };
                graph.add_edge(from, to, ());
            }
        }
    }
    let from_node = nodes[&from_block];
    let to_node = nodes[&to_block];
    let blocks: Vec<usize> = astar(
        &graph,
        from_node,
        |candidate| candidate == to_node,
        |_| 1usize,
        |_| 0usize,
    )
    .map(|(_, nodes)| nodes.into_iter().map(|node| graph[node]).collect())
    .unwrap_or_default();
    Ok(CfgPathEvidence {
        from_address,
        to_address,
        from_block,
        to_block,
        structurally_reachable: !blocks.is_empty(),
        feasibility_claim: false,
        blocks,
    })
}

fn cfg_location(runtime: &artifact::FunctionBody, location: &str) -> Result<u64> {
    let address = if let Some(offset) = location.strip_prefix('+') {
        let offset = crate::parse::u32_literal(offset).ok_or_else(|| {
            crate::Error::invalid(format!("invalid CFG path offset {location:?}"))
        })?;
        runtime.address.checked_add(u64::from(offset))
    } else {
        crate::parse::u32_literal(location).map(u64::from)
    }
    .ok_or_else(|| crate::Error::invalid(format!("invalid CFG path location {location:?}")))?;
    let end = runtime.address.saturating_add(runtime.size as u64);
    if address < runtime.address || address >= end {
        return Err(crate::Error::invalid(format!(
            "CFG path location {location:?} ({address:#010x}) is outside {} at {:#010x}..{end:#010x}",
            runtime.symbol, runtime.address
        )));
    }
    Ok(address)
}

fn cfg_block(runtime: &artifact::FunctionBody, address: u64) -> Result<usize> {
    let offset = address - runtime.address;
    runtime
        .basic_blocks
        .iter()
        .find(|block| offset >= block.start_offset && offset < block.end_offset)
        .map(|block| block.id)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "CFG path address {address:#010x} does not belong to a decoded basic block"
            ))
        })
}

fn reviewed_path_knowledge(
    source: &str,
    symbol: &str,
    address: u64,
    project: &ProjectSpec,
) -> Result<(Vec<ReviewedPreconditionEvidence>, Vec<ReviewedPathEvidence>)> {
    let Some(workspace) = &project.functions else {
        return Ok((Vec::new(), Vec::new()));
    };
    let pack = crate::function_workspace::FunctionPack::load_reviewed(&workspace.pack)?;
    let identity = format!("{source}::{symbol}@{address:#010x}");
    let Some(function) = pack
        .functions
        .iter()
        .find(|function| function.source == source && function.identity == identity)
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    Ok((
        function
            .preconditions
            .iter()
            .map(|precondition| ReviewedPreconditionEvidence {
                id: precondition.id.clone(),
                expression: precondition.expression.clone(),
                rationale: precondition.rationale.clone(),
            })
            .collect(),
        function
            .paths
            .iter()
            .map(|path| ReviewedPathEvidence {
                id: path.id.clone(),
                class: path.class.clone(),
                summary: path.summary.clone(),
                evidence: path.evidence.clone(),
            })
            .collect(),
    ))
}

struct SemanticEvidenceRequest<'a> {
    source: &'a str,
    symbol: &'a str,
    runtime: &'a artifact::FunctionBody,
    graph_depth: usize,
    include_callers: bool,
    include_linked_ir_record: bool,
    origin: Option<&'a OriginFunctionEvidence>,
    project: &'a ProjectSpec,
}

fn semantic_evidence(
    request: SemanticEvidenceRequest<'_>,
) -> Result<Vec<SemanticFunctionEvidence>> {
    let SemanticEvidenceRequest {
        source,
        symbol,
        runtime,
        graph_depth,
        include_callers,
        include_linked_ir_record,
        origin,
        project,
    } = request;
    let mut evidence = Vec::new();
    let function_pack = project
        .functions
        .as_ref()
        .map(|workspace| crate::function_workspace::FunctionPack::load_reviewed(&workspace.pack))
        .transpose()?;
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| profile.sources.iter().any(|candidate| candidate == source))
    {
        if !profile.output.is_dir() {
            continue;
        }
        let reader = artifacts::LinkedIrReader::open(&profile.output)?;
        let (member, address) =
            origin.map_or((runtime.member.as_deref(), runtime.address), |origin| {
                (
                    origin.linked_member.as_deref(),
                    origin.linked_address.unwrap_or(runtime.address),
                )
            });
        let Some(function) = reader.get_function(source, symbol, member, address)? else {
            continue;
        };
        let complete = function.completeness.executable_complete;
        let exact = function.exact();
        let reviewed = function_pack.as_ref().and_then(|pack| {
            pack.functions.iter().find(|reviewed| {
                reviewed.profile == profile.id
                    && reviewed.source == source
                    && reviewed.identity == function.identity
            })
        });
        let reviewed_signature = reviewed
            .and_then(|reviewed| {
                reviewed
                    .signature
                    .as_ref()
                    .map(|signature| (reviewed, signature))
            })
            .map(|(reviewed, signature)| ReviewedFunctionSignatureEvidence {
                name: reviewed.name.clone().unwrap_or_else(|| symbol.to_owned()),
                arguments: signature
                    .arguments
                    .iter()
                    .map(|argument| ReviewedFunctionArgumentEvidence {
                        index: argument.index,
                        name: argument.name.clone(),
                        abi: argument.abi.clone(),
                        role: argument.role.clone(),
                    })
                    .collect(),
                return_abi: signature.return_abi.clone(),
                return_role: signature.return_role.clone(),
                provenance: "reviewed-function-pack",
            });
        let pseudo = function_pack.as_ref().map_or_else(
            || function.pseudo.clone(),
            |pack| {
                apply_reviewed_call_signatures(
                    &function.pseudo,
                    profile.id.as_str(),
                    source,
                    &function.calls,
                    pack,
                )
            },
        );
        let pseudo = reviewed_signature
            .as_ref()
            .map_or(pseudo.clone(), |signature| {
                apply_reviewed_signature(&pseudo, signature)
            });
        let mut blockers = function
            .diagnostics()
            .map(|(layer, diagnostic)| {
                let resolution_route = crate::blocker_resolution::blocker_resolution_route(
                    project,
                    &diagnostic.root_id,
                    &diagnostic.kind,
                    &diagnostic.rendered,
                );
                BlockerExplanationEvidence {
                    root_id: diagnostic.root_id.clone(),
                    layer: layer.to_owned(),
                    kind: diagnostic.kind.clone(),
                    site: diagnostic.site,
                    message: diagnostic.rendered.clone(),
                    resolution_route,
                    relocation_candidates: diagnostic
                        .site
                        .map(|site| relocation_candidates_at(runtime, origin, site))
                        .unwrap_or_default(),
                    provenance: crate::FactProvenance::Derived,
                    accuracy: crate::FactAccuracy::Exact,
                    completeness: crate::FactCompleteness::Partial,
                }
            })
            .collect::<Vec<_>>();
        let calls = function
            .calls
            .iter()
            .map(|call| call_knowledge(call, &function, &reader))
            .collect::<Vec<_>>();
        let search_limits = artifacts::GraphSearchLimits {
            max_depth: graph_depth,
            max_visited_nodes: MAX_GRAPH_NODES,
            max_examined_edges: MAX_GRAPH_EDGES,
        };
        let reachability = reader.reachable_from(&function.identity, search_limits)?;
        let reachable_functions = reachability
            .identities
            .iter()
            .filter(|identity| *identity != &function.identity)
            .cloned()
            .collect::<Vec<_>>();
        let graph_slice = reader.graph_slice(
            &function.identity,
            graph_depth,
            include_callers,
            search_limits,
        )?;
        let graph_limit = graph_slice.limit.or(reachability.limit);
        if let Some(limit) = graph_limit.filter(|limit| *limit != "max-depth") {
            let root_id = format!("inspect-graph-limit:{}", function.identity);
            let message = format!("focused graph traversal reached {limit}");
            let resolution_route = crate::blocker_resolution::blocker_resolution_route(
                project, &root_id, limit, &message,
            );
            blockers.push(BlockerExplanationEvidence {
                root_id,
                layer: "navigation".to_owned(),
                kind: limit.to_owned(),
                site: None,
                message,
                resolution_route,
                relocation_candidates: Vec::new(),
                provenance: crate::FactProvenance::Derived,
                accuracy: crate::FactAccuracy::Bounded,
                completeness: crate::FactCompleteness::Partial,
            });
        }
        let mut call_graph_edges = graph_slice
            .edges
            .into_iter()
            .map(|edge| CallGraphEdgeEvidence {
                caller: edge.caller,
                callee: edge.callee,
                site: edge.site,
                kind: edge.kind,
            })
            .collect::<Vec<_>>();
        call_graph_edges.sort_by(|left, right| {
            (&left.caller, left.site, &left.callee).cmp(&(&right.caller, right.site, &right.callee))
        });
        let event_dispatches = function
            .effect_summary
            .event_dispatches
            .iter()
            .map(|dispatch| EventDispatchEvidence {
                semantic_action_index: dispatch.semantic_action_index,
                mechanism: dispatch.mechanism.clone(),
                execution_context: dispatch.execution_context.clone(),
                receiver: dispatch.receiver.clone(),
                interface_complete: dispatch.interface_complete,
                bindings: dispatch
                    .bindings
                    .iter()
                    .map(|binding| EventDispatchBindingEvidence {
                        role: binding.role.clone(),
                        value: binding.argument.value().to_owned(),
                    })
                    .collect(),
                blockers: dispatch.blockers.clone(),
            })
            .collect::<Vec<_>>();
        let mut reviewed_event_routes = Vec::new();
        for route in function_pack
            .as_ref()
            .into_iter()
            .flat_map(|pack| &pack.event_routes)
            .filter(|route| {
                route.profile == profile.id
                    && route.source == source
                    && route.dispatcher == function.identity
            })
        {
            reviewed_event_routes.push(event_route_evidence(route, &event_dispatches, project)?);
        }
        let linked_ir = include_linked_ir_record
            .then(|| StoredLinkedIrRecord::from_function(&function))
            .transpose()?;
        let instruction_evidence = instruction_evidence(runtime, &function, &blockers);
        evidence.push(SemanticFunctionEvidence {
            profile: profile.id.clone(),
            report: profile.output.display().to_string(),
            complete,
            exact,
            reviewed_signature,
            pseudo,
            blockers,
            instruction_evidence,
            calls,
            reachable_functions,
            call_graph_edges,
            graph_limits: InvestigationGraphLimits {
                max_depth: graph_depth,
                max_visited_nodes: MAX_GRAPH_NODES,
                max_examined_edges: MAX_GRAPH_EDGES,
                visited_nodes: graph_slice.visited_nodes.max(reachability.visited_nodes),
                examined_edges: graph_slice.examined_edges.max(reachability.examined_edges),
                reached: graph_limit.map(str::to_owned),
            },
            event_dispatches,
            reviewed_event_routes,
            linked_ir,
        });
    }
    Ok(evidence)
}

fn abi_pseudo_type(abi: &str) -> &str {
    match abi {
        "void" => "()",
        "ptr" | "mut-ptr" | "out-ptr" => "*mut opaque",
        "const-ptr" => "*const opaque",
        "fn-ptr" => "fn_ptr",
        "opaque-handle" => "opaque_handle",
        value => value,
    }
}

fn replace_identifier(input: &str, from: &str, to: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = input[cursor..].find(from) {
        let start = cursor + relative;
        let end = start + from.len();
        let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
        let left_clear = start == 0 || !identifier(input.as_bytes()[start - 1]);
        let right_clear = end == input.len() || !identifier(input.as_bytes()[end]);
        output.push_str(&input[cursor..start]);
        if left_clear && right_clear {
            output.push_str(to);
        } else {
            output.push_str(from);
        }
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn apply_reviewed_signature(pseudo: &str, signature: &ReviewedFunctionSignatureEvidence) -> String {
    let arguments = signature
        .arguments
        .iter()
        .map(|argument| format!("{}: {}", argument.name, abi_pseudo_type(&argument.abi)))
        .collect::<Vec<_>>()
        .join(", ");
    let header = format!(
        "fn {}({arguments}) -> {} {{",
        signature.name,
        signature
            .return_abi
            .as_deref()
            .map(abi_pseudo_type)
            .unwrap_or("unknown_abi")
    );
    let mut output = String::new();
    for line in pseudo.lines() {
        if line.starts_with("fn ") {
            output
                .push_str("// REVIEWED-SIGNATURE: function pack; body remains generated facts.\n");
            output.push_str(&header);
        } else if line.trim_start().starts_with("// argN denotes args[N]") {
            output.push_str("    // Argument names below are reviewed aliases for raw ABI inputs.");
        } else {
            output.push_str(line);
        }
        output.push('\n');
    }
    for argument in signature.arguments.iter().rev() {
        output = replace_identifier(
            &output,
            &format!("ctx{}", argument.index),
            &format!("{}_memory", argument.name),
        );
        output = replace_identifier(&output, &format!("arg{}", argument.index), &argument.name);
    }
    output
}

fn pseudo_call_identifier(identity: &str) -> String {
    let mut output = identity
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if output.is_empty() {
        output.push_str("unnamed");
    } else if output.as_bytes()[0].is_ascii_digit() {
        output.insert_str(0, "fn_");
    }
    output
}

fn truncate_call_arguments(line: &str, callee: &str, argument_count: usize) -> String {
    let needle = format!("{callee}(");
    let mut output = line.to_owned();
    let mut cursor = 0;
    while let Some(relative_start) = output[cursor..].find(&needle) {
        let open = cursor + relative_start + needle.len() - 1;
        let mut parenthesis_depth = 0_usize;
        let mut bracket_depth = 0_usize;
        let mut close = None;
        for (relative, character) in output[open..].char_indices() {
            match character {
                '(' => parenthesis_depth += 1,
                ')' => {
                    parenthesis_depth = parenthesis_depth.saturating_sub(1);
                    if parenthesis_depth == 0 {
                        close = Some(open + relative);
                        break;
                    }
                }
                '[' => bracket_depth += 1,
                ']' => bracket_depth = bracket_depth.saturating_sub(1),
                _ => {}
            }
        }
        let Some(close) = close else {
            break;
        };
        let arguments = &output[open + 1..close];
        let mut ends = Vec::new();
        let mut nested_parentheses = 0_usize;
        let mut nested_brackets = 0_usize;
        for (relative, character) in arguments.char_indices() {
            match character {
                '(' => nested_parentheses += 1,
                ')' => nested_parentheses = nested_parentheses.saturating_sub(1),
                '[' => nested_brackets += 1,
                ']' => nested_brackets = nested_brackets.saturating_sub(1),
                ',' if nested_parentheses == 0 && nested_brackets == 0 => ends.push(relative),
                _ => {}
            }
        }
        let retained = if argument_count == 0 {
            String::new()
        } else if argument_count > ends.len() {
            arguments.to_owned()
        } else {
            arguments[..ends[argument_count - 1]].to_owned()
        };
        let retained = retained.trim_end().to_owned();
        output.replace_range(open + 1..close, &retained);
        cursor = open + retained.len() + 2;
    }
    output
}

fn apply_reviewed_call_signatures(
    pseudo: &str,
    profile: &str,
    source: &str,
    calls: &[artifacts::StoredCall],
    pack: &crate::function_workspace::FunctionPack,
) -> String {
    let mut output = pseudo.to_owned();
    let mut applied = BTreeSet::new();
    for call in calls {
        if !applied.insert(call.target.as_str()) {
            continue;
        }
        let Some(signature) = pack.functions.iter().find_map(|reviewed| {
            (reviewed.profile == profile
                && reviewed.source == source
                && reviewed.identity == call.target)
                .then_some(reviewed.signature.as_ref())
                .flatten()
        }) else {
            continue;
        };
        output = output
            .lines()
            .map(|line| {
                truncate_call_arguments(
                    line,
                    &pseudo_call_identifier(&call.target),
                    signature.arguments.len(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        output.push('\n');
    }
    output
}

fn call_knowledge(
    call: &artifacts::StoredCall,
    function: &artifacts::StoredFunction,
    reader: &artifacts::LinkedIrReader,
) -> CallKnowledgeEvidence {
    let unresolved_target = call.kind.contains("unresolved")
        || (call.kind == "structural-relocation" && call.project_symbol().is_some());
    let mut target_candidates = call.project_candidates().to_vec();
    if target_candidates.is_empty() && call.target.contains(" | ") {
        target_candidates = call.target.split(" | ").map(str::to_owned).collect();
    }
    let target_status = if unresolved_target {
        "unresolved"
    } else if target_candidates.len() > 1 || call.kind.contains("ambiguous") {
        "candidates"
    } else {
        "exact"
    };
    let target_blocker = match target_status {
        "unresolved" => {
            Some("the call instruction is known, but no unique target is proven".to_owned())
        }
        "candidates" => Some(
            "multiple structurally valid targets remain; linker/runtime selection is not proven"
                .to_owned(),
        ),
        _ => None,
    };
    let argument_evidence = call
        .arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            let status = if value == "unknown"
                || value.contains("varies-across")
                || value.contains("bits:0=?")
            {
                "unresolved"
            } else if value.starts_with("one-of(") || value.contains("unknown") {
                "partial"
            } else {
                "exact"
            };
            CallArgumentEvidence {
                position,
                status,
                value: argument_value_with_labels(value, reader),
                provenance: if call.argument_shapes() > 1 {
                    format!(
                        "forward symbolic execution at this callsite across {} guarded shapes",
                        call.argument_shapes()
                    )
                } else {
                    "forward symbolic execution of the ABI register/stack state at this callsite"
                        .to_owned()
                },
            }
        })
        .collect::<Vec<_>>();
    let mut provenance = Vec::new();
    if let Some(value) = call.semantic_contract_provenance() {
        provenance.push(value);
    }
    if let Some(value) = call.trampoline_provenance() {
        provenance.push(value);
    }
    if let Some(site) = call.site {
        provenance.extend(
            function
                .projected_relocations
                .iter()
                .filter(|relocation| {
                    relocation.site <= site && site.saturating_sub(relocation.site) <= 64
                })
                .map(|relocation| {
                    format!(
                        "linked {:#010x} <- {:?}::{} +{} [{} {} -> {}{:+#x}]",
                        relocation.site,
                        relocation.origin_member,
                        relocation.origin_symbol,
                        relocation
                            .origin_offsets
                            .iter()
                            .map(|offset| format!("{offset:#x}"))
                            .collect::<Vec<_>>()
                            .join(","),
                        relocation.correspondence,
                        relocation.kind,
                        relocation.symbol,
                        relocation.addend,
                    )
                }),
        );
    }
    provenance.sort();
    provenance.dedup();
    CallKnowledgeEvidence {
        kind: call.kind.clone(),
        target: call.target.clone(),
        site: call.site,
        target_status,
        target_candidates,
        target_blocker,
        knowledge: call.knowledge(),
        semantic_operation: call.semantic_operation.clone(),
        execution_model: call.execution_model_id().map(str::to_owned),
        arguments: call.arguments.clone(),
        argument_evidence,
        argument_shapes: call.argument_shapes(),
        guards: call.guard_expressions(),
        provenance,
    }
}

fn argument_value_with_labels(value: &str, reader: &artifacts::LinkedIrReader) -> String {
    let value = compact_argument_value(value);
    let Some(address) = value
        .strip_prefix("const:0x")
        .and_then(|value| u32::from_str_radix(value, 16).ok())
    else {
        return value;
    };
    let labels = reader.labels_at_address(address);
    if labels.is_empty() {
        value
    } else {
        format!("{value} ({})", labels.join(" | "))
    }
}

fn compact_argument_value(value: &str) -> String {
    if let Some(value) = value
        .strip_suffix("&0xffffffff|0x00000000")
        .filter(|value| !value.is_empty())
    {
        return value.to_owned();
    }
    let Some(bits) = value.strip_prefix("bits:") else {
        return value.to_owned();
    };
    let fields = bits.split(',').collect::<Vec<_>>();
    if fields.len() != 32 {
        return value.to_owned();
    }
    let Some(base) = fields[0]
        .strip_prefix("0=")
        .and_then(|field| field.strip_suffix(".0"))
    else {
        return value.to_owned();
    };
    let aligned = fields.iter().enumerate().all(|(bit, field)| {
        field
            .strip_prefix(&format!("{bit}="))
            .is_some_and(|source| source == format!("{base}.{bit}"))
    });
    if aligned {
        base.to_owned()
    } else {
        value.to_owned()
    }
}

fn event_route_evidence(
    route: &crate::function_workspace::ReviewedEventRoute,
    dispatches: &[EventDispatchEvidence],
    project: &ProjectSpec,
) -> Result<ReviewedEventRouteEvidence> {
    let selector = format!("const:{:#010x}", route.selector_value);
    let dispatch_constraint_matched = dispatches.iter().any(|dispatch| {
        dispatch.mechanism == route.mechanism
            && dispatch.execution_context == route.execution_context
            && route
                .receiver
                .as_ref()
                .is_none_or(|receiver| dispatch.receiver.as_ref() == Some(receiver))
            && dispatch.interface_complete
            && dispatch
                .bindings
                .iter()
                .any(|binding| binding.role == route.selector_role && binding.value == selector)
    });
    let mut blockers = Vec::new();
    if !dispatch_constraint_matched {
        blockers.push(format!(
            "generated dispatch does not prove {} {}={selector}",
            route.mechanism, route.selector_role
        ));
    }
    let consumer_analysis = dispatch_constraint_matched
        .then(|| {
            event_function_analysis(
                project,
                &route.consumer_profile,
                &route.consumer_source,
                &route.consumer_entry,
                "consumer entry",
                &mut blockers,
            )
        })
        .transpose()?
        .flatten();
    let case_handler_analysis = if dispatch_constraint_matched {
        route
            .case_handler
            .as_ref()
            .map(|handler| {
                event_function_analysis(
                    project,
                    &handler.profile,
                    &handler.source,
                    &handler.function,
                    "case handler",
                    &mut blockers,
                )
            })
            .transpose()?
            .flatten()
    } else {
        None
    };
    Ok(reviewed_event_route(
        route,
        dispatch_constraint_matched,
        consumer_analysis,
        case_handler_analysis,
        blockers,
    ))
}

fn event_function_analysis(
    project: &ProjectSpec,
    profile_id: &str,
    source: &str,
    identity: &str,
    role: &str,
    blockers: &mut Vec<String>,
) -> Result<Option<EventHandlerAnalysisEvidence>> {
    let Some(profile) = project
        .ir_profiles
        .iter()
        .find(|profile| profile.id == profile_id)
    else {
        blockers.push(format!("{role} profile {profile_id:?} is not configured"));
        return Ok(None);
    };
    if !profile.output.is_dir() {
        blockers.push(format!(
            "{role} profile {profile_id:?} has not been generated"
        ));
        return Ok(None);
    }
    let reader = artifacts::LinkedIrReader::open(&profile.output)?;
    match reader.get_function_by_identity(identity)? {
        Some(function) if function.source == source => {
            let function_blockers = function.blockers().map(str::to_owned).collect();
            let reachability_depth = 12;
            let reachability = reader.reachable_from(
                &function.identity,
                artifacts::GraphSearchLimits {
                    max_depth: reachability_depth,
                    max_visited_nodes: MAX_GRAPH_NODES,
                    max_examined_edges: MAX_GRAPH_EDGES,
                },
            )?;
            Ok(Some(EventHandlerAnalysisEvidence {
                identity: function.identity.clone(),
                complete: function.completeness.executable_complete,
                exact: function.exact(),
                direct_instruction_effects: function.direct_instruction_effect_count(),
                direct_calls: function.direct_call_count(),
                reachable_functions: reachability.identities.len().saturating_sub(1),
                reachability_depth,
                reachability_limit: reachability.limit.map(str::to_owned),
                blockers: function_blockers,
            }))
        }
        Some(_) => {
            blockers.push(format!(
                "{role} {identity:?} does not belong to source {source:?}"
            ));
            Ok(None)
        }
        None => {
            blockers.push(format!(
                "{role} {identity:?} is absent from profile {profile_id:?}"
            ));
            Ok(None)
        }
    }
}

fn reviewed_event_route(
    route: &crate::function_workspace::ReviewedEventRoute,
    dispatch_constraint_matched: bool,
    consumer_analysis: Option<EventHandlerAnalysisEvidence>,
    case_handler_analysis: Option<EventHandlerAnalysisEvidence>,
    blockers: Vec<String>,
) -> ReviewedEventRouteEvidence {
    ReviewedEventRouteEvidence {
        id: route.id.clone(),
        mechanism: route.mechanism.clone(),
        selector_role: route.selector_role.clone(),
        selector_value: route.selector_value,
        receiver: route.receiver.clone(),
        execution_context: route.execution_context.clone(),
        consumer_profile: route.consumer_profile.clone(),
        consumer_source: route.consumer_source.clone(),
        consumer_entry: route.consumer_entry.clone(),
        delivery_operation: route.delivery.operation.clone(),
        delivery_output_role: route.delivery.output_role.clone(),
        delivery_selector_offset: route.delivery.selector_offset,
        delivery_selector_width: route.delivery.selector_width,
        delivery_encoding: route.delivery.encoding.clone(),
        case_handler_profile: route
            .case_handler
            .as_ref()
            .map(|handler| handler.profile.clone()),
        case_handler_source: route
            .case_handler
            .as_ref()
            .map(|handler| handler.source.clone()),
        case_handler: route
            .case_handler
            .as_ref()
            .map(|handler| handler.function.clone()),
        rationale: route.rationale.clone(),
        dispatch_constraint_matched,
        consumer_analysis,
        case_handler_analysis,
        blockers,
    }
}

fn relocation_candidates_at(
    runtime: &artifact::FunctionBody,
    origin: Option<&OriginFunctionEvidence>,
    site: u32,
) -> Vec<String> {
    runtime
        .instructions
        .iter()
        .filter(|instruction| {
            instruction.address == u64::from(site) || instruction.offset == u64::from(site)
        })
        .flat_map(|instruction| instruction.relocations.iter())
        .map(|relocation| relocation.symbol.clone())
        .chain(
            origin
                .into_iter()
                .flat_map(|origin| &origin.instruction_correspondence)
                .filter(move |correspondence| correspondence.runtime_address == u64::from(site))
                .flat_map(|correspondence| correspondence.relocation_symbols.iter().cloned()),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn instruction_evidence(
    runtime: &artifact::FunctionBody,
    function: &artifacts::StoredFunction,
    blockers: &[BlockerExplanationEvidence],
) -> Vec<InstructionEvidence> {
    #[derive(Default)]
    struct Evidence {
        effects: BTreeSet<InstructionEffectEvidence>,
        calls: BTreeSet<String>,
        operations: BTreeSet<String>,
        blockers: BTreeSet<String>,
        decode: Option<String>,
    }
    let mut sites = BTreeMap::<u64, Evidence>::new();
    for effect in &function.instruction_effects {
        let (kind, access, width, target, paths, guards, value) = effect.investigation_fields();
        sites
            .entry(u64::from(effect.site()))
            .or_default()
            .effects
            .insert(InstructionEffectEvidence {
                kind,
                access: access.to_owned(),
                width,
                target,
                paths: paths.to_vec(),
                guards: guards.to_vec(),
                value: value.map(str::to_owned),
            });
    }
    for call in &function.calls {
        let Some(site) = call.site else { continue };
        let evidence = sites.entry(u64::from(site)).or_default();
        evidence.calls.insert(call.target.clone());
        if let Some(operation) = &call.semantic_operation {
            evidence.operations.insert(operation.clone());
        }
    }
    for blocker in blockers {
        let Some(site) = blocker.site else { continue };
        sites
            .entry(u64::from(site))
            .or_default()
            .blockers
            .insert(blocker.root_id.clone());
    }
    for blocker in &function.decode_blockers {
        sites.entry(blocker.address).or_default().decode = Some(blocker.class.clone());
    }
    sites
        .into_iter()
        .map(|(address, evidence)| InstructionEvidence {
            address,
            block: runtime.basic_blocks.iter().find_map(|block| {
                let offset = address.checked_sub(runtime.address)?;
                (offset >= block.start_offset && offset < block.end_offset).then_some(block.id)
            }),
            effects: evidence.effects.into_iter().collect(),
            call_targets: evidence.calls.into_iter().collect(),
            semantic_operations: evidence.operations.into_iter().collect(),
            blocker_ids: evidence.blockers.into_iter().collect(),
            decode_blocker: evidence.decode,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_cfg_path_is_explicitly_not_an_execution_claim() {
        let runtime = artifact::FunctionBody {
            artifact: "fixture.elf".to_owned(),
            member: None,
            symbol: "root".to_owned(),
            address: 0x1000,
            size: 12,
            addresses_resolved: true,
            accounted_bytes: 12,
            instructions: Vec::new(),
            basic_blocks: vec![
                artifact::FunctionBasicBlock {
                    id: 0,
                    start_offset: 0,
                    end_offset: 4,
                    reachable: true,
                    successors: vec![artifact::FunctionBlockSuccessor {
                        kind: "fallthrough".to_owned(),
                        block: Some(1),
                        target: Some(0x1004),
                    }],
                },
                artifact::FunctionBasicBlock {
                    id: 1,
                    start_offset: 4,
                    end_offset: 8,
                    reachable: true,
                    successors: vec![artifact::FunctionBlockSuccessor {
                        kind: "branch".to_owned(),
                        block: Some(2),
                        target: Some(0x1008),
                    }],
                },
                artifact::FunctionBasicBlock {
                    id: 2,
                    start_offset: 8,
                    end_offset: 12,
                    reachable: true,
                    successors: Vec::new(),
                },
            ],
            loops: Vec::new(),
            labels: Vec::new(),
        };

        let path = structural_cfg_path(&runtime, "+0x0:+0x8").unwrap();
        assert_eq!(path.blocks, [0, 1, 2]);
        assert!(path.structurally_reachable);
        assert!(!path.feasibility_claim);
    }

    #[test]
    fn argument_display_compacts_full_width_symbolic_sources() {
        let bits = (0..32)
            .map(|bit| format!("{bit}=result_of_stack_size.return.{bit}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            compact_argument_value(&format!("bits:{bits}")),
            "result_of_stack_size.return"
        );
        assert_eq!(
            compact_argument_value("ram:read0&0xffffffff|0x00000000"),
            "ram:read0"
        );
    }

    #[test]
    fn reviewed_signature_names_only_reviewed_abi_inputs() {
        let signature = ReviewedFunctionSignatureEvidence {
            name: "select_phy_channel".to_owned(),
            arguments: vec![
                ReviewedFunctionArgumentEvidence {
                    index: 0,
                    name: "channel_or_frequency".to_owned(),
                    abi: "u32".to_owned(),
                    role: Some("channel-or-frequency".to_owned()),
                },
                ReviewedFunctionArgumentEvidence {
                    index: 1,
                    name: "cbw".to_owned(),
                    abi: "u32".to_owned(),
                    role: Some("channel-bandwidth".to_owned()),
                },
            ],
            return_abi: None,
            return_role: None,
            provenance: "reviewed-function-pack",
        };
        let pseudo = apply_reviewed_signature(
            "// vendor symbol: archive::phy_chip_set_chan\nfn archive__phy_chip_set_chan(args: [u32; 16]) -> u32 {\n    // argN denotes args[N]; ctxN denotes memory rooted at pointer argument N.\n    let call0 = helper(arg0, arg1, abi_inputs[2..16]);\n    return arg10;\n}\n",
            &signature,
        );
        assert!(
            pseudo.contains(
                "fn select_phy_channel(channel_or_frequency: u32, cbw: u32) -> unknown_abi"
            )
        );
        assert!(pseudo.contains("helper(channel_or_frequency, cbw, abi_inputs[2..16])"));
        assert!(pseudo.contains("return arg10;"));
        assert!(!pseudo.contains("args: [u32; 16]"));
    }

    #[test]
    fn reviewed_callee_arity_hides_unrelated_live_abi_registers() {
        let line = "    let call0 = libpp__lmacInitAc(0x2, 0x3, nested(arg0, arg1), 0xa, 0x0, unknown_abi_inputs[5..12], arg8);";

        let truncated = truncate_call_arguments(line, "libpp__lmacInitAc", 5);

        assert_eq!(
            truncated,
            "    let call0 = libpp__lmacInitAc(0x2, 0x3, nested(arg0, arg1), 0xa, 0x0);"
        );
        assert_eq!(
            truncate_call_arguments("return libpp__rcAttach(call5, arg1);", "libpp__rcAttach", 0),
            "return libpp__rcAttach();"
        );
    }
}
