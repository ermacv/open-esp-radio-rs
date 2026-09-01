//! Query-time RAM definitions which can reach an exact publication anchor.
//!
//! This is deliberately a structural, intra-function slice. It consumes the
//! persisted linked IR and authenticates the corresponding artifact before
//! rebuilding the lossless CFG. Calls on a path to publication are reported as
//! blockers: their RAM effects are never guessed or silently composed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{FunctionBody, FunctionControlFlowKind, ProjectSpec, Result, artifact, artifacts};

use super::{
    EvidenceLevel, FlowBlocker, FlowCfgPathWitness, FlowClaims, FlowInvestigationReport,
    FlowLimits, FlowMemoryDefinitionClassification, FlowMemoryDefinitionEvidence,
    FlowMemorySliceEvidence, FlowPublicationEvidence, FlowStatus, FlowStepEvidence,
    MAX_EXAMINED_EDGES, MAX_LOADED_FUNCTIONS, MAX_VISITED_NODES, PublicationFlowRequest,
    PublicationSelectorRequest,
    target::limit_blocker,
    value::{compose_call_arguments, root_domains},
};

const MAX_LOCAL_INSTRUCTIONS: usize = 16_384;
const MAX_LOCAL_BLOCKS: usize = 4_096;
const MAX_LOCAL_EFFECTS: usize = 65_536;
const MAX_LOCAL_CFG_MEMBERSHIP_WORK: usize = 8_388_608;
const MAX_LOCAL_LOCATIONS: usize = 4_096;
const MAX_LOCAL_DATAFLOW_WORK: usize = 16_777_216;
const MAX_LOCAL_ALIAS_WORK: usize = 4_194_304;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LocationKey {
    object: MemoryObjectIdentity,
    offset: i64,
    width: u8,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MemoryObjectIdentity {
    Argument(u8),
    Global(Option<String>, String),
    Dereferenced(Box<Self>, i64),
    Absolute(String, u32),
    Indexed(Box<Self>, u8, i64),
    Allocation(u32),
    ZeroedAllocation(u32),
    OpaqueExternalObject(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObjectClass {
    Absolute(String, i128),
    Other,
}

#[derive(Clone, Debug)]
struct MemoryDefinition {
    site: u32,
    block: usize,
    object: String,
    values: BTreeSet<String>,
    value_complete: bool,
}

#[derive(Clone, Debug)]
struct LocationDefinitions {
    class: ObjectClass,
    object: String,
    definitions: BTreeMap<u32, MemoryDefinition>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ReachingDefinition {
    Incoming,
    LocalWrite(u32),
    UnknownCall(u32),
}

impl ReachingDefinition {
    const fn site(&self) -> Option<u32> {
        match self {
            Self::Incoming => None,
            Self::LocalWrite(site) | Self::UnknownCall(site) => Some(*site),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CallClobber {
    site: u32,
    block: usize,
}

#[derive(Debug)]
struct ControlFlow {
    entry: usize,
    successors: BTreeMap<usize, BTreeSet<usize>>,
    predecessors: BTreeMap<usize, BTreeSet<usize>>,
    reachable: BTreeSet<usize>,
    instruction_blocks: BTreeMap<u32, usize>,
}

#[derive(Clone, Debug)]
struct PublicationAnchor {
    evidence: FlowPublicationEvidence,
    site: Option<u32>,
}

pub(super) fn investigate(
    request: PublicationFlowRequest<'_>,
    project: &ProjectSpec,
) -> Result<FlowInvestigationReport> {
    let mut reports = Vec::new();
    for profile in project
        .ir_profiles
        .iter()
        .filter(|profile| {
            profile
                .sources
                .iter()
                .any(|source| source == request.source)
        })
        .filter(|profile| profile.output.is_dir())
    {
        let reader = artifacts::LinkedIrReader::open(&profile.output)?;
        let roots = reader.function_identities(request.source, request.root_symbol);
        if roots.len() != 1 {
            continue;
        }
        let root = roots[0].clone();
        reports.push(investigate_profile(
            request.clone(),
            profile,
            &reader,
            root,
        )?);
    }
    reports
        .into_iter()
        .min_by_key(|report| {
            (
                publication_profile_priority(report.status, !report.publications.is_empty()),
                report.blockers.len(),
                std::cmp::Reverse(report.memory_slice.len()),
                report.profile.clone(),
            )
        })
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "no generated linked-IR profile contains exactly one root {}:{}; run `project analyze` after selecting the function",
                request.source, request.root_symbol
            ))
        })
}

const fn publication_profile_priority(status: FlowStatus, publication_reached: bool) -> u8 {
    match status {
        FlowStatus::Complete => 0,
        FlowStatus::Incomplete if publication_reached => 1,
        FlowStatus::Incomplete => 2,
        FlowStatus::NotReached => 3,
    }
}

fn investigate_profile(
    request: PublicationFlowRequest<'_>,
    profile: &crate::project_ir::ProjectIrProfile,
    reader: &artifacts::LinkedIrReader,
    root: String,
) -> Result<FlowInvestigationReport> {
    let root_is_publication = reader
        .get_function_by_identity(&root)?
        .is_some_and(|function| !matching_publications(&function, &request.selector).is_empty());
    let (reachable_identities, visited_nodes, examined_edges, reached_limit) =
        if root_is_publication {
            (BTreeSet::from([root.clone()]), 1, 0, None)
        } else {
            let reachability = reader.reachable_from(
                &root,
                artifacts::GraphSearchLimits {
                    max_depth: request.max_depth,
                    max_visited_nodes: MAX_VISITED_NODES,
                    max_examined_edges: MAX_EXAMINED_EDGES,
                },
            )?;
            (
                reachability.identities,
                reachability.visited_nodes,
                reachability.examined_edges,
                reachability.limit,
            )
        };
    let mut blockers = Vec::new();
    if let Some(limit) = reached_limit {
        blockers.push(limit_blocker(limit));
    }
    let identities = reachable_identities
        .iter()
        .take(MAX_LOADED_FUNCTIONS)
        .cloned()
        .collect::<Vec<_>>();
    if reachable_identities.len() > identities.len() {
        blockers.push(limit_blocker("max-loaded-functions"));
    }

    let mut functions = BTreeMap::new();
    let mut target_functions = BTreeSet::new();
    for identity in identities {
        let Some(function) = reader.get_function_by_identity(&identity)? else {
            blockers.push(FlowBlocker::manual(
                "missing-function-record",
                format!("reachable identity {identity:?} has no indexed function record"),
                "regenerate the linked-IR profile",
            ));
            continue;
        };
        if !matching_publications(&function, &request.selector).is_empty() {
            target_functions.insert(identity.clone());
        }
        functions.insert(identity, function);
    }

    if target_functions.is_empty() {
        return Ok(FlowInvestigationReport {
            schema_version: 5,
            command: "inspect flow",
            mode: "publication-memory",
            status: if blockers.is_empty() {
                FlowStatus::NotReached
            } else {
                FlowStatus::Incomplete
            },
            profile: profile.id.clone(),
            linked_ir: profile.output.display().to_string(),
            root,
            target_kind: Some(request.selector.kind().to_owned()),
            target: Some(request.selector.label()),
            route: None,
            claims: FlowClaims::default(),
            limits: FlowLimits {
                max_depth: request.max_depth,
                visited_nodes,
                examined_edges,
                loaded_functions: functions.len(),
                reached: reached_limit.map(str::to_owned),
                ..FlowLimits::new(request.max_depth)
            },
            steps: Vec::new(),
            effects: Vec::new(),
            publications: Vec::new(),
            memory_slice: Vec::new(),
            rust_boundaries: Vec::new(),
            blockers: {
                blockers.push(FlowBlocker::manual(
                    "publication-not-reached",
                    "no reachable function contains the exact publication selector",
                    "narrow the root, raise --max-depth, or inspect the exact call/MMIO selector",
                ));
                blockers
            },
        });
    }

    let path_search = reader.shortest_path_to_any(
        &root,
        &target_functions,
        artifacts::GraphSearchLimits {
            max_depth: request.max_depth,
            max_visited_nodes: MAX_VISITED_NODES,
            max_examined_edges: MAX_EXAMINED_EDGES,
        },
    )?;
    if let Some(limit) = path_search.limit {
        blockers.push(limit_blocker(limit));
    }
    let Some(path) = path_search.path.as_ref() else {
        blockers.push(FlowBlocker::manual(
            "publication-path-missing",
            "the reachable publication set has no bounded traversable root path",
            "regenerate the linked IR and inspect unresolved call boundaries",
        ));
        return Ok(empty_incomplete_report(
            request.clone(),
            profile,
            root,
            FlowLimits {
                max_depth: request.max_depth,
                visited_nodes,
                examined_edges,
                loaded_functions: functions.len(),
                reached: reached_limit.map(str::to_owned),
                ..FlowLimits::new(request.max_depth)
            },
            blockers,
        ));
    };
    let publication_function = path
        .last()
        .map_or(root.as_str(), |edge| edge.callee.as_str())
        .to_owned();
    if target_functions.len() > 1 {
        blockers.push(FlowBlocker::manual(
            "ambiguous-publication-function",
            format!(
                "the exact selector occurs in {} reachable functions; focused the shortest root path at {publication_function}",
                target_functions.len()
            ),
            "use a narrower root or an exact call/register selector that identifies one publication function",
        ));
    }
    let Some(function) = functions.get(&publication_function) else {
        blockers.push(FlowBlocker::manual(
            "missing-publication-function",
            format!("cannot load focused publication function {publication_function}"),
            "regenerate the linked-IR profile",
        ));
        return Ok(empty_incomplete_report(
            request.clone(),
            profile,
            root,
            FlowLimits {
                max_depth: request.max_depth,
                visited_nodes,
                examined_edges,
                loaded_functions: functions.len(),
                reached: reached_limit.map(str::to_owned),
                ..FlowLimits::new(request.max_depth)
            },
            blockers,
        ));
    };

    let mut anchors = matching_publications(function, &request.selector);
    if anchors.len() > 1 {
        blockers.push(FlowBlocker::manual(
            "ambiguous-publication-site",
            format!(
                "the exact selector occurs at {} sites in {publication_function}",
                anchors.len()
            ),
            "select a publication function/root for which the exact selector has one instruction site",
        ));
    }
    for anchor in anchors
        .iter()
        .filter(|anchor| !anchor.evidence.selector_exact)
    {
        blockers.push(FlowBlocker::manual(
            "conditional-publication-candidate",
            format!(
                "{} at {} is a conditional publication candidate (modes={}), not an exact selected publication",
                anchor.evidence.target,
                anchor
                    .site
                    .map(|site| format!("{site:#010x}"))
                    .unwrap_or_else(|| "an unresolved site".to_owned()),
                if anchor.evidence.modes.is_empty() {
                    "unknown".to_owned()
                } else {
                    anchor.evidence.modes.join("|")
                }
            ),
            "inspect the selector guard/index domain and bind the concrete candidate before claiming the publication",
        ));
    }
    let steps = compose_steps(
        path,
        &functions,
        &profile.output.display().to_string(),
        &mut blockers,
    );
    if !path.is_empty() {
        blockers.push(FlowBlocker::manual(
            "interprocedural-publication-slice",
            format!(
                "the focused publication is {} call edges below root {root}; caller RAM definitions are not composed into the local slice",
                path.len()
            ),
            "inspect each descriptor-building caller separately or select the publication function as the root",
        ));
    }

    let authenticated =
        reader.authenticated_source_artifact(&function.source, &function.artifact_sha256)?;
    let body = artifact::inspect_function_body_at_data(
        &authenticated.path,
        &authenticated.bytes,
        function.member.as_deref(),
        &function.symbol,
        function.address.map(u64::from),
    )?;
    let cfg = match local_cfg_budget_error(&body, function.instruction_effects.len()) {
        Some(message) => {
            blockers.push(FlowBlocker::manual(
                "operation-slice-local-limit",
                format!("{publication_function}: {message}"),
                "narrow the publication function or raise the typed local slice limits after reviewing its resource cost",
            ));
            None
        }
        None => match build_control_flow(&body) {
            Ok(cfg) => Some(cfg),
            Err(message) => {
                blockers.push(FlowBlocker::manual(
                    "malformed-publication-cfg",
                    format!("{publication_function}: {message}"),
                    "inspect the lossless function body and resolve its CFG/decode blockers",
                ));
                None
            }
        },
    };
    let decoded_body_complete = body.addresses_resolved
        && body.accounted_bytes == body.size
        && body
            .instructions
            .iter()
            .all(|instruction| instruction.supported && u32::try_from(instruction.address).is_ok());
    if !decoded_body_complete {
        blockers.push(FlowBlocker::manual(
            "incomplete-authenticated-publication-body",
            format!(
                "{publication_function} has unresolved addresses, bytes, or instructions in its authenticated body"
            ),
            "resolve the lossless decode/address blockers before treating the RAM slice as complete",
        ));
    }
    let local_effects_complete = function.completeness.body_complete;
    if !local_effects_complete {
        blockers.push(FlowBlocker::manual(
            "incomplete-persisted-local-effects",
            format!(
                "{publication_function} has incomplete persisted local effect recovery even though its authenticated instruction body may be complete"
            ),
            "inspect the linked-IR diagnostics and resolve the symbolic branch/effect budget before treating the RAM slice as complete",
        ));
    }
    let mut memory_slice = Vec::new();
    if let Some(cfg) = &cfg {
        let (locations, collection_blockers) = collect_definitions(function, cfg);
        let collection_complete = collection_blockers.is_empty();
        blockers.extend(collection_blockers);
        let one_anchor = anchors.len() == 1;
        for anchor in &mut anchors {
            let Some(site) = anchor.site else {
                blockers.push(FlowBlocker::manual(
                    "publication-site-missing",
                    format!(
                        "publication anchor {} has no instruction site",
                        anchor.evidence.target
                    ),
                    "regenerate the linked IR with instruction-site effects",
                ));
                continue;
            };
            let Some(instruction) = instruction_at_site(&body, site) else {
                blockers.push(FlowBlocker::manual(
                    "publication-site-not-an-instruction",
                    format!(
                        "publication site {site:#010x} is not an authenticated instruction start in {publication_function}"
                    ),
                    "regenerate the linked IR from the authenticated artifact",
                ));
                continue;
            };
            let Some(sink_block) = cfg.instruction_blocks.get(&site).copied() else {
                blockers.push(FlowBlocker::manual(
                    "publication-site-outside-cfg",
                    format!(
                        "publication site {site:#010x} has no authenticated CFG block in {publication_function}"
                    ),
                    "regenerate the linked IR from the authenticated artifact",
                ));
                continue;
            };
            if !cfg.reachable.contains(&sink_block) {
                blockers.push(FlowBlocker::manual(
                    "publication-site-unreachable",
                    format!(
                        "publication site {site:#010x} is an instruction start in unreachable CFG block {sink_block}"
                    ),
                    "select a reachable publication or resolve the persisted/live CFG disagreement",
                ));
                continue;
            }
            if matches!(
                request.selector,
                PublicationSelectorRequest::Operation(_) | PublicationSelectorRequest::Call(_)
            ) && !matches!(
                (
                    anchor.evidence.tails.as_slice(),
                    instruction.control_flow.kind
                ),
                (
                    [false],
                    FunctionControlFlowKind::Call | FunctionControlFlowKind::IndirectCall
                ) | (
                    [true],
                    FunctionControlFlowKind::Jump | FunctionControlFlowKind::IndirectJump
                )
            ) {
                blockers.push(FlowBlocker::manual(
                    "publication-call-kind-mismatch",
                    format!(
                        "persisted call publication {site:#010x} is authenticated as {}",
                        instruction.control_flow.kind.label()
                    ),
                    "regenerate linked call facts from the authenticated instruction body",
                ));
                continue;
            }
            if matches!(
                request.selector,
                PublicationSelectorRequest::Register(_) | PublicationSelectorRequest::Address(_)
            ) && (!anchor.evidence.persisted_block_complete
                || anchor.evidence.persisted_blocks.as_slice() != [sink_block])
            {
                blockers.push(FlowBlocker::manual(
                    "publication-effect-block-mismatch",
                    format!(
                        "MMIO publication {site:#010x} has persisted blocks {:?}, authenticated CFG block {sink_block}",
                        anchor.evidence.persisted_blocks
                    ),
                    "regenerate linked instruction effects from the authenticated artifact",
                ));
                continue;
            }
            anchor.evidence.site_authenticated = true;
            if !one_anchor {
                continue;
            }
            let call_clobbers =
                call_clobbers_before_publication(&body, cfg, site, &request.selector);
            if !call_clobbers.is_empty() {
                blockers.push(FlowBlocker::manual(
                    "interprocedural-memory-boundary",
                    format!(
                        "{publication_function} has {} unsliced call instruction(s) on structural paths to publication {site:#010x}",
                        call_clobbers.len()
                    ),
                    "inspect the callees or add reviewed no-memory/no-alias contracts before claiming a complete publication memory inventory",
                ));
            }
            let unresolved_control_flow = body.instructions.iter().any(|instruction| {
                let selected_indirect_tail = instruction.address == u64::from(site)
                    && matches!(
                        request.selector,
                        PublicationSelectorRequest::Operation(_)
                            | PublicationSelectorRequest::Call(_)
                    )
                    && anchor.evidence.tails.as_slice() == [true]
                    && instruction.control_flow.kind == FunctionControlFlowKind::IndirectJump;
                matches!(
                    instruction.control_flow.kind,
                    FunctionControlFlowKind::IndirectJump | FunctionControlFlowKind::Unknown
                ) && !selected_indirect_tail
                    && u32::try_from(instruction.address)
                        .ok()
                        .and_then(|site| cfg.instruction_blocks.get(&site))
                        .is_some_and(|block| cfg.reachable.contains(block))
            });
            if unresolved_control_flow {
                blockers.push(FlowBlocker::manual(
                    "unresolved-publication-control-flow",
                    format!(
                        "{publication_function} has unresolved control flow on a structural path before publication {site:#010x}"
                    ),
                    "resolve the indirect/unknown predecessor before promoting the local definition set",
                ));
            }
            memory_slice.extend(slice_at_publication(
                &publication_function,
                site,
                sink_block,
                cfg,
                &locations,
                &call_clobbers,
                decoded_body_complete
                    && local_effects_complete
                    && collection_complete
                    && !unresolved_control_flow,
                anchor.evidence.selector_exact && anchor.evidence.site_authenticated,
                &mut blockers,
            ));
        }
    }
    let publications = anchors
        .iter()
        .map(|anchor| anchor.evidence.clone())
        .collect::<Vec<_>>();
    memory_slice.sort_by(|left, right| {
        (
            &left.publication_function,
            left.publication_site,
            &left.object,
            left.offset,
            left.width,
        )
            .cmp(&(
                &right.publication_function,
                right.publication_site,
                &right.object,
                right.offset,
                right.width,
            ))
    });
    blockers.sort_by(|left, right| (&left.kind, &left.message).cmp(&(&right.kind, &right.message)));
    blockers.dedup_by(|left, right| left.kind == right.kind && left.message == right.message);

    Ok(FlowInvestigationReport {
        schema_version: 5,
        command: "inspect flow",
        mode: "publication-memory",
        status: if blockers.is_empty() {
            FlowStatus::Complete
        } else {
            FlowStatus::Incomplete
        },
        profile: profile.id.clone(),
        linked_ir: profile.output.display().to_string(),
        root,
        target_kind: Some(request.selector.kind().to_owned()),
        target: Some(request.selector.label()),
        route: None,
        claims: FlowClaims {
            structural_navigation: true,
            ..FlowClaims::default()
        },
        limits: FlowLimits {
            max_depth: request.max_depth,
            visited_nodes: visited_nodes.max(path_search.visited_nodes),
            examined_edges: examined_edges.max(path_search.examined_edges),
            loaded_functions: functions.len(),
            reached: reached_limit.or(path_search.limit).map(str::to_owned),
            ..FlowLimits::new(request.max_depth)
        },
        steps,
        effects: Vec::new(),
        publications,
        memory_slice,
        rust_boundaries: Vec::new(),
        blockers,
    })
}

fn empty_incomplete_report(
    request: PublicationFlowRequest<'_>,
    profile: &crate::project_ir::ProjectIrProfile,
    root: String,
    limits: FlowLimits,
    blockers: Vec<FlowBlocker>,
) -> FlowInvestigationReport {
    FlowInvestigationReport {
        schema_version: 5,
        command: "inspect flow",
        mode: "publication-memory",
        status: FlowStatus::Incomplete,
        profile: profile.id.clone(),
        linked_ir: profile.output.display().to_string(),
        root,
        target_kind: Some(request.selector.kind().to_owned()),
        target: Some(request.selector.label()),
        route: None,
        claims: FlowClaims::default(),
        limits,
        steps: Vec::new(),
        effects: Vec::new(),
        publications: Vec::new(),
        memory_slice: Vec::new(),
        rust_boundaries: Vec::new(),
        blockers,
    }
}

fn matching_publications(
    function: &artifacts::StoredFunction,
    selector: &PublicationSelectorRequest<'_>,
) -> Vec<PublicationAnchor> {
    let mut anchors = Vec::new();
    match selector {
        PublicationSelectorRequest::Operation(operation) => {
            anchors.extend(
                function
                    .calls
                    .iter()
                    .filter(|call| call.semantic_operation.as_deref() == Some(*operation))
                    .map(|call| PublicationAnchor {
                        site: call.site,
                        evidence: FlowPublicationEvidence {
                            // The site is observed, but the operation name is
                            // reviewed semantic knowledge and must never be
                            // promoted to an observed hardware fact.
                            evidence: EvidenceLevel::Reviewed,
                            function: function.identity.clone(),
                            site: call.site,
                            kind: "operation".to_owned(),
                            target: (*operation).to_owned(),
                            selector_exact: call.publication_selector_exact(),
                            site_authenticated: false,
                            persisted_blocks: Vec::new(),
                            persisted_block_complete: true,
                            tails: vec![call.tail()],
                            modes: vec!["reviewed-operation".to_owned(), call.kind.clone()],
                            paths: Vec::new(),
                            guards: call.guard_expressions(),
                            operation: call.semantic_operation.clone(),
                            address: None,
                            registers: Vec::new(),
                        },
                    }),
            );
        }
        PublicationSelectorRequest::Call(target) => {
            anchors.extend(
                function
                    .calls
                    .iter()
                    .filter(|call| call.target == *target)
                    .map(|call| PublicationAnchor {
                        site: call.site,
                        evidence: FlowPublicationEvidence {
                            evidence: EvidenceLevel::Observed,
                            function: function.identity.clone(),
                            site: call.site,
                            kind: "call".to_owned(),
                            target: (*target).to_owned(),
                            selector_exact: call.publication_selector_exact(),
                            site_authenticated: false,
                            persisted_blocks: Vec::new(),
                            persisted_block_complete: true,
                            tails: vec![call.tail()],
                            modes: vec![call.kind.clone()],
                            paths: Vec::new(),
                            guards: call.guard_expressions(),
                            operation: call.semantic_operation.clone(),
                            address: None,
                            registers: Vec::new(),
                        },
                    }),
            );
        }
        PublicationSelectorRequest::Register(expected) => {
            anchors.extend(function.instruction_effects.iter().filter_map(|effect| {
                let artifacts::StoredInstructionEffect::Mmio {
                    site,
                    access,
                    address,
                    register,
                    mode,
                    block,
                    paths,
                    guards,
                    ..
                } = effect
                else {
                    return None;
                };
                (access == "write" && register == *expected).then(|| PublicationAnchor {
                    site: Some(*site),
                    evidence: FlowPublicationEvidence {
                        evidence: EvidenceLevel::Observed,
                        function: function.identity.clone(),
                        site: Some(*site),
                        kind: "register".to_owned(),
                        target: (*expected).to_owned(),
                        selector_exact: mode == "static",
                        site_authenticated: false,
                        persisted_blocks: block.iter().copied().collect(),
                        persisted_block_complete: block.is_some(),
                        tails: Vec::new(),
                        modes: vec![mode.clone()],
                        paths: paths.clone(),
                        guards: guards.clone(),
                        operation: None,
                        address: Some(*address),
                        registers: vec![register.to_owned()],
                    },
                })
            }));
        }
        PublicationSelectorRequest::Address(expected) => {
            anchors.extend(function.instruction_effects.iter().filter_map(|effect| {
                let artifacts::StoredInstructionEffect::Mmio {
                    site,
                    access,
                    address,
                    register,
                    mode,
                    block,
                    paths,
                    guards,
                    ..
                } = effect
                else {
                    return None;
                };
                (access == "write" && address == expected).then(|| PublicationAnchor {
                    site: Some(*site),
                    evidence: FlowPublicationEvidence {
                        evidence: EvidenceLevel::Observed,
                        function: function.identity.clone(),
                        site: Some(*site),
                        kind: "address".to_owned(),
                        target: format!("{expected:#010x}"),
                        selector_exact: mode == "static",
                        site_authenticated: false,
                        persisted_blocks: block.iter().copied().collect(),
                        persisted_block_complete: block.is_some(),
                        tails: Vec::new(),
                        modes: vec![mode.clone()],
                        paths: paths.clone(),
                        guards: guards.clone(),
                        operation: None,
                        address: Some(*address),
                        registers: vec![register.to_owned()],
                    },
                })
            }));
        }
    }
    anchors.sort_by(|left, right| {
        (left.site, &left.evidence.kind, &left.evidence.target).cmp(&(
            right.site,
            &right.evidence.kind,
            &right.evidence.target,
        ))
    });
    merge_publication_anchors(anchors)
}

fn merge_publication_anchors(anchors: Vec<PublicationAnchor>) -> Vec<PublicationAnchor> {
    let mut merged = Vec::<PublicationAnchor>::new();
    for anchor in anchors {
        if let Some(previous) = merged.last_mut().filter(|previous| {
            previous.site == anchor.site
                && previous.evidence.kind == anchor.evidence.kind
                && previous.evidence.target == anchor.evidence.target
        }) {
            previous.evidence.selector_exact &= anchor.evidence.selector_exact;
            previous.evidence.persisted_block_complete &= anchor.evidence.persisted_block_complete;
            previous.evidence.tails.extend(anchor.evidence.tails);
            previous
                .evidence
                .persisted_blocks
                .extend(anchor.evidence.persisted_blocks);
            previous.evidence.modes.extend(anchor.evidence.modes);
            previous.evidence.paths.extend(anchor.evidence.paths);
            previous.evidence.guards.extend(anchor.evidence.guards);
            previous
                .evidence
                .registers
                .extend(anchor.evidence.registers);
            previous.evidence.modes.sort();
            previous.evidence.modes.dedup();
            previous.evidence.paths.sort();
            previous.evidence.paths.dedup();
            previous.evidence.guards.sort();
            previous.evidence.guards.dedup();
            previous.evidence.registers.sort();
            previous.evidence.registers.dedup();
            previous.evidence.persisted_blocks.sort_unstable();
            previous.evidence.persisted_blocks.dedup();
            previous.evidence.tails.sort_unstable();
            previous.evidence.tails.dedup();
        } else {
            merged.push(anchor);
        }
    }
    merged
}

fn compose_steps(
    path: &[artifacts::StoredGraphEdge],
    functions: &BTreeMap<String, artifacts::StoredFunction>,
    origin: &str,
    blockers: &mut Vec<FlowBlocker>,
) -> Vec<FlowStepEvidence> {
    let mut domains = root_domains();
    let mut steps = Vec::new();
    for (ordinal, edge) in path.iter().enumerate() {
        let Some(caller) = functions.get(&edge.caller) else {
            blockers.push(FlowBlocker::manual(
                "missing-caller-record",
                format!("cannot load caller {}", edge.caller),
                "regenerate the linked-IR profile",
            ));
            continue;
        };
        let Some(call) = caller.calls.iter().find(|call| {
            call.site == edge.site && call.target == edge.callee && call.kind == edge.kind
        }) else {
            blockers.push(FlowBlocker::manual(
                "missing-call-fact",
                format!(
                    "graph edge {} -> {} has no exact call fact",
                    edge.caller, edge.callee
                ),
                "regenerate the linked IR and inspect the caller body",
            ));
            continue;
        };
        let (next, arguments) = compose_call_arguments(caller, call, &domains);
        domains = next;
        steps.push(FlowStepEvidence {
            ordinal,
            evidence: EvidenceLevel::Observed,
            context_evidence: EvidenceLevel::Observed,
            context: "synchronous".to_owned(),
            caller: edge.caller.clone(),
            callee: edge.callee.clone(),
            site: edge.site,
            kind: edge.kind.clone(),
            tail: call.tail(),
            argument_shapes: call.argument_shapes(),
            arguments,
            guards: call.guard_expressions(),
            origin: origin.to_owned(),
        });
    }
    steps
}

fn local_cfg_budget_error(body: &FunctionBody, effect_count: usize) -> Option<String> {
    let membership_work = body.instructions.len().checked_mul(body.basic_blocks.len());
    let call_count = body
        .instructions
        .iter()
        .filter(|instruction| {
            matches!(
                instruction.control_flow.kind,
                FunctionControlFlowKind::Call | FunctionControlFlowKind::IndirectCall
            )
        })
        .count();
    let graph_size = body.basic_blocks.len().checked_add(
        body.basic_blocks
            .iter()
            .map(|block| block.successors.len())
            .sum::<usize>(),
    );
    let call_path_work = graph_size.and_then(|size| call_count.checked_mul(size));
    if body.instructions.len() > MAX_LOCAL_INSTRUCTIONS
        || body.basic_blocks.len() > MAX_LOCAL_BLOCKS
        || effect_count > MAX_LOCAL_EFFECTS
        || membership_work.is_none_or(|work| work > MAX_LOCAL_CFG_MEMBERSHIP_WORK)
        || call_path_work.is_none_or(|work| work > MAX_LOCAL_DATAFLOW_WORK)
    {
        Some(format!(
            "local CFG/effect budget exceeded (instructions={}, blocks={}, effects={}, membership-work={}, call-path-work={})",
            body.instructions.len(),
            body.basic_blocks.len(),
            effect_count,
            membership_work
                .map(|work| work.to_string())
                .unwrap_or_else(|| "overflow".to_owned()),
            display_work(call_path_work),
        ))
    } else {
        None
    }
}

fn build_control_flow(body: &FunctionBody) -> std::result::Result<ControlFlow, String> {
    if !body.addresses_resolved || body.accounted_bytes != body.size {
        return Err("body bytes or runtime addresses are incomplete".to_owned());
    }
    if body
        .instructions
        .windows(2)
        .any(|pair| pair[0].address >= pair[1].address)
    {
        return Err("instruction sites are not strictly ordered".to_owned());
    }
    let blocks = body
        .basic_blocks
        .iter()
        .map(|block| (block.id, block))
        .collect::<BTreeMap<_, _>>();
    if blocks.len() != body.basic_blocks.len() || blocks.is_empty() {
        return Err("basic-block identifiers are empty or duplicated".to_owned());
    }
    for block in blocks.values() {
        if block.start_offset >= block.end_offset || block.end_offset > body.size as u64 {
            return Err(format!(
                "basic block {} has an invalid byte range",
                block.id
            ));
        }
    }
    let mut instruction_blocks = BTreeMap::new();
    for instruction in &body.instructions {
        let matching_blocks = body
            .basic_blocks
            .iter()
            .filter(|block| {
                let offset = instruction.address.saturating_sub(body.address);
                offset >= block.start_offset && offset < block.end_offset
            })
            .map(|block| block.id)
            .collect::<Vec<_>>();
        if matching_blocks.len() != 1 {
            return Err(format!(
                "instruction {:#010x} belongs to {} basic blocks",
                instruction.address,
                matching_blocks.len()
            ));
        }
        let site = u32::try_from(instruction.address)
            .map_err(|_| format!("instruction {:#x} is outside RV32", instruction.address))?;
        if instruction_blocks
            .insert(site, matching_blocks[0])
            .is_some()
        {
            return Err(format!("instruction site {site:#010x} is duplicated"));
        }
    }
    let entry = block_for_address(body, body.address)
        .ok_or_else(|| "function entry has no basic block".to_owned())?;
    let mut successors = BTreeMap::<usize, BTreeSet<usize>>::new();
    let mut predecessors = BTreeMap::<usize, BTreeSet<usize>>::new();
    for block in blocks.values() {
        successors.entry(block.id).or_default();
        predecessors.entry(block.id).or_default();
        for successor in &block.successors {
            let Some(target) = successor.block else {
                let body_end = body.address.saturating_add(body.size as u64);
                if successor
                    .target
                    .is_some_and(|target| target < body.address || target >= body_end)
                {
                    // A resolved direct tail jump leaves this function and
                    // cannot reach a later local publication site.  It is a
                    // terminal edge for the intra-function CFG, not an
                    // unresolved successor.
                    continue;
                }
                return Err(format!(
                    "basic block {} has an unresolved successor",
                    block.id
                ));
            };
            if !blocks.contains_key(&target) {
                return Err(format!(
                    "basic block {} references missing successor {target}",
                    block.id
                ));
            }
            successors.entry(block.id).or_default().insert(target);
            predecessors.entry(target).or_default().insert(block.id);
        }
    }
    let mut reachable = BTreeSet::from([entry]);
    let mut pending = VecDeque::from([entry]);
    while let Some(block) = pending.pop_front() {
        for successor in successors.get(&block).into_iter().flatten() {
            if reachable.insert(*successor) {
                pending.push_back(*successor);
            }
        }
    }
    if blocks
        .values()
        .any(|block| block.reachable != reachable.contains(&block.id))
    {
        return Err("persisted basic-block reachability disagrees with CFG successors".to_owned());
    }
    Ok(ControlFlow {
        entry,
        successors,
        predecessors,
        reachable,
        instruction_blocks,
    })
}

fn collect_definitions(
    function: &artifacts::StoredFunction,
    cfg: &ControlFlow,
) -> (BTreeMap<LocationKey, LocationDefinitions>, Vec<FlowBlocker>) {
    let mut locations = BTreeMap::<LocationKey, LocationDefinitions>::new();
    let mut blockers = Vec::new();
    for effect in &function.instruction_effects {
        let artifacts::StoredInstructionEffect::Memory {
            site,
            block,
            access,
            width,
            object,
            offset,
            value,
            value_pseudo,
            ..
        } = effect
        else {
            continue;
        };
        if memory_effect_shape_error(access, *width) == Some("access") {
            blockers.push(FlowBlocker::manual(
                "invalid-memory-effect-access",
                format!(
                    "{} memory effect {site:#010x} has unsupported access {access:?}",
                    function.identity
                ),
                "regenerate linked IR with the canonical read/write access vocabulary",
            ));
            continue;
        }
        if memory_effect_shape_error(access, *width) == Some("width") {
            blockers.push(FlowBlocker::manual(
                "invalid-memory-effect-width",
                format!(
                    "{} memory effect {site:#010x} has unsupported width {width}",
                    function.identity
                ),
                "regenerate linked IR with an authenticated RV32 byte, half-word, or word transfer width",
            ));
            continue;
        }
        if access != "write" {
            continue;
        }
        let Some(actual_block) = cfg.instruction_blocks.get(site).copied() else {
            blockers.push(FlowBlocker::manual(
                "memory-effect-site-not-an-instruction",
                format!(
                    "{} memory write {site:#010x} is not an authenticated instruction start",
                    function.identity
                ),
                "regenerate the linked IR from the authenticated artifact",
            ));
            continue;
        };
        if block.is_none_or(|persisted| persisted != actual_block) {
            blockers.push(FlowBlocker::manual(
                "memory-effect-block-mismatch",
                format!(
                    "{} memory write {site:#010x} has persisted block {block:?}, authenticated CFG block {actual_block}",
                    function.identity
                ),
                "regenerate the linked IR before using instruction-site dataflow",
            ));
            continue;
        }
        if !cfg.reachable.contains(&actual_block) {
            continue;
        }
        let key = LocationKey {
            object: memory_object_identity(object),
            offset: *offset,
            width: *width,
        };
        let object_name = display_object(object);
        let recovered_value = value_pseudo.as_ref().or(value.as_ref());
        let definition = MemoryDefinition {
            site: *site,
            block: actual_block,
            object: object_name.clone(),
            values: recovered_value.into_iter().cloned().collect(),
            value_complete: recovered_value.is_some(),
        };
        insert_definition(
            &mut locations,
            key,
            classify_object(object),
            object_name,
            definition,
            &function.identity,
            &mut blockers,
        );
    }
    (locations, blockers)
}

fn memory_effect_shape_error(access: &str, width: u8) -> Option<&'static str> {
    if !matches!(access, "read" | "write") {
        Some("access")
    } else if !matches!(width, 8 | 16 | 32) {
        Some("width")
    } else {
        None
    }
}

fn insert_definition(
    locations: &mut BTreeMap<LocationKey, LocationDefinitions>,
    key: LocationKey,
    class: ObjectClass,
    object: String,
    definition: MemoryDefinition,
    function: &str,
    blockers: &mut Vec<FlowBlocker>,
) {
    let location = locations.entry(key).or_insert_with(|| LocationDefinitions {
        class,
        object,
        definitions: BTreeMap::new(),
    });
    if let Some(previous) = location.definitions.get_mut(&definition.site) {
        if previous.block != definition.block || previous.object != definition.object {
            blockers.push(FlowBlocker::manual(
                "conflicting-memory-definition",
                format!(
                    "{function} has conflicting persisted memory effects at {:#010x}",
                    definition.site
                ),
                "regenerate the linked IR and inspect the instruction effect",
            ));
        } else {
            previous.values.extend(definition.values);
            previous.value_complete &= definition.value_complete;
        }
        return;
    }
    location.definitions.insert(definition.site, definition);
}

#[allow(clippy::too_many_arguments)]
fn slice_at_publication(
    function: &str,
    publication_site: u32,
    publication_block: usize,
    cfg: &ControlFlow,
    locations: &BTreeMap<LocationKey, LocationDefinitions>,
    call_clobbers: &[CallClobber],
    definition_set_complete: bool,
    publication_exact: bool,
    blockers: &mut Vec<FlowBlocker>,
) -> Vec<FlowMemorySliceEvidence> {
    if let Some(message) = local_slice_budget_error(cfg, locations, call_clobbers.len()) {
        blockers.push(FlowBlocker::manual(
            "operation-slice-local-limit",
            format!("{function}: {message}"),
            "narrow the publication function or raise the typed local slice limits after reviewing its resource cost",
        ));
        return Vec::new();
    }
    let mut reaching_by_location = BTreeMap::<LocationKey, BTreeSet<ReachingDefinition>>::new();
    for (key, location) in locations {
        let reaching = reaching_definitions(
            cfg,
            location,
            call_clobbers,
            publication_block,
            publication_site,
        );
        if reaching
            .iter()
            .any(|definition| matches!(definition, ReachingDefinition::LocalWrite(_)))
        {
            reaching_by_location.insert(key.clone(), reaching);
        }
    }

    let mut output = Vec::new();
    for (key, reaching) in &reaching_by_location {
        let location = &locations[key];
        let partial_overlap = reaching_by_location.keys().any(|other| {
            other != key
                && other.object == key.object
                && byte_ranges_overlap(key.offset, key.width, other.offset, other.width)
        });
        if partial_overlap {
            blockers.push(FlowBlocker::manual(
                "partially-overlapping-memory-definitions",
                format!(
                    "{function} has partially overlapping writes for {} {:+#x}/{} before publication {publication_site:#010x}",
                    location.object, key.offset, key.width
                ),
                "model the byte-lane composition before promoting either write as a last definition",
            ));
        }
        let object_alias_complete = !partial_overlap
            && reaching_by_location.keys().all(|other| {
                other == key
                    || definitely_disjoint(key, &location.class, other, &locations[other].class)
            });
        if !object_alias_complete {
            blockers.push(FlowBlocker::manual(
                "memory-alias-boundary",
                format!(
                    "{function} cannot prove a complete alias set for {} {:+#x}/{} before publication {publication_site:#010x}",
                    location.object, key.offset, key.width
                ),
                "narrow the publication function or add independently reviewed pointer/object identity evidence",
            ));
        }
        let reaching_call = reaching.iter().find_map(|definition| match definition {
            ReachingDefinition::UnknownCall(site) => Some(*site),
            ReachingDefinition::Incoming | ReachingDefinition::LocalWrite(_) => None,
        });
        if let Some(call_site) = reaching_call {
            blockers.push(FlowBlocker::manual(
                "interprocedural-memory-clobber",
                format!(
                    "{function} call {call_site:#010x} may be the last write to {} {:+#x}/{} before publication {publication_site:#010x}",
                    location.object, key.offset, key.width
                ),
                "inspect the callee memory effects or establish a reviewed no-alias contract for this object",
            ));
        }
        let alias_complete = object_alias_complete && reaching_call.is_none();
        let incoming_definition_possible = reaching.contains(&ReachingDefinition::Incoming);
        let sites = reaching
            .iter()
            .filter_map(|definition| match definition {
                ReachingDefinition::LocalWrite(site) => Some(*site),
                ReachingDefinition::Incoming | ReachingDefinition::UnknownCall(_) => None,
            })
            .collect::<Vec<_>>();
        let claims_closed = alias_complete && definition_set_complete && publication_exact;
        let must = sites.len() == 1 && !incoming_definition_possible && claims_closed;
        let mut definitions = Vec::new();
        for site in sites {
            let definition = &location.definitions[&site];
            let witness = shortest_definition_witness(
                cfg,
                location,
                definition,
                publication_block,
                publication_site,
            );
            if witness.is_none() {
                blockers.push(FlowBlocker::manual(
                    "memory-definition-witness-missing",
                    format!(
                        "reaching definition {site:#010x} has no shortest CFG witness to publication {publication_site:#010x}"
                    ),
                    "inspect the authenticated CFG and regenerate linked instruction effects",
                ));
            }
            definitions.push(FlowMemoryDefinitionEvidence {
                evidence: EvidenceLevel::Observed,
                function: function.to_owned(),
                site,
                object: definition.object.clone(),
                offset: key.offset,
                width: key.width,
                values: definition.values.iter().cloned().collect(),
                value_complete: definition.value_complete,
                classification: if must && witness.is_some() {
                    FlowMemoryDefinitionClassification::Must
                } else if claims_closed && witness.is_some() {
                    FlowMemoryDefinitionClassification::Alternative
                } else {
                    FlowMemoryDefinitionClassification::Candidate
                },
                witness,
            });
        }
        output.push(FlowMemorySliceEvidence {
            publication_function: function.to_owned(),
            publication_site,
            object: location.object.clone(),
            offset: key.offset,
            width: key.width,
            alias_complete,
            definition_set_complete,
            publication_exact,
            incoming_definition_possible,
            definitions,
        });
    }
    output
}

fn local_slice_budget_error(
    cfg: &ControlFlow,
    locations: &BTreeMap<LocationKey, LocationDefinitions>,
    call_count: usize,
) -> Option<String> {
    let location_count = locations.len();
    let definition_count = locations
        .values()
        .map(|location| location.definitions.len())
        .sum::<usize>();
    let edge_count = cfg
        .successors
        .values()
        .map(BTreeSet::len)
        .sum::<usize>()
        .max(1);
    let state_count = definition_count
        .checked_add(location_count)
        .and_then(|count| {
            call_count
                .checked_mul(location_count)
                .and_then(|calls| count.checked_add(calls))
        });
    let dataflow_work = state_count.and_then(|count| edge_count.checked_mul(count));
    let witness_work = cfg.reachable.len().checked_mul(definition_count);
    let alias_work = location_count.checked_mul(location_count);
    if location_count > MAX_LOCAL_LOCATIONS
        || dataflow_work.is_none_or(|work| work > MAX_LOCAL_DATAFLOW_WORK)
        || witness_work.is_none_or(|work| work > MAX_LOCAL_DATAFLOW_WORK)
        || alias_work.is_none_or(|work| work > MAX_LOCAL_ALIAS_WORK)
    {
        Some(format!(
            "local dataflow budget exceeded (locations={location_count}, definitions={definition_count}, calls={call_count}, dataflow-work={}, witness-work={}, alias-work={})",
            display_work(dataflow_work),
            display_work(witness_work),
            display_work(alias_work),
        ))
    } else {
        None
    }
}

fn display_work(work: Option<usize>) -> String {
    work.map(|work| work.to_string())
        .unwrap_or_else(|| "overflow".to_owned())
}

fn reaching_definitions(
    cfg: &ControlFlow,
    location: &LocationDefinitions,
    call_clobbers: &[CallClobber],
    publication_block: usize,
    publication_site: u32,
) -> BTreeSet<ReachingDefinition> {
    let mut by_block = BTreeMap::<usize, Vec<ReachingDefinition>>::new();
    for definition in location.definitions.values() {
        by_block
            .entry(definition.block)
            .or_default()
            .push(ReachingDefinition::LocalWrite(definition.site));
    }
    for clobber in call_clobbers {
        by_block
            .entry(clobber.block)
            .or_default()
            .push(ReachingDefinition::UnknownCall(clobber.site));
    }
    for events in by_block.values_mut() {
        events.sort_by_key(ReachingDefinition::site);
    }
    let mut incoming = cfg
        .reachable
        .iter()
        .map(|block| (*block, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = incoming.clone();
    let mut pending = VecDeque::from([cfg.entry]);
    let mut queued = BTreeSet::from([cfg.entry]);
    while let Some(block) = pending.pop_front() {
        queued.remove(&block);
        let mut next_in = BTreeSet::new();
        if block == cfg.entry {
            next_in.insert(ReachingDefinition::Incoming);
        }
        for predecessor in cfg.predecessors.get(&block).into_iter().flatten() {
            if cfg.reachable.contains(predecessor) {
                next_in.extend(outgoing.get(predecessor).into_iter().flatten().cloned());
            }
        }
        let mut next_out = next_in.clone();
        for event in by_block.get(&block).into_iter().flatten() {
            apply_reaching_event(&mut next_out, event);
        }
        incoming.insert(block, next_in);
        if outgoing.get(&block) != Some(&next_out) {
            outgoing.insert(block, next_out);
            for successor in cfg.successors.get(&block).into_iter().flatten() {
                if cfg.reachable.contains(successor) && queued.insert(*successor) {
                    pending.push_back(*successor);
                }
            }
        }
    }
    let mut at_publication = incoming.remove(&publication_block).unwrap_or_default();
    for event in by_block
        .get(&publication_block)
        .into_iter()
        .flatten()
        .filter(|event| event.site().is_some_and(|site| site < publication_site))
    {
        apply_reaching_event(&mut at_publication, event);
    }
    at_publication
}

fn apply_reaching_event(reaching: &mut BTreeSet<ReachingDefinition>, event: &ReachingDefinition) {
    match event {
        ReachingDefinition::LocalWrite(_) => {
            reaching.clear();
            reaching.insert(event.clone());
        }
        ReachingDefinition::UnknownCall(_) => {
            // An unsliced callee may write this location, but absent a model
            // it may also leave the prior definition intact.
            reaching.insert(event.clone());
        }
        ReachingDefinition::Incoming => {
            reaching.insert(ReachingDefinition::Incoming);
        }
    }
}

fn shortest_definition_witness(
    cfg: &ControlFlow,
    location: &LocationDefinitions,
    definition: &MemoryDefinition,
    publication_block: usize,
    publication_site: u32,
) -> Option<FlowCfgPathWitness> {
    let mut definitions = BTreeMap::<usize, BTreeSet<u32>>::new();
    for candidate in location.definitions.values() {
        definitions
            .entry(candidate.block)
            .or_default()
            .insert(candidate.site);
    }
    if definition.block == publication_block
        && definition.site < publication_site
        && definitions
            .get(&definition.block)
            .into_iter()
            .flatten()
            .all(|site| *site <= definition.site || *site >= publication_site)
    {
        return Some(cfg_witness(vec![definition.block]));
    }
    if definitions
        .get(&definition.block)
        .into_iter()
        .flatten()
        .any(|site| *site > definition.site)
    {
        return None;
    }

    let mut pending = VecDeque::new();
    let mut visited = BTreeSet::from([definition.block]);
    let mut predecessor = BTreeMap::<usize, usize>::new();
    for successor in cfg.successors.get(&definition.block).into_iter().flatten() {
        if visited.insert(*successor) {
            predecessor.insert(*successor, definition.block);
            pending.push_back(*successor);
        }
    }
    while let Some(block) = pending.pop_front() {
        if block == publication_block {
            let prefix_is_clear = definitions
                .get(&block)
                .into_iter()
                .flatten()
                .all(|site| *site >= publication_site || *site == definition.site);
            if prefix_is_clear {
                let mut path = vec![block];
                let mut cursor = block;
                while cursor != definition.block {
                    cursor = *predecessor.get(&cursor)?;
                    path.push(cursor);
                }
                path.reverse();
                return Some(cfg_witness(path));
            }
            continue;
        }
        if definitions
            .get(&block)
            .is_some_and(|sites| !sites.is_empty())
        {
            continue;
        }
        for successor in cfg.successors.get(&block).into_iter().flatten() {
            if visited.insert(*successor) {
                predecessor.insert(*successor, block);
                pending.push_back(*successor);
            }
        }
    }
    None
}

fn cfg_witness(blocks: Vec<usize>) -> FlowCfgPathWitness {
    FlowCfgPathWitness {
        blocks,
        proof: "shortest-conservative-cfg-path",
        path_feasibility_claim: false,
    }
}

fn call_clobbers_before_publication(
    body: &FunctionBody,
    cfg: &ControlFlow,
    publication_site: u32,
    selector: &PublicationSelectorRequest<'_>,
) -> Vec<CallClobber> {
    body.instructions
        .iter()
        .filter_map(|instruction| {
            if !matches!(
                instruction.control_flow.kind,
                FunctionControlFlowKind::Call | FunctionControlFlowKind::IndirectCall
            ) {
                return None;
            }
            let Ok(site) = u32::try_from(instruction.address) else {
                return None;
            };
            if site == publication_site
                && matches!(
                    selector,
                    PublicationSelectorRequest::Operation(_) | PublicationSelectorRequest::Call(_)
                )
            {
                return None;
            }
            if !structural_path_exists(cfg, site, publication_site) {
                return None;
            }
            cfg.instruction_blocks
                .get(&site)
                .copied()
                .map(|block| CallClobber { site, block })
        })
        .collect()
}

fn structural_path_exists(cfg: &ControlFlow, earlier_site: u32, later_site: u32) -> bool {
    let Some(&earlier) = cfg.instruction_blocks.get(&earlier_site) else {
        return false;
    };
    let Some(&later) = cfg.instruction_blocks.get(&later_site) else {
        return false;
    };
    if !cfg.reachable.contains(&earlier) || !cfg.reachable.contains(&later) {
        return false;
    }
    if earlier == later && earlier_site < later_site {
        return true;
    }
    let mut pending = VecDeque::new();
    let mut visited = if earlier == later {
        BTreeSet::new()
    } else {
        BTreeSet::from([earlier])
    };
    for successor in cfg.successors.get(&earlier).into_iter().flatten() {
        pending.push_back(*successor);
    }
    while let Some(block) = pending.pop_front() {
        if block == later {
            return true;
        }
        if !visited.insert(block) {
            continue;
        }
        for successor in cfg.successors.get(&block).into_iter().flatten() {
            pending.push_back(*successor);
        }
    }
    false
}

fn definitely_disjoint(
    left: &LocationKey,
    left_class: &ObjectClass,
    right: &LocationKey,
    right_class: &ObjectClass,
) -> bool {
    let left_width = i128::from(left.width.div_ceil(8));
    let right_width = i128::from(right.width.div_ceil(8));
    if left.object == right.object {
        return ranges_disjoint(
            i128::from(left.offset),
            left_width,
            i128::from(right.offset),
            right_width,
        );
    }
    match (left_class, right_class) {
        (
            ObjectClass::Absolute(left_space, left_base),
            ObjectClass::Absolute(right_space, right_base),
        ) if left_space == right_space => ranges_disjoint(
            *left_base + i128::from(left.offset),
            left_width,
            *right_base + i128::from(right.offset),
            right_width,
        ),
        _ => false,
    }
}

fn ranges_disjoint(left: i128, left_width: i128, right: i128, right_width: i128) -> bool {
    left + left_width <= right || right + right_width <= left
}

fn byte_ranges_overlap(left: i64, left_width: u8, right: i64, right_width: u8) -> bool {
    !ranges_disjoint(
        i128::from(left),
        i128::from(left_width.div_ceil(8)),
        i128::from(right),
        i128::from(right_width.div_ceil(8)),
    )
}

fn classify_object(object: &artifacts::StoredMemoryObject) -> ObjectClass {
    match object {
        artifacts::StoredMemoryObject::Absolute {
            address_space,
            address,
        } => ObjectClass::Absolute(address_space.clone(), i128::from(*address)),
        artifacts::StoredMemoryObject::Argument { .. }
        | artifacts::StoredMemoryObject::Global { .. }
        | artifacts::StoredMemoryObject::Allocation { .. }
        | artifacts::StoredMemoryObject::ZeroedAllocation { .. }
        | artifacts::StoredMemoryObject::Dereferenced { .. }
        | artifacts::StoredMemoryObject::Indexed { .. }
        | artifacts::StoredMemoryObject::OpaqueExternalObject { .. } => ObjectClass::Other,
    }
}

fn memory_object_identity(object: &artifacts::StoredMemoryObject) -> MemoryObjectIdentity {
    match object {
        artifacts::StoredMemoryObject::Argument { index } => MemoryObjectIdentity::Argument(*index),
        artifacts::StoredMemoryObject::Global { member, symbol } => {
            MemoryObjectIdentity::Global(member.clone(), symbol.clone())
        }
        artifacts::StoredMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => MemoryObjectIdentity::Dereferenced(
            Box::new(memory_object_identity(pointer)),
            *pointer_offset,
        ),
        artifacts::StoredMemoryObject::Absolute {
            address_space,
            address,
        } => MemoryObjectIdentity::Absolute(address_space.clone(), *address),
        artifacts::StoredMemoryObject::Indexed {
            object,
            argument,
            stride,
        } => MemoryObjectIdentity::Indexed(
            Box::new(memory_object_identity(object)),
            *argument,
            *stride,
        ),
        artifacts::StoredMemoryObject::Allocation { call_token } => {
            MemoryObjectIdentity::Allocation(*call_token)
        }
        artifacts::StoredMemoryObject::ZeroedAllocation { call_token } => {
            MemoryObjectIdentity::ZeroedAllocation(*call_token)
        }
        artifacts::StoredMemoryObject::OpaqueExternalObject { call_token } => {
            MemoryObjectIdentity::OpaqueExternalObject(*call_token)
        }
    }
}

fn display_object(object: &artifacts::StoredMemoryObject) -> String {
    match object {
        artifacts::StoredMemoryObject::Argument { index } => format!("arg{index}"),
        artifacts::StoredMemoryObject::Global { member, symbol } => member
            .as_deref()
            .map_or_else(|| symbol.clone(), |member| format!("{member}::{symbol}")),
        artifacts::StoredMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => format!("*({} {pointer_offset:+#x})", display_object(pointer)),
        artifacts::StoredMemoryObject::Absolute {
            address_space,
            address,
        } => format!("{address_space}:{address:#010x}"),
        artifacts::StoredMemoryObject::Indexed {
            object,
            argument,
            stride,
        } => format!("{}[arg{argument} * {stride:#x}]", display_object(object)),
        artifacts::StoredMemoryObject::Allocation { call_token } => {
            format!("allocation#{call_token}")
        }
        artifacts::StoredMemoryObject::ZeroedAllocation { call_token } => {
            format!("calloc#{call_token}")
        }
        artifacts::StoredMemoryObject::OpaqueExternalObject { call_token } => {
            format!("opaque-external#{call_token}")
        }
    }
}

#[cfg(test)]
fn instruction_block_for_site(body: &FunctionBody, site: u32) -> Option<usize> {
    instruction_at_site(body, site)
        .and_then(|instruction| block_for_address(body, instruction.address))
}

fn instruction_at_site(body: &FunctionBody, site: u32) -> Option<&crate::FunctionInstruction> {
    body.instructions
        .binary_search_by_key(&u64::from(site), |instruction| instruction.address)
        .ok()
        .map(|index| &body.instructions[index])
}

fn block_for_address(body: &FunctionBody, address: u64) -> Option<usize> {
    let offset = address.checked_sub(body.address)?;
    body.basic_blocks
        .iter()
        .find(|block| offset >= block.start_offset && offset < block.end_offset)
        .map(|block| block.id)
}

#[cfg(test)]
mod tests {
    use crate::artifact::FunctionBlockSuccessor;
    use crate::{FunctionBasicBlock, FunctionControlFlow, FunctionInstruction};

    use super::*;

    fn instruction(address: u64, kind: FunctionControlFlowKind) -> FunctionInstruction {
        FunctionInstruction {
            offset: address - 0x1000,
            address,
            width: 4,
            raw: "00000000".to_owned(),
            text: kind.label().to_owned(),
            supported: kind != FunctionControlFlowKind::Unknown,
            blocker_class: None,
            control_flow: FunctionControlFlow { kind, target: None },
            relocations: Vec::new(),
        }
    }

    fn body(
        instructions: Vec<FunctionInstruction>,
        successors: Vec<FunctionBlockSuccessor>,
    ) -> FunctionBody {
        let size = instructions.len() * 4;
        FunctionBody {
            artifact: "fixture.elf".to_owned(),
            member: None,
            symbol: "fixture".to_owned(),
            address: 0x1000,
            size,
            addresses_resolved: true,
            accounted_bytes: size,
            instructions,
            basic_blocks: vec![FunctionBasicBlock {
                id: 0,
                start_offset: 0,
                end_offset: size as u64,
                reachable: true,
                successors,
            }],
            loops: Vec::new(),
            labels: Vec::new(),
        }
    }

    fn cfg(entry: usize, edges: &[(usize, &[usize])], reachable: &[usize]) -> ControlFlow {
        let mut successors = edges
            .iter()
            .map(|(block, successors)| (*block, successors.iter().copied().collect()))
            .collect::<BTreeMap<_, BTreeSet<_>>>();
        let mut predecessors = reachable
            .iter()
            .map(|block| (*block, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for block in reachable {
            successors.entry(*block).or_default();
        }
        for (block, targets) in &successors {
            for target in targets {
                predecessors.entry(*target).or_default().insert(*block);
            }
        }
        ControlFlow {
            entry,
            successors,
            predecessors,
            reachable: reachable.iter().copied().collect(),
            instruction_blocks: BTreeMap::new(),
        }
    }

    fn location(definitions: &[(u32, usize)]) -> (LocationKey, LocationDefinitions) {
        let identity = MemoryObjectIdentity::Argument(0);
        (
            LocationKey {
                object: identity,
                offset: 0,
                width: 32,
            },
            LocationDefinitions {
                class: ObjectClass::Other,
                object: "arg0".to_owned(),
                definitions: definitions
                    .iter()
                    .map(|(site, block)| {
                        (
                            *site,
                            MemoryDefinition {
                                site: *site,
                                block: *block,
                                object: "arg0".to_owned(),
                                values: BTreeSet::from([format!("value@{site:#x}")]),
                                value_complete: true,
                            },
                        )
                    })
                    .collect(),
            },
        )
    }

    fn slice(
        cfg: &ControlFlow,
        publication_block: usize,
        publication_site: u32,
        locations: BTreeMap<LocationKey, LocationDefinitions>,
    ) -> (Vec<FlowMemorySliceEvidence>, Vec<FlowBlocker>) {
        let mut blockers = Vec::new();
        let slice = slice_at_publication(
            "fixture::function",
            publication_site,
            publication_block,
            cfg,
            &locations,
            &[],
            true,
            true,
            &mut blockers,
        );
        (slice, blockers)
    }

    fn publication(evidence: FlowPublicationEvidence) -> PublicationAnchor {
        PublicationAnchor {
            site: evidence.site,
            evidence,
        }
    }

    #[test]
    fn sequential_last_write_kills_the_earlier_write_and_excludes_later_writes() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let (key, location) = location(&[(0x1004, 0), (0x1008, 0), (0x1010, 0)]);
        let (slice, blockers) = slice(&cfg, 0, 0x100c, BTreeMap::from([(key, location)]));

        assert!(blockers.is_empty());
        assert_eq!(slice.len(), 1);
        assert!(!slice[0].incoming_definition_possible);
        assert_eq!(slice[0].definitions.len(), 1);
        assert_eq!(slice[0].definitions[0].site, 0x1008);
        assert_eq!(
            slice[0].definitions[0].classification,
            FlowMemoryDefinitionClassification::Must
        );
        assert_eq!(
            slice[0].definitions[0].witness.as_ref().unwrap().blocks,
            [0]
        );
    }

    #[test]
    fn diamond_reports_both_arm_writes_as_alternative_last_definitions() {
        let cfg = cfg(
            0,
            &[(0, &[1, 2]), (1, &[3]), (2, &[3]), (3, &[])],
            &[0, 1, 2, 3],
        );
        let (key, location) = location(&[(0x1010, 1), (0x1020, 2)]);
        let (slice, blockers) = slice(&cfg, 3, 0x1030, BTreeMap::from([(key, location)]));

        assert!(blockers.is_empty());
        assert!(!slice[0].incoming_definition_possible);
        assert_eq!(
            slice[0]
                .definitions
                .iter()
                .map(|definition| definition.site)
                .collect::<Vec<_>>(),
            [0x1010, 0x1020]
        );
        assert!(slice[0].definitions.iter().all(|definition| {
            definition.classification == FlowMemoryDefinitionClassification::Alternative
                && !definition.witness.as_ref().unwrap().path_feasibility_claim
        }));
    }

    #[test]
    fn one_sided_diamond_retains_the_incoming_definition_alternative() {
        let cfg = cfg(
            0,
            &[(0, &[1, 2]), (1, &[3]), (2, &[3]), (3, &[])],
            &[0, 1, 2, 3],
        );
        let (key, location) = location(&[(0x1010, 1)]);
        let (slice, blockers) = slice(&cfg, 3, 0x1030, BTreeMap::from([(key, location)]));

        assert!(blockers.is_empty());
        assert!(slice[0].incoming_definition_possible);
        assert_eq!(slice[0].definitions.len(), 1);
        assert_eq!(
            slice[0].definitions[0].classification,
            FlowMemoryDefinitionClassification::Alternative
        );
    }

    #[test]
    fn dead_block_write_does_not_enter_the_publication_slice() {
        let cfg = cfg(0, &[(0, &[2]), (1, &[2]), (2, &[])], &[0, 2]);
        let (key, location) = location(&[(0x1010, 1)]);
        let (slice, blockers) = slice(&cfg, 2, 0x1020, BTreeMap::from([(key, location)]));

        assert!(blockers.is_empty());
        assert!(slice.is_empty());
    }

    #[test]
    fn loop_fixed_point_retains_entry_and_loop_body_last_definitions() {
        let cfg = cfg(
            0,
            &[(0, &[1]), (1, &[2, 3]), (2, &[1]), (3, &[])],
            &[0, 1, 2, 3],
        );
        let (key, location) = location(&[(0x1000, 0), (0x1020, 2)]);
        let (slice, blockers) = slice(&cfg, 3, 0x1030, BTreeMap::from([(key, location)]));

        assert!(blockers.is_empty());
        assert!(!slice[0].incoming_definition_possible);
        assert_eq!(slice[0].definitions.len(), 2);
        assert!(slice[0].definitions.iter().all(|definition| {
            definition.classification == FlowMemoryDefinitionClassification::Alternative
        }));
    }

    #[test]
    fn partially_overlapping_slots_fail_closed() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let (first_key, first) = location(&[(0x1000, 0)]);
        let (_, second) = location(&[(0x1004, 0)]);
        let second_key = LocationKey {
            object: MemoryObjectIdentity::Argument(0),
            offset: 2,
            width: 32,
        };
        let (slice, blockers) = slice(
            &cfg,
            0,
            0x1010,
            BTreeMap::from([(first_key, first), (second_key, second)]),
        );

        assert_eq!(slice.len(), 2);
        assert!(slice.iter().all(|item| !item.alias_complete));
        assert!(
            blockers
                .iter()
                .any(|blocker| { blocker.kind == "partially-overlapping-memory-definitions" })
        );
        assert!(
            slice
                .iter()
                .flat_map(|item| &item.definitions)
                .all(|definition| {
                    definition.classification == FlowMemoryDefinitionClassification::Candidate
                })
        );
    }

    #[test]
    fn same_site_variants_merge_values_without_creating_extra_definitions() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let key = LocationKey {
            object: MemoryObjectIdentity::Argument(0),
            offset: 0,
            width: 32,
        };
        let mut locations = BTreeMap::new();
        let mut blockers = Vec::new();
        for value in ["path-a", "path-b"] {
            insert_definition(
                &mut locations,
                key.clone(),
                ObjectClass::Other,
                "arg0".to_owned(),
                MemoryDefinition {
                    site: 0x1004,
                    block: 0,
                    object: "arg0".to_owned(),
                    values: BTreeSet::from([value.to_owned()]),
                    value_complete: true,
                },
                "fixture::function",
                &mut blockers,
            );
        }
        insert_definition(
            &mut locations,
            key,
            ObjectClass::Other,
            "arg0".to_owned(),
            MemoryDefinition {
                site: 0x1004,
                block: 0,
                object: "arg0".to_owned(),
                values: BTreeSet::new(),
                value_complete: false,
            },
            "fixture::function",
            &mut blockers,
        );

        assert!(blockers.is_empty());
        let (slice, slice_blockers) = slice(&cfg, 0, 0x1008, locations);
        assert!(slice_blockers.is_empty());
        assert_eq!(slice[0].definitions.len(), 1);
        assert_eq!(slice[0].definitions[0].values, ["path-a", "path-b"]);
        assert!(!slice[0].definitions[0].value_complete);
        assert_eq!(
            slice[0].definitions[0].classification,
            FlowMemoryDefinitionClassification::Must
        );
    }

    #[test]
    fn disjoint_fields_of_the_same_nested_object_have_complete_alias_sets() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let object =
            MemoryObjectIdentity::Dereferenced(Box::new(MemoryObjectIdentity::Argument(0)), 0);
        let locations = [
            (
                LocationKey {
                    object: object.clone(),
                    offset: 0,
                    width: 32,
                },
                0x1000,
            ),
            (
                LocationKey {
                    object,
                    offset: 8,
                    width: 32,
                },
                0x1004,
            ),
        ]
        .into_iter()
        .map(|(key, site)| {
            (
                key,
                LocationDefinitions {
                    class: ObjectClass::Other,
                    object: "*(arg0 +0x0)".to_owned(),
                    definitions: BTreeMap::from([(
                        site,
                        MemoryDefinition {
                            site,
                            block: 0,
                            object: "*(arg0 +0x0)".to_owned(),
                            values: BTreeSet::from([format!("value@{site:#x}")]),
                            value_complete: true,
                        },
                    )]),
                },
            )
        })
        .collect();

        let (slice, blockers) = slice(&cfg, 0, 0x1008, locations);
        assert!(blockers.is_empty());
        assert_eq!(slice.len(), 2);
        assert!(slice.iter().all(|item| item.alias_complete));
        assert!(
            slice
                .iter()
                .flat_map(|item| &item.definitions)
                .all(|definition| {
                    definition.classification == FlowMemoryDefinitionClassification::Must
                })
        );
    }

    #[test]
    fn reached_incomplete_profile_outranks_not_reached_profile() {
        assert!(
            publication_profile_priority(FlowStatus::Incomplete, true)
                < publication_profile_priority(FlowStatus::NotReached, false)
        );
        assert!(
            publication_profile_priority(FlowStatus::Complete, true)
                < publication_profile_priority(FlowStatus::Incomplete, true)
        );
    }

    #[test]
    fn instruction_site_authentication_rejects_mid_instruction_offsets() {
        let body = body(
            vec![instruction(0x1000, FunctionControlFlowKind::Linear)],
            Vec::new(),
        );
        assert_eq!(instruction_block_for_site(&body, 0x1000), Some(0));
        assert_eq!(instruction_block_for_site(&body, 0x1002), None);
    }

    #[test]
    fn resolved_external_tail_jump_is_a_terminal_cfg_edge() {
        let mut jump = instruction(0x1000, FunctionControlFlowKind::Jump);
        jump.control_flow.target = Some(0x2000);
        let body = body(
            vec![jump],
            vec![FunctionBlockSuccessor {
                kind: "jump".to_owned(),
                block: None,
                target: Some(0x2000),
            }],
        );

        let cfg = build_control_flow(&body).expect("external direct tail jump is terminal");
        assert!(cfg.successors[&0].is_empty());
    }

    #[test]
    fn indexed_candidate_anchors_merge_guards_without_becoming_exact() {
        let base = FlowPublicationEvidence {
            evidence: EvidenceLevel::Observed,
            function: "source::function".to_owned(),
            site: Some(0x1000),
            kind: "register".to_owned(),
            target: "MODEM.ENTRY4".to_owned(),
            selector_exact: false,
            site_authenticated: false,
            persisted_blocks: vec![0],
            persisted_block_complete: true,
            tails: Vec::new(),
            modes: vec!["indexed-candidate".to_owned()],
            paths: vec!["entry".to_owned()],
            guards: Vec::new(),
            operation: None,
            address: Some(0x2000),
            registers: vec!["MODEM.ENTRY4".to_owned()],
        };
        let mut guarded = base.clone();
        guarded.guards.push("arg0 <= 5".to_owned());

        let merged = merge_publication_anchors(vec![publication(base), publication(guarded)]);
        assert_eq!(merged.len(), 1);
        assert!(!merged[0].evidence.selector_exact);
        assert_eq!(merged[0].evidence.guards, ["arg0 <= 5"]);
    }

    #[test]
    fn incomplete_inventory_or_candidate_publication_never_emits_must() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let (key, location) = location(&[(0x1004, 0)]);
        for (definition_set_complete, publication_exact) in [(false, true), (true, false)] {
            let mut blockers = Vec::new();
            let slice = slice_at_publication(
                "fixture::function",
                0x1008,
                0,
                &cfg,
                &BTreeMap::from([(key.clone(), location.clone())]),
                &[],
                definition_set_complete,
                publication_exact,
                &mut blockers,
            );
            assert_eq!(
                slice[0].definitions[0].classification,
                FlowMemoryDefinitionClassification::Candidate
            );
        }
    }

    #[test]
    fn differently_named_globals_are_not_proven_disjoint() {
        let left = LocationKey {
            object: MemoryObjectIdentity::Global(None, "image".to_owned()),
            offset: 0x24,
            width: 32,
        };
        let right = LocationKey {
            object: MemoryObjectIdentity::Global(None, "state".to_owned()),
            offset: 4,
            width: 32,
        };
        assert!(!definitely_disjoint(
            &left,
            &ObjectClass::Other,
            &right,
            &ObjectClass::Other
        ));
    }

    #[test]
    fn absolute_ranges_are_disjoint_only_inside_the_same_address_space() {
        let left = LocationKey {
            object: MemoryObjectIdentity::Absolute("ram".to_owned(), 0x1000),
            offset: 0,
            width: 32,
        };
        let right = LocationKey {
            object: MemoryObjectIdentity::Absolute("ram".to_owned(), 0x2000),
            offset: 0,
            width: 32,
        };
        assert!(definitely_disjoint(
            &left,
            &ObjectClass::Absolute("ram".to_owned(), 0x1000),
            &right,
            &ObjectClass::Absolute("ram".to_owned(), 0x2000)
        ));
        assert!(!definitely_disjoint(
            &left,
            &ObjectClass::Absolute("ram".to_owned(), 0x1000),
            &right,
            &ObjectClass::Absolute("other".to_owned(), 0x2000)
        ));
    }

    #[test]
    fn invalid_memory_effect_vocabulary_is_rejected() {
        assert_eq!(memory_effect_shape_error("store", 32), Some("access"));
        assert_eq!(memory_effect_shape_error("write", 0), Some("width"));
        assert_eq!(memory_effect_shape_error("write", 7), Some("width"));
        assert_eq!(memory_effect_shape_error("read", 16), None);
    }

    #[test]
    fn authenticated_calls_are_clobbers_even_without_persisted_call_facts() {
        let body = body(
            vec![
                instruction(0x1000, FunctionControlFlowKind::Call),
                instruction(0x1004, FunctionControlFlowKind::Linear),
            ],
            Vec::new(),
        );
        let cfg = build_control_flow(&body).unwrap();
        let clobbers = call_clobbers_before_publication(
            &body,
            &cfg,
            0x1004,
            &PublicationSelectorRequest::Register("MODEM.HEAD"),
        );
        assert_eq!(
            clobbers,
            [CallClobber {
                site: 0x1000,
                block: 0
            }]
        );
        assert!(
            call_clobbers_before_publication(
                &body,
                &cfg,
                0x1000,
                &PublicationSelectorRequest::Call("source::callee")
            )
            .is_empty()
        );
    }

    #[test]
    fn local_write_after_call_kills_the_possible_call_clobber_for_that_location() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let (key, location) = location(&[(0x1004, 0)]);
        let clobbers = [CallClobber {
            site: 0x1000,
            block: 0,
        }];
        let mut blockers = Vec::new();
        let slice = slice_at_publication(
            "fixture::function",
            0x1008,
            0,
            &cfg,
            &BTreeMap::from([(key, location)]),
            &clobbers,
            true,
            true,
            &mut blockers,
        );
        assert!(blockers.is_empty());
        assert!(slice[0].alias_complete);
        assert_eq!(
            slice[0].definitions[0].classification,
            FlowMemoryDefinitionClassification::Must
        );
    }

    #[test]
    fn call_after_local_write_retains_the_write_as_an_open_candidate() {
        let cfg = cfg(0, &[(0, &[])], &[0]);
        let (key, location) = location(&[(0x1000, 0)]);
        let clobbers = [CallClobber {
            site: 0x1004,
            block: 0,
        }];
        let mut blockers = Vec::new();
        let slice = slice_at_publication(
            "fixture::function",
            0x1008,
            0,
            &cfg,
            &BTreeMap::from([(key, location)]),
            &clobbers,
            true,
            true,
            &mut blockers,
        );
        assert!(!slice[0].alias_complete);
        assert_eq!(
            slice[0].definitions[0].classification,
            FlowMemoryDefinitionClassification::Candidate
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.kind == "interprocedural-memory-clobber")
        );
    }
}
