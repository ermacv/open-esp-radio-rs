//! Project-aware, lossless function investigation.
//!
//! This report deliberately combines two different truth domains without
//! conflating them: a complete container/decode/CFG view and the best
//! available semantic linked-IR view.  Semantic incompleteness never removes
//! raw instructions or basic blocks.

mod correspondence;
mod origin;
mod replacement;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use petgraph::{algo::astar, graph::DiGraph};
use serde::Serialize;

use crate::{ProjectSpec, Result, artifact, artifacts};
use origin::origin_evidence;
pub(crate) use replacement::replacement_evidence;
pub use replacement::{ReplacementEvidence, ReplacementProofEvidence};

const MAX_GRAPH_NODES: usize = 4_096;
const MAX_GRAPH_EDGES: usize = 32_768;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FunctionInvestigationReport {
    pub schema_version: u32,
    pub command: &'static str,
    pub source: String,
    pub symbol: String,
    pub runtime: artifact::FunctionBody,
    pub origin: Option<OriginFunctionEvidence>,
    pub semantics: Vec<SemanticFunctionEvidence>,
    pub reviewed_preconditions: Vec<ReviewedPreconditionEvidence>,
    pub reviewed_paths: Vec<ReviewedPathEvidence>,
    pub cfg_path: Option<CfgPathEvidence>,
    pub proof_ledger: Vec<InvestigationLedgerEntry>,
    pub replacements: Vec<ReplacementEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CfgPathEvidence {
    pub from_address: u64,
    pub to_address: u64,
    pub from_block: usize,
    pub to_block: usize,
    pub structurally_reachable: bool,
    /// Always false: graph reachability alone does not prove satisfiable
    /// branch predicates or a realizable runtime state.
    pub feasibility_claim: bool,
    pub blocks: Vec<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedPreconditionEvidence {
    pub id: String,
    pub expression: String,
    pub rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedPathEvidence {
    pub id: String,
    pub class: String,
    pub summary: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginFunctionEvidence {
    pub association: &'static str,
    pub inventory_report: Option<String>,
    /// Authoritative address selected by the already generated link unit.
    /// This remains an association claim, not a reconstruction of linker
    /// selection, but lets an archive inspection reuse the matching linked IR.
    pub linked_address: Option<u64>,
    pub linked_member: Option<String>,
    /// Relocation-backed dependencies retained by the relocatable archive
    /// member. These are never projected onto linked instruction addresses by
    /// offset arithmetic: linker relaxation can change both instruction count
    /// and offsets, so an offset-only association would be unsound.
    pub relocation_dependencies: Vec<OriginRelocationDependency>,
    /// Monotonic structural correspondence between relocation-bearing origin
    /// instructions and linked instructions. This is navigation evidence,
    /// never an execution or semantic-equivalence claim.
    pub instruction_correspondence: Vec<OriginInstructionCorrespondence>,
    pub body: artifact::FunctionBody,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginRelocationDependency {
    pub symbol: String,
    pub references: usize,
    pub instruction_offsets: Vec<u64>,
    pub kinds: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OriginInstructionCorrespondence {
    pub origin_offsets: Vec<u64>,
    pub runtime_address: u64,
    pub runtime_offset: u64,
    pub kind: &'static str,
    pub relocation_symbols: Vec<String>,
    /// Always false. Structural instruction alignment helps investigation but
    /// does not prove identical runtime semantics after linker rewriting.
    pub semantic_equivalence_claim: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SemanticFunctionEvidence {
    pub profile: String,
    pub report: String,
    pub complete: bool,
    pub exact: bool,
    pub pseudo: String,
    pub blockers: Vec<BlockerExplanationEvidence>,
    pub instruction_evidence: Vec<InstructionEvidence>,
    pub calls: Vec<CallKnowledgeEvidence>,
    pub reachable_functions: Vec<String>,
    pub call_graph_edges: Vec<CallGraphEdgeEvidence>,
    pub graph_limits: InvestigationGraphLimits,
    pub event_dispatches: Vec<EventDispatchEvidence>,
    pub reviewed_event_routes: Vec<ReviewedEventRouteEvidence>,
    /// Schema-validated persistent function record. Keeping the complete
    /// record prevents this focused view from silently dropping new semantic
    /// evidence when the linked-IR schema grows.
    pub linked_ir: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvestigationGraphLimits {
    pub max_depth: usize,
    pub max_visited_nodes: usize,
    pub max_examined_edges: usize,
    pub visited_nodes: usize,
    pub examined_edges: usize,
    pub reached: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedEventRouteEvidence {
    pub id: String,
    pub mechanism: String,
    pub selector_role: String,
    pub selector_value: u32,
    pub receiver: Option<String>,
    pub execution_context: String,
    pub consumer_profile: String,
    pub consumer_source: String,
    pub consumer_entry: String,
    pub delivery_operation: String,
    pub delivery_output_role: String,
    pub delivery_selector_offset: u32,
    pub delivery_selector_width: u8,
    pub delivery_encoding: String,
    pub case_handler_profile: Option<String>,
    pub case_handler_source: Option<String>,
    pub case_handler: Option<String>,
    pub rationale: String,
    pub dispatch_constraint_matched: bool,
    pub consumer_analysis: Option<EventHandlerAnalysisEvidence>,
    pub case_handler_analysis: Option<EventHandlerAnalysisEvidence>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventHandlerAnalysisEvidence {
    pub identity: String,
    pub complete: bool,
    pub exact: bool,
    pub direct_instruction_effects: usize,
    pub direct_calls: usize,
    pub reachable_functions: usize,
    pub reachability_depth: usize,
    pub reachability_limit: Option<String>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BlockerExplanationEvidence {
    pub root_id: String,
    pub layer: String,
    pub kind: String,
    pub site: Option<u32>,
    pub message: String,
    pub required_model: String,
    pub relocation_candidates: Vec<String>,
    pub confidence: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InstructionEvidence {
    pub address: u64,
    pub block: Option<usize>,
    pub effects: Vec<InstructionEffectEvidence>,
    pub call_targets: Vec<String>,
    pub semantic_operations: Vec<String>,
    pub blocker_ids: Vec<String>,
    pub decode_blocker: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct InstructionEffectEvidence {
    pub kind: &'static str,
    pub access: String,
    pub width: u8,
    pub target: String,
    pub paths: Vec<String>,
    pub guards: Vec<String>,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventDispatchEvidence {
    pub semantic_action_index: usize,
    pub mechanism: String,
    pub execution_context: String,
    pub receiver: Option<String>,
    pub interface_complete: bool,
    pub bindings: Vec<EventDispatchBindingEvidence>,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventDispatchBindingEvidence {
    pub role: String,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallGraphEdgeEvidence {
    pub caller: String,
    pub callee: String,
    pub site: Option<u32>,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallKnowledgeEvidence {
    pub kind: String,
    pub target: String,
    pub site: Option<u32>,
    pub knowledge: &'static str,
    pub semantic_operation: Option<String>,
    pub execution_model: Option<String>,
    /// ABI argument expressions recovered at this exact call site. Multiple
    /// branch shapes are retained as an explicit domain by linked IR.
    pub arguments: Vec<String>,
    pub argument_shapes: usize,
    pub guards: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InvestigationLedgerEntry {
    pub layer: &'static str,
    pub status: &'static str,
    pub detail: String,
}

pub(crate) struct FunctionInvestigationRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) symbol: &'a str,
    pub(crate) runtime_address: Option<u64>,
    pub(crate) artifact: &'a Path,
    pub(crate) inventory: Option<&'a Path>,
    pub(crate) member: Option<&'a str>,
    pub(crate) origin_member: Option<&'a str>,
    pub(crate) graph_depth: usize,
    pub(crate) include_callers: bool,
    pub(crate) cfg_path: Option<&'a str>,
}

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
    let semantics = semantic_evidence(
        request.source,
        request.symbol,
        &runtime,
        request.graph_depth,
        request.include_callers,
        origin.as_ref(),
        project,
    )?;
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
    Ok(FunctionInvestigationReport {
        schema_version: 8,
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
                    "{} basic blocks, {} entry-reachable",
                    runtime.basic_blocks.len(),
                    reachable_blocks
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
    })
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

fn semantic_evidence(
    source: &str,
    symbol: &str,
    runtime: &artifact::FunctionBody,
    graph_depth: usize,
    include_callers: bool,
    origin: Option<&OriginFunctionEvidence>,
    project: &ProjectSpec,
) -> Result<Vec<SemanticFunctionEvidence>> {
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
        let complete = function.complete;
        let exact = function.exact();
        let pseudo = function.pseudo.clone();
        let mut blockers = function
            .diagnostics()
            .map(|(layer, diagnostic)| BlockerExplanationEvidence {
                root_id: diagnostic.root_id.clone(),
                layer: layer.to_owned(),
                kind: diagnostic.kind.clone(),
                site: diagnostic.site,
                message: diagnostic.rendered.clone(),
                required_model: blocker_requirement(&diagnostic.kind, &diagnostic.rendered)
                    .to_owned(),
                relocation_candidates: diagnostic
                    .site
                    .map(|site| relocation_candidates_at(runtime, origin, site))
                    .unwrap_or_default(),
                confidence: "review-required",
            })
            .collect::<Vec<_>>();
        let calls = function
            .calls
            .iter()
            .map(|call| CallKnowledgeEvidence {
                kind: call.kind.clone(),
                target: call.target.clone(),
                site: call.site,
                knowledge: call.knowledge(),
                semantic_operation: call.semantic_operation.clone(),
                execution_model: call.execution_model_id().map(str::to_owned),
                arguments: call.arguments.clone(),
                argument_shapes: call.argument_shapes(),
                guards: call.guard_expressions(),
            })
            .collect::<Vec<_>>();
        let search_limits = artifacts::GraphSearchLimits {
            max_depth: graph_depth,
            max_visited_nodes: MAX_GRAPH_NODES,
            max_examined_edges: MAX_GRAPH_EDGES,
        };
        let reachability = reader.reachable_from(&function.identity, search_limits);
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
        );
        let graph_limit = graph_slice.limit.or(reachability.limit);
        if let Some(limit) = graph_limit.filter(|limit| *limit != "max-depth") {
            blockers.push(BlockerExplanationEvidence {
                root_id: format!("inspect-graph-limit:{}", function.identity),
                layer: "navigation".to_owned(),
                kind: limit.to_owned(),
                site: None,
                message: format!("focused graph traversal reached {limit}"),
                required_model: "narrow the root/depth or raise the explicit investigation budget"
                    .to_owned(),
                relocation_candidates: Vec::new(),
                confidence: "bounded-navigation",
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
        let linked_ir = serde_json::to_value(&function)?;
        let instruction_evidence = instruction_evidence(runtime, &function, &blockers);
        evidence.push(SemanticFunctionEvidence {
            profile: profile.id.clone(),
            report: profile.output.display().to_string(),
            complete,
            exact,
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
            );
            Ok(Some(EventHandlerAnalysisEvidence {
                identity: function.identity.clone(),
                complete: function.complete,
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

fn blocker_requirement(kind: &str, message: &str) -> &'static str {
    if message.starts_with("call-summary-flattening:") {
        return "close the listed callee cause blockers; the direct call and linked code are already known";
    }
    if message.starts_with("unmodeled-reviewed-external-call") {
        return "add an executable return/output model for this already reviewed external call";
    }
    match kind {
        "memory-load" | "memory-store" => {
            "identify the memory object and add a reviewed global/context type or scenario seed"
        }
        "indirect-control-flow" | "call-shape" => {
            "add a reviewed interface-table layout and a runtime table instance"
        }
        "unresolved-call" => "supply authoritative linked code or an explicit external-call model",
        "call-boundary" => "inspect and close the selected callee's semantic blockers",
        "call-result-model" => "add an executable return/output model for the external call",
        "control-flow" => "add a scenario or reviewed precondition that selects this path",
        "poll-model" => "add a bounded device-read sequence for the polling condition",
        "analysis-budget" => {
            "inspect the reported graph boundary and raise only the relevant budget"
        }
        "memory-intrinsic" => "add a bounded size and source/destination memory-object model",
        _ => "inspect the associated instruction/basic block and add the missing reviewed model",
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
            labels: Vec::new(),
        };

        let path = structural_cfg_path(&runtime, "+0x0:+0x8").unwrap();
        assert_eq!(path.blocks, [0, 1, 2]);
        assert!(path.structurally_reachable);
        assert!(!path.feasibility_claim);
    }
}
