//! Target-directed, project-aware inter-function value-flow investigation.
//!
//! This is intentionally a navigation and evidence report. It composes exact
//! constants and unchanged ABI inputs across recovered call edges, but never
//! upgrades symbolic expressions or incomplete leaf analysis into proof.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::{ProjectSpec, Result, artifacts};

#[derive(Clone, Debug)]
pub(crate) enum FlowTargetRequest<'a> {
    Function(&'a str),
    Register(&'a str),
    Address(u32),
}

#[derive(Clone, Debug)]
pub(crate) struct FlowInvestigationRequest<'a> {
    pub(crate) source: &'a str,
    pub(crate) root_symbol: &'a str,
    pub(crate) target: FlowTargetRequest<'a>,
    pub(crate) max_depth: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowInvestigationReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) profile: String,
    pub(crate) linked_ir: String,
    pub(crate) root: String,
    pub(crate) target_kind: &'static str,
    pub(crate) target: String,
    pub(crate) reached: bool,
    pub(crate) complete: bool,
    pub(crate) edges: Vec<FlowEdgeEvidence>,
    pub(crate) sink_effects: Vec<FlowSinkEffect>,
    pub(crate) blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowEdgeEvidence {
    pub(crate) ordinal: usize,
    pub(crate) caller: String,
    pub(crate) callee: String,
    pub(crate) site: Option<u32>,
    pub(crate) kind: String,
    pub(crate) tail: bool,
    pub(crate) argument_shapes: usize,
    pub(crate) arguments: Vec<FlowArgumentEvidence>,
    pub(crate) guards: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowArgumentEvidence {
    pub(crate) position: usize,
    pub(crate) local: String,
    pub(crate) resolved: String,
    pub(crate) constants: Vec<u32>,
    pub(crate) provenance: &'static str,
    pub(crate) pointee: Vec<FlowPointeeEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowPointeeEvidence {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) local: String,
    pub(crate) resolved: String,
    pub(crate) constants: Vec<u32>,
    pub(crate) provenance: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct FlowSinkEffect {
    pub(crate) function: String,
    pub(crate) access: String,
    pub(crate) width: u8,
    pub(crate) address: u32,
    pub(crate) register: String,
    pub(crate) value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ValueDomain {
    Constants(BTreeSet<u32>),
    RootArgument(usize),
    Symbolic(String),
    Unknown,
}

impl ValueDomain {
    fn render(&self) -> String {
        match self {
            Self::Constants(values) if values.len() == 1 => {
                format!("{:#010x}", values.first().expect("one value"))
            }
            Self::Constants(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|value| format!("{value:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::RootArgument(position) => format!("root.arg{position}"),
            Self::Symbolic(value) => value.clone(),
            Self::Unknown => "unknown".to_owned(),
        }
    }

    fn provenance(&self) -> &'static str {
        match self {
            Self::Constants(_) => "exact-constant-domain",
            Self::RootArgument(_) => "root-argument",
            Self::Symbolic(_) => "uncomposed-symbolic-expression",
            Self::Unknown => "unknown",
        }
    }

    fn constants(&self) -> Vec<u32> {
        match self {
            Self::Constants(values) => values.iter().copied().collect(),
            _ => Vec::new(),
        }
    }
}

pub(crate) fn investigate(
    request: FlowInvestigationRequest<'_>,
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
        let functions = reader.read_all_functions()?;
        let roots = functions
            .iter()
            .filter(|function| {
                function.source == request.source && function.symbol == request.root_symbol
            })
            .collect::<Vec<_>>();
        if roots.len() != 1 {
            continue;
        }
        let root = roots[0];
        let target_identities = target_identities(&functions, &request.target);
        if target_identities.is_empty() {
            continue;
        }
        let graph = reader.graph_slice(&root.identity, request.max_depth, false);
        let Some(path) = shortest_path(
            &root.identity,
            &target_identities,
            &graph,
            request.max_depth,
        ) else {
            continue;
        };
        let by_identity = functions
            .iter()
            .map(|function| (function.identity.as_str(), function))
            .collect::<BTreeMap<_, _>>();
        let mut domains = (0..16).map(ValueDomain::RootArgument).collect::<Vec<_>>();
        let mut edges = Vec::new();
        let mut blockers = Vec::new();
        for (ordinal, edge) in path.iter().enumerate() {
            let Some(caller) = by_identity.get(edge.caller.as_str()).copied() else {
                blockers.push(format!("missing caller record {}", edge.caller));
                continue;
            };
            let call = caller.calls.iter().find(|call| {
                call.target == edge.callee && call.site == edge.site && call.kind == edge.kind
            });
            let Some(call) = call else {
                blockers.push(format!(
                    "graph edge {} -> {} has no matching call fact",
                    edge.caller, edge.callee
                ));
                continue;
            };
            let next = call
                .arguments
                .iter()
                .map(|value| resolve_value(value, &domains))
                .collect::<Vec<_>>();
            let arguments: Vec<FlowArgumentEvidence> = call
                .arguments
                .iter()
                .zip(&next)
                .enumerate()
                .map(|(position, (local, resolved))| FlowArgumentEvidence {
                    position,
                    local: local.clone(),
                    resolved: resolved.render(),
                    constants: resolved.constants(),
                    provenance: resolved.provenance(),
                    pointee: stack_pointee(caller, call.site, local, &domains),
                })
                .collect();
            for argument in &arguments {
                if argument.local.starts_with("private-stack:") && argument.pointee.is_empty() {
                    blockers.push(format!(
                        "{} -> {} passes an unresolved private stack object in a{}",
                        edge.caller, edge.callee, argument.position
                    ));
                }
            }
            if call.arguments.is_empty() {
                blockers.push(format!(
                    "{} -> {} has no recovered ABI arguments",
                    edge.caller, edge.callee
                ));
            }
            domains = next;
            edges.push(FlowEdgeEvidence {
                ordinal,
                caller: edge.caller.clone(),
                callee: edge.callee.clone(),
                site: edge.site,
                kind: edge.kind.clone(),
                tail: call.tail(),
                argument_shapes: call.argument_shapes(),
                arguments,
                guards: call.guard_expressions(),
            });
        }
        let sink = path
            .last()
            .map_or(root.identity.as_str(), |edge| edge.callee.as_str());
        let mut sink_effects = by_identity
            .get(sink)
            .into_iter()
            .flat_map(|function| &function.mmio_accesses)
            .filter(|effect| target_matches_effect(&request.target, effect))
            .map(|effect| FlowSinkEffect {
                function: sink.to_owned(),
                access: effect.access().to_owned(),
                width: effect.width(),
                address: effect.address,
                register: effect.register().to_owned(),
                value: effect.value().map(str::to_owned),
            })
            .collect::<Vec<_>>();
        if let Some(function) = by_identity.get(sink) {
            for effect in &function.instruction_effects {
                let Some((access, width, address, register, value)) = effect.mmio() else {
                    continue;
                };
                if !target_matches_mmio(&request.target, address, register) {
                    continue;
                }
                let evidence = FlowSinkEffect {
                    function: sink.to_owned(),
                    access: access.to_owned(),
                    width,
                    address,
                    register: register.to_owned(),
                    value: value.map(str::to_owned),
                };
                if !sink_effects.contains(&evidence) {
                    sink_effects.push(evidence);
                }
            }
        }
        let sink_complete = by_identity
            .get(sink)
            .is_some_and(|function| function.complete);
        reports.push(FlowInvestigationReport {
            schema_version: 1,
            command: "inspect flow",
            profile: profile.id.clone(),
            linked_ir: profile.output.display().to_string(),
            root: root.identity.clone(),
            target_kind: target_kind(&request.target),
            target: target_label(&request.target),
            reached: true,
            complete: blockers.is_empty() && sink_complete,
            edges,
            sink_effects,
            blockers,
        });
    }
    reports
        .into_iter()
        .min_by_key(|report| (report.edges.len(), report.profile.clone()))
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "no generated linked-IR profile contains a path from {}:{} to {}; run `project analyze` after adding both functions to one IR profile",
                request.source,
                request.root_symbol,
                target_label(&request.target)
            ))
        })
}

fn target_identities(
    functions: &[artifacts::StoredFunction],
    target: &FlowTargetRequest<'_>,
) -> BTreeSet<String> {
    functions
        .iter()
        .filter(|function| match target {
            FlowTargetRequest::Function(target) => {
                function.identity == *target || function.symbol == *target
            }
            FlowTargetRequest::Register(register) => {
                function
                    .mmio_accesses
                    .iter()
                    .any(|effect| effect.register() == *register)
                    || function.instruction_effects.iter().any(|effect| {
                        effect
                            .mmio()
                            .is_some_and(|(_, _, _, candidate, _)| candidate == *register)
                    })
            }
            FlowTargetRequest::Address(address) => {
                function
                    .mmio_accesses
                    .iter()
                    .any(|effect| effect.address == *address)
                    || function.instruction_effects.iter().any(|effect| {
                        effect
                            .mmio()
                            .is_some_and(|(_, _, candidate, _, _)| candidate == *address)
                    })
            }
        })
        .map(|function| function.identity.clone())
        .collect()
}

fn shortest_path(
    root: &str,
    targets: &BTreeSet<String>,
    graph: &[artifacts::StoredGraphEdge],
    max_depth: usize,
) -> Option<Vec<artifacts::StoredGraphEdge>> {
    if targets.contains(root) {
        return Some(Vec::new());
    }
    let mut queue = VecDeque::from([(root.to_owned(), Vec::new())]);
    let mut visited = BTreeSet::from([root.to_owned()]);
    while let Some((node, path)) = queue.pop_front() {
        if path.len() >= max_depth {
            continue;
        }
        for edge in graph.iter().filter(|edge| edge.caller == node) {
            let mut next = path.clone();
            next.push(edge.clone());
            if targets.contains(&edge.callee) {
                return Some(next);
            }
            if visited.insert(edge.callee.clone()) {
                queue.push_back((edge.callee.clone(), next));
            }
        }
    }
    None
}

fn resolve_value(value: &str, caller: &[ValueDomain]) -> ValueDomain {
    if let Some(value) = value.strip_prefix("const:") {
        return parse_u32(value)
            .map(|value| ValueDomain::Constants(BTreeSet::from([value])))
            .unwrap_or(ValueDomain::Unknown);
    }
    if let Some(values) = value
        .strip_prefix("one-of(")
        .and_then(|value| value.strip_suffix(')'))
    {
        let constants = values
            .split(',')
            .filter_map(parse_u32)
            .collect::<BTreeSet<_>>();
        return if constants.is_empty() {
            ValueDomain::Unknown
        } else {
            ValueDomain::Constants(constants)
        };
    }
    if let Some(position) = value
        .strip_prefix("arg")
        .and_then(|value| value.parse::<usize>().ok())
    {
        return caller
            .get(position)
            .cloned()
            .unwrap_or(ValueDomain::Unknown);
    }
    if value == "unknown" || value.starts_with("varies-across-") {
        ValueDomain::Unknown
    } else {
        ValueDomain::Symbolic(value.to_owned())
    }
}

fn stack_pointee(
    function: &artifacts::StoredFunction,
    call_site: Option<u32>,
    pointer: &str,
    caller: &[ValueDomain],
) -> Vec<FlowPointeeEvidence> {
    let Some(base) = parse_stack_offset(pointer) else {
        return Vec::new();
    };
    let mut latest = BTreeMap::<(i32, u8), (u32, &artifacts::StoredFlowValue)>::new();
    for fact in &function.local_value_flow {
        let artifacts::StoredLocalValueFlow::StackStore {
            site,
            offset,
            width,
            value,
        } = fact
        else {
            continue;
        };
        if call_site.is_some_and(|call_site| *site >= call_site) {
            continue;
        }
        let relative = offset.wrapping_sub(base);
        if !(0..=256).contains(&relative) {
            continue;
        }
        let key = (relative, *width);
        if latest.get(&key).is_none_or(|(previous, _)| previous < site) {
            latest.insert(key, (*site, value));
        }
    }
    latest
        .into_iter()
        .map(|((offset, width), (_, value))| {
            let resolved = value.constant.map_or_else(
                || {
                    value
                        .input
                        .and_then(|position| caller.get(usize::from(position)).cloned())
                        .unwrap_or_else(|| resolve_value(&value.expression, caller))
                },
                |value| ValueDomain::Constants(BTreeSet::from([value])),
            );
            FlowPointeeEvidence {
                offset,
                width,
                local: value.expression.clone(),
                resolved: resolved.render(),
                constants: resolved.constants(),
                provenance: resolved.provenance(),
            }
        })
        .collect()
}

fn parse_stack_offset(value: &str) -> Option<i32> {
    let value = value.strip_prefix("private-stack:")?;
    let value = value.strip_prefix('+').unwrap_or(value);
    if let Some(hex) = value.strip_prefix("-0x") {
        return i32::try_from(u32::from_str_radix(hex, 16).ok()?)
            .ok()
            .and_then(i32::checked_neg);
    }
    value
        .strip_prefix("0x")
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map(|value| value as i32)
        .or_else(|| value.parse().ok())
}

fn parse_u32(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .or_else(|| value.parse().ok())
}

fn target_matches_effect(
    target: &FlowTargetRequest<'_>,
    effect: &artifacts::StoredMmioAccess,
) -> bool {
    match target {
        FlowTargetRequest::Function(_) => true,
        FlowTargetRequest::Register(register) => effect.register() == *register,
        FlowTargetRequest::Address(address) => effect.address == *address,
    }
}

fn target_matches_mmio(target: &FlowTargetRequest<'_>, address: u32, register: &str) -> bool {
    match target {
        FlowTargetRequest::Function(_) => true,
        FlowTargetRequest::Register(candidate) => register == *candidate,
        FlowTargetRequest::Address(candidate) => address == *candidate,
    }
}

fn target_kind(target: &FlowTargetRequest<'_>) -> &'static str {
    match target {
        FlowTargetRequest::Function(_) => "function",
        FlowTargetRequest::Register(_) => "register",
        FlowTargetRequest::Address(_) => "address",
    }
}

fn target_label(target: &FlowTargetRequest<'_>) -> String {
    match target {
        FlowTargetRequest::Function(value) | FlowTargetRequest::Register(value) => {
            (*value).to_owned()
        }
        FlowTargetRequest::Address(value) => format!("{value:#010x}"),
    }
}
