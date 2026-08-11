//! Project-aware, lossless function investigation.
//!
//! This report deliberately combines two different truth domains without
//! conflating them: a complete container/decode/CFG view and the best
//! available semantic linked-IR view.  Semantic incompleteness never removes
//! raw instructions or basic blocks.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use petgraph::{algo::astar, graph::DiGraph};
use serde::Serialize;

use crate::{ProjectSpec, Result, artifact, artifacts};

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
pub struct ReplacementEvidence {
    pub association: &'static str,
    pub report: String,
    pub report_complete_project_run: bool,
    pub report_passed: bool,
    pub freshness_claim: bool,
    pub vendor_source: String,
    pub vendor_symbol: String,
    pub status: String,
    pub reviewed: bool,
    pub disposition: Option<String>,
    pub protocol: Option<String>,
    pub production_component: Option<String>,
    pub verification_probes: Vec<String>,
    pub proofs: serde_json::Value,
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
    pub body: artifact::FunctionBody,
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
    pub event_dispatches: Vec<EventDispatchEvidence>,
    pub reviewed_event_routes: Vec<ReviewedEventRouteEvidence>,
    /// Schema-validated persistent function record. Keeping the complete
    /// record prevents this focused view from silently dropping new semantic
    /// evidence when the linked-IR schema grows.
    pub linked_ir: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewedEventRouteEvidence {
    pub id: String,
    pub mechanism: String,
    pub selector_role: String,
    pub selector_value: u32,
    pub receiver: Option<String>,
    pub execution_context: String,
    pub handler_profile: String,
    pub handler_source: String,
    pub handler: String,
    pub rationale: String,
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
    pub call_targets: Vec<String>,
    pub semantic_operations: Vec<String>,
    pub blocker_ids: Vec<String>,
    pub decode_blocker: Option<String>,
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
    let runtime =
        artifact::inspect_function_body(request.artifact, request.member, request.symbol)?;
    let (origin, origin_ledger) = origin_evidence(&request, project)?;
    let semantics = semantic_evidence(
        request.source,
        request.symbol,
        &runtime,
        request.graph_depth,
        request.include_callers,
        project,
    )?;
    let (reviewed_preconditions, reviewed_paths) =
        reviewed_path_knowledge(request.source, &runtime, project)?;
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
        schema_version: 4,
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

fn replacement_evidence(
    source: &str,
    symbol: &str,
    project: &ProjectSpec,
) -> Result<Vec<ReplacementEvidence>> {
    let Some(workspace) = project.verification.as_ref() else {
        return Ok(Vec::new());
    };
    if !workspace.report.is_file() {
        return Ok(Vec::new());
    }
    let input = std::fs::read_to_string(&workspace.report)?;
    let document: serde_json::Value = serde_json::from_str(&input)?;
    let complete = document
        .get("complete_project_run")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let passed = document
        .get("passed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let candidates = document
        .pointer("/replacement_graph/replacements")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|edge| {
            edge.pointer("/vendor/symbol")
                .and_then(serde_json::Value::as_str)
                == Some(symbol)
        })
        .collect::<Vec<_>>();
    let exact = candidates
        .iter()
        .copied()
        .filter(|edge| {
            edge.pointer("/vendor/source")
                .and_then(serde_json::Value::as_str)
                == Some(source)
        })
        .collect::<Vec<_>>();
    let selected = if !exact.is_empty() {
        exact
    } else if candidates.len() == 1 {
        candidates
    } else {
        Vec::new()
    };
    selected
        .into_iter()
        .map(|edge| {
            let vendor_source = value_string(edge, "/vendor/source")?;
            let association = if vendor_source == source {
                "exact-source-symbol"
            } else {
                "unique-symbol-across-replacement-graph"
            };
            let rust = edge.get("rust");
            Ok(ReplacementEvidence {
                association,
                report: workspace.report.display().to_string(),
                report_complete_project_run: complete,
                report_passed: passed,
                freshness_claim: false,
                vendor_source,
                vendor_symbol: value_string(edge, "/vendor/symbol")?,
                status: value_string(edge, "/status")?,
                reviewed: edge
                    .get("reviewed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                disposition: optional_value_string(edge, "/disposition"),
                protocol: optional_value_string(edge, "/protocol"),
                production_component: rust
                    .and_then(|rust| optional_value_string(rust, "/production_component")),
                verification_probes: rust
                    .and_then(|rust| rust.get("verification_probes"))
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect(),
                proofs: edge
                    .get("proofs")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            })
        })
        .collect()
}

fn value_string(value: &serde_json::Value, pointer: &str) -> Result<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| crate::Error::invalid(format!("replacement report lacks {pointer}")))
}

fn optional_value_string(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn reviewed_path_knowledge(
    source: &str,
    runtime: &artifact::FunctionBody,
    project: &ProjectSpec,
) -> Result<(Vec<ReviewedPreconditionEvidence>, Vec<ReviewedPathEvidence>)> {
    let Some(workspace) = &project.functions else {
        return Ok((Vec::new(), Vec::new()));
    };
    let pack = crate::function_workspace::FunctionPack::load_reviewed(&workspace.pack)?;
    let identity = format!("{source}::{}@{:#010x}", runtime.symbol, runtime.address);
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

fn origin_evidence(
    request: &FunctionInvestigationRequest<'_>,
    project: &ProjectSpec,
) -> Result<(Option<OriginFunctionEvidence>, InvestigationLedgerEntry)> {
    let Some(inventory) = request.inventory else {
        return Ok((
            None,
            InvestigationLedgerEntry {
                layer: "link-origin",
                status: "unavailable",
                detail: "selected source has no source-inventory input".to_owned(),
            },
        ));
    };
    let inventory_report = project
        .symbol_inventory
        .as_ref()
        .map(|spec| spec.output.as_path())
        .filter(|path| path.is_file());
    let association = inventory_report
        .map(artifacts::load_link_unit_origins)
        .transpose()?
        .and_then(|origins| {
            origins.into_iter().find(|origin| {
                origin.symbol == request.symbol
                    && origin
                        .linked_sources
                        .iter()
                        .any(|source| source == request.source)
            })
        });
    let member = request.origin_member.or_else(|| {
        association
            .as_ref()
            .and_then(|origin| origin.origin_member.as_deref())
    });
    if member.is_none() && association.is_none() {
        let candidates = artifact::load_code_symbols(
            inventory,
            request.symbol,
            artifact::CodeSymbolSelection::All,
        )?
        .into_iter()
        .filter(|candidate| candidate.name == request.symbol)
        .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Ok((
                None,
                InvestigationLedgerEntry {
                    layer: "link-origin",
                    status: if candidates.is_empty() {
                        "unavailable"
                    } else {
                        "ambiguous"
                    },
                    detail: if candidates.is_empty() {
                        format!(
                            "raw inventory contains no exact symbol {:?}",
                            request.symbol
                        )
                    } else {
                        format!(
                            "raw inventory contains {} candidates; pass --origin-member or generate a unique link-origin association",
                            candidates.len()
                        )
                    },
                },
            ));
        }
    }
    let body = artifact::inspect_function_body(inventory, member, request.symbol)?;
    let status = if association.is_some() {
        "unique-association"
    } else {
        "unreviewed-selection"
    };
    Ok((
        Some(OriginFunctionEvidence {
            association: status,
            inventory_report: inventory_report.map(|path| path.display().to_string()),
            body,
        }),
        InvestigationLedgerEntry {
            layer: "link-origin",
            status,
            detail: if let Some(association) = association {
                format!(
                    "linked symbol associated by unique name/kind with archive member {}",
                    association
                        .origin_member
                        .as_deref()
                        .unwrap_or("<linked-image>")
                )
            } else {
                "archive body selected without a generated unique-origin association".to_owned()
            },
        },
    ))
}

fn semantic_evidence(
    source: &str,
    symbol: &str,
    runtime: &artifact::FunctionBody,
    graph_depth: usize,
    include_callers: bool,
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
        let Some(function) =
            reader.get_function(source, symbol, runtime.member.as_deref(), runtime.address)?
        else {
            continue;
        };
        let complete = function.complete;
        let exact = function.exact();
        let pseudo = function.pseudo.clone();
        let blockers = function
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
                    .map(|site| relocation_candidates_at(runtime, site))
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
            })
            .collect();
        let reachable_functions = function.effect_summary.reachable_functions.clone();
        let mut call_graph_edges = reader
            .graph_slice(&function.identity, graph_depth, include_callers)
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
            .collect();
        let reviewed_event_routes = function_pack
            .as_ref()
            .into_iter()
            .flat_map(|pack| &pack.event_routes)
            .filter(|route| {
                route.profile == profile.id
                    && route.source == source
                    && route.dispatcher == function.identity
            })
            .map(|route| ReviewedEventRouteEvidence {
                id: route.id.clone(),
                mechanism: route.mechanism.clone(),
                selector_role: route.selector_role.clone(),
                selector_value: route.selector_value,
                receiver: route.receiver.clone(),
                execution_context: route.execution_context.clone(),
                handler_profile: route.handler_profile.clone(),
                handler_source: route.handler_source.clone(),
                handler: route.handler.clone(),
                rationale: route.rationale.clone(),
            })
            .collect();
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
            event_dispatches,
            reviewed_event_routes,
            linked_ir,
        });
    }
    Ok(evidence)
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

fn relocation_candidates_at(runtime: &artifact::FunctionBody, site: u32) -> Vec<String> {
    runtime
        .instructions
        .iter()
        .filter(|instruction| {
            instruction.address == u64::from(site) || instruction.offset == u64::from(site)
        })
        .flat_map(|instruction| instruction.relocations.iter())
        .map(|relocation| relocation.symbol.clone())
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
        calls: BTreeSet<String>,
        operations: BTreeSet<String>,
        blockers: BTreeSet<String>,
        decode: Option<String>,
    }
    let mut sites = BTreeMap::<u64, Evidence>::new();
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
