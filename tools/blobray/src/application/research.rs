//! Explainable, scope-aware prioritization of the next research action.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use super::ProjectSession;
use crate::{
    Result,
    artifacts::LinkedIrReader,
    registers::{RegisterFacts, load_effective_register_model},
    review_scopes::{ReviewScopeReport, ReviewScopesDocument},
};

pub(crate) const RESEARCH_SCHEMA: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchScoreBreakdown {
    pub(crate) guaranteed_weight: u64,
    pub(crate) optimistic_weight: u64,
    pub(crate) marginal_weight: u64,
    pub(crate) root_weight: u64,
    pub(crate) verification_weight: u64,
    pub(crate) publication_weight: u64,
    pub(crate) cost_penalty: u64,
    pub(crate) co_blocker_penalty: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RelatedResearchFinding {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) knowledge_required: String,
    pub(crate) summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchCandidate {
    pub(crate) rank: usize,
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) severity: String,
    pub(crate) score: u64,
    pub(crate) direct_functions: usize,
    pub(crate) guaranteed_unlock: usize,
    pub(crate) optimistic_unlock: usize,
    pub(crate) marginal_unlock_after_co_blockers: usize,
    pub(crate) co_blockers: usize,
    pub(crate) affected_scope_roots: Vec<String>,
    pub(crate) scopes: Vec<String>,
    pub(crate) verification_surfaces: Vec<String>,
    pub(crate) publication_scopes: Vec<String>,
    pub(crate) estimated_cost: String,
    pub(crate) confidence: String,
    pub(crate) knowledge_required: String,
    pub(crate) next_command: String,
    pub(crate) summary: String,
    /// Other independently scored findings resolved by the same next action.
    /// They remain visible rather than consuming duplicate ranked slots.
    pub(crate) related_findings: Vec<RelatedResearchFinding>,
    pub(crate) score_breakdown: ResearchScoreBreakdown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ResearchNextReport {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) project: String,
    pub(crate) scope: Option<String>,
    pub(crate) analyzed_scopes: Vec<String>,
    /// Count before grouping findings that lead to the same user action.
    pub(crate) total_candidates: usize,
    pub(crate) total_actions: usize,
    pub(crate) returned_candidates: usize,
    pub(crate) verification_diagnostic: Option<String>,
    pub(crate) candidates: Vec<ResearchCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphNode {
    source: String,
    symbol: String,
    dependencies: BTreeSet<String>,
    complete: bool,
}

#[derive(Debug, Default)]
struct ScopeGraph {
    nodes: BTreeMap<String, GraphNode>,
    outgoing: BTreeMap<String, BTreeSet<String>>,
    incoming: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug)]
struct Accumulator {
    id: String,
    kind: String,
    severity: String,
    message: String,
    direct: BTreeSet<String>,
    guaranteed: BTreeSet<String>,
    optimistic: BTreeSet<String>,
    marginal: BTreeSet<String>,
    co_blockers: BTreeSet<String>,
    roots: BTreeSet<String>,
    scopes: BTreeSet<String>,
    publication_scopes: BTreeSet<String>,
}

struct Seed {
    id: String,
    kind: String,
    severity: String,
    message: String,
    direct: BTreeSet<String>,
    guaranteed: BTreeSet<String>,
    optimistic: BTreeSet<String>,
    marginal: BTreeSet<String>,
    co_blockers: BTreeSet<String>,
    roots: BTreeSet<String>,
}

pub(crate) fn next(
    session: &ProjectSession,
    scope_filter: Option<&str>,
    limit: usize,
) -> Result<ResearchNextReport> {
    if limit == 0 {
        return Err(crate::Error::invalid(
            "research next limit must be non-zero",
        ));
    }
    let document = crate::review_scopes::load_for_project(&session.project)?;
    let scopes = select_scopes(&document, scope_filter)?;
    let analyzed_scopes = scopes.iter().map(|scope| scope.id.clone()).collect();
    let graphs = scopes
        .iter()
        .map(|scope| Ok((scope.id.clone(), load_graph(session, scope)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut candidates = BTreeMap::new();
    for scope in &scopes {
        add_blockers(scope, &graphs[&scope.id], &mut candidates);
    }
    add_registers(session, &scopes, &graphs, &mut candidates)?;
    add_unknown_semantics(session, &scopes, &graphs, &mut candidates)?;
    add_interfaces(session, &scopes, &graphs, &mut candidates)?;
    attach_candidate_co_blockers(&mut candidates);
    let (surfaces, verification_diagnostic) = verification_surfaces(&session.project);
    let mut ranked = candidates
        .into_values()
        .map(|candidate| finalize(candidate, &surfaces))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.guaranteed_unlock.cmp(&left.guaranteed_unlock))
            .then_with(|| right.optimistic_unlock.cmp(&left.optimistic_unlock))
            .then_with(|| left.id.cmp(&right.id))
    });
    let total_candidates = ranked.len();
    let mut ranked = coalesce_actions(ranked);
    let total_actions = ranked.len();
    ranked.truncate(limit);
    for (index, candidate) in ranked.iter_mut().enumerate() {
        candidate.rank = index + 1;
    }
    Ok(ResearchNextReport {
        schema_version: RESEARCH_SCHEMA,
        command: "research next".to_owned(),
        project: session.project.id.clone(),
        scope: scope_filter.map(str::to_owned),
        analyzed_scopes,
        total_candidates,
        total_actions,
        returned_candidates: ranked.len(),
        verification_diagnostic,
        candidates: ranked,
    })
}

fn select_scopes<'a>(
    document: &'a ReviewScopesDocument,
    selected: Option<&str>,
) -> Result<Vec<&'a ReviewScopeReport>> {
    if let Some(selected) = selected
        && !document.scopes.iter().any(|scope| scope.id == selected)
    {
        return Err(crate::Error::invalid(format!(
            "unknown review scope {selected:?}"
        )));
    }
    Ok(document
        .scopes
        .iter()
        .filter(|scope| selected.is_none_or(|selected| scope.id == selected))
        .collect())
}

fn load_graph(session: &ProjectSession, scope: &ReviewScopeReport) -> Result<ScopeGraph> {
    let selected = scope
        .function_identities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut nodes = BTreeMap::<String, GraphNode>::new();
    for profile_id in &scope.profiles {
        let profile = session
            .project
            .ir_profiles
            .iter()
            .find(|profile| profile.id == *profile_id)
            .ok_or_else(|| crate::Error::invalid(format!("unknown IR profile {profile_id:?}")))?;
        for function in LinkedIrReader::open(&profile.output)?
            .read_review_projection()?
            .functions
        {
            if !selected.contains(function.identity.as_str()) {
                continue;
            }
            let node = GraphNode {
                source: function.source,
                symbol: function.symbol,
                dependencies: function.dependencies.into_iter().collect(),
                complete: function.completeness.body_complete
                    && function.completeness.call_targets_complete
                    && function.completeness.transitive_effects_complete
                    && function.completeness.executable_complete
                    && function.diagnostics.is_empty()
                    && function.decode_blockers.is_empty(),
            };
            match nodes.entry(function.identity.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(node);
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    if existing.source != node.source
                        || existing.symbol != node.symbol
                        || existing.dependencies != node.dependencies
                    {
                        return Err(crate::Error::invalid(format!(
                            "scope {:?} has inconsistent projections for {:?}",
                            scope.id, function.identity
                        )));
                    }
                    existing.complete &= node.complete;
                }
            }
        }
    }
    let mut graph = ScopeGraph {
        nodes,
        ..ScopeGraph::default()
    };
    for identity in graph.nodes.keys().cloned().collect::<Vec<_>>() {
        for dependency in graph.nodes[&identity].dependencies.clone() {
            if let Some(target) = resolve_function(&graph, &dependency) {
                graph
                    .outgoing
                    .entry(identity.clone())
                    .or_default()
                    .insert(target.clone());
                graph
                    .incoming
                    .entry(target)
                    .or_default()
                    .insert(identity.clone());
            }
        }
    }
    Ok(graph)
}

fn resolve_function(graph: &ScopeGraph, selector: &str) -> Option<String> {
    if graph.nodes.contains_key(selector) {
        return Some(selector.to_owned());
    }
    let matches = graph
        .nodes
        .iter()
        .filter(|(identity, node)| {
            node.symbol == selector
                || format!("{}:{}", node.source, node.symbol) == selector
                || identity.ends_with(&format!("::{selector}"))
        })
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches[0].clone())
}

fn reverse_reachable(graph: &ScopeGraph, starts: &BTreeSet<String>) -> BTreeSet<String> {
    let mut reached = starts.clone();
    let mut queue = VecDeque::from_iter(starts.iter().cloned());
    while let Some(function) = queue.pop_front() {
        for caller in graph.incoming.get(&function).into_iter().flatten() {
            if reached.insert(caller.clone()) {
                queue.push_back(caller.clone());
            }
        }
    }
    reached
}

fn add_blockers(
    scope: &ReviewScopeReport,
    graph: &ScopeGraph,
    candidates: &mut BTreeMap<String, Accumulator>,
) {
    let mut by_function = BTreeMap::<String, BTreeSet<String>>::new();
    for item in &scope.review_queue {
        for function in &item.functions {
            by_function
                .entry(function.clone())
                .or_default()
                .insert(item.id.clone());
        }
    }
    for item in &scope.review_queue {
        let direct = item.functions.iter().cloned().collect::<BTreeSet<_>>();
        let optimistic = reverse_reachable(graph, &direct);
        let co_blockers = direct
            .iter()
            .flat_map(|function| by_function.get(function).into_iter().flatten())
            .filter(|id| *id != &item.id)
            .cloned()
            .collect();
        let guaranteed = direct
            .iter()
            .filter(|function| {
                by_function
                    .get(*function)
                    .is_some_and(|blockers| blockers.len() == 1 && blockers.contains(&item.id))
                    && graph
                        .outgoing
                        .get(*function)
                        .into_iter()
                        .flatten()
                        .all(|dependency| graph.nodes[dependency].complete)
            })
            .cloned()
            .collect();
        merge(
            candidates,
            Seed {
                id: item.id.clone(),
                kind: item.kind.clone(),
                severity: item.severity.clone(),
                message: item.message.clone(),
                guaranteed,
                optimistic,
                marginal: direct.clone(),
                direct,
                co_blockers,
                roots: item.affected_scope_roots.iter().cloned().collect(),
            },
            scope,
        );
    }
}

fn merge(candidates: &mut BTreeMap<String, Accumulator>, seed: Seed, scope: &ReviewScopeReport) {
    let item = candidates
        .entry(seed.id.clone())
        .or_insert_with(|| Accumulator {
            id: seed.id,
            kind: seed.kind,
            severity: seed.severity,
            message: seed.message,
            direct: BTreeSet::new(),
            guaranteed: BTreeSet::new(),
            optimistic: BTreeSet::new(),
            marginal: BTreeSet::new(),
            co_blockers: BTreeSet::new(),
            roots: BTreeSet::new(),
            scopes: BTreeSet::new(),
            publication_scopes: BTreeSet::new(),
        });
    item.direct.extend(seed.direct);
    item.guaranteed.extend(seed.guaranteed);
    item.optimistic.extend(seed.optimistic);
    item.marginal.extend(seed.marginal);
    item.co_blockers.extend(seed.co_blockers);
    item.roots.extend(seed.roots);
    item.scopes.insert(scope.id.clone());
    if scope.publication {
        item.publication_scopes.insert(scope.id.clone());
    }
}

fn add_registers(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(());
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let identities = load_effective_register_model(paths)?.register_identities()?;
    for fact in &facts.registers {
        if identities.contains_key(&(u64::from(fact.address), u32::from(fact.width))) {
            continue;
        }
        add_register_seed(
            scopes,
            graphs,
            candidates,
            fact,
            SeedTemplate {
                id: format!("register-{:#010x}-{}", fact.address, fact.width),
                kind: "register-model".to_owned(),
                severity: "warning".to_owned(),
                message: format!(
                    "name and review MMIO {:#010x}/{} before publication",
                    fact.address, fact.width
                ),
            },
        );
    }
    Ok(())
}

struct SeedTemplate {
    id: String,
    kind: String,
    severity: String,
    message: String,
}

fn add_register_seed(
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
    fact: &crate::registers::RegisterFact,
    template: SeedTemplate,
) {
    let function_keys = fact
        .read_functions
        .iter()
        .chain(&fact.write_functions)
        .collect::<BTreeSet<_>>();
    for scope in scopes {
        let graph = &graphs[&scope.id];
        let direct = function_keys
            .iter()
            .filter_map(|function| resolve_function(graph, function))
            .collect::<BTreeSet<_>>();
        if direct.is_empty() {
            continue;
        }
        merge(
            candidates,
            Seed {
                id: template.id.clone(),
                kind: template.kind.clone(),
                severity: template.severity.clone(),
                message: template.message.clone(),
                guaranteed: BTreeSet::new(),
                optimistic: reverse_reachable(graph, &direct),
                marginal: direct.clone(),
                direct,
                co_blockers: BTreeSet::new(),
                roots: BTreeSet::new(),
            },
            scope,
        );
    }
}

fn add_unknown_semantics(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let Some(paths) = session.project.registers.as_ref() else {
        return Ok(());
    };
    let facts = RegisterFacts::load(&paths.facts)?;
    let knowledge =
        open_radio_vendor_review::ReviewKnowledge::load_all(&session.project.reviewed_knowledge)
            .map_err(|error| {
                crate::Error::invalid(format!("cannot prioritize reviewed knowledge: {error}"))
            })?;
    for assertion in knowledge.assertions().values().filter(|assertion| {
        assertion.kind == "hardware-write-semantics"
            && matches!(&assertion.value, open_radio_vendor_review::AssertionValue::String(value) if value == "unknown")
    }) {
        let Some((address, width)) = parse_register(&assertion.subject) else {
            continue;
        };
        let Some(fact) = facts
            .registers
            .iter()
            .find(|fact| fact.address == address && fact.width == width)
        else {
            continue;
        };
        add_register_seed(
            scopes,
            graphs,
            candidates,
            fact,
            SeedTemplate {
                id: format!("semantic-{}", assertion.id),
                kind: "register-write-semantics".to_owned(),
                severity: "warning".to_owned(),
                message: format!(
                    "prove write semantics for {address:#010x}/{width}; software access cannot prove W1C/self-clear"
                ),
            },
        );
    }
    Ok(())
}

fn parse_register(subject: &str) -> Option<(u32, u8)> {
    let physical = subject.strip_prefix("mmio:cpu:")?.split('#').next()?;
    let (address, width) = physical.split_once('/')?;
    Some((
        u32::from_str_radix(address.strip_prefix("0x")?, 16).ok()?,
        width.parse().ok()?,
    ))
}

fn add_interfaces(
    session: &ProjectSession,
    scopes: &[&ReviewScopeReport],
    graphs: &BTreeMap<String, ScopeGraph>,
    candidates: &mut BTreeMap<String, Accumulator>,
) -> Result<()> {
    let Some(workspace) = session.interface_workspace()? else {
        return Ok(());
    };
    for observation in workspace.unreviewed_observations() {
        let id = stable_id(
            "interface",
            &format!(
                "{}:{}:{:+#x}/{}:{:?}",
                observation.source,
                observation.contract,
                observation.offset,
                observation.width,
                observation.selector
            ),
        );
        for scope in scopes {
            let graph = &graphs[&scope.id];
            let direct = observation
                .functions
                .iter()
                .filter_map(|function| resolve_function(graph, function))
                .collect::<BTreeSet<_>>();
            if direct.is_empty() {
                continue;
            }
            merge(
                candidates,
                Seed {
                    id: id.clone(),
                    kind: "interface-layout".to_owned(),
                    severity: "warning".to_owned(),
                    message: format!(
                        "review interface slot {} at {:+#x}/{}",
                        observation.contract, observation.offset, observation.width
                    ),
                    guaranteed: BTreeSet::new(),
                    optimistic: reverse_reachable(graph, &direct),
                    marginal: direct.clone(),
                    direct,
                    co_blockers: BTreeSet::new(),
                    roots: BTreeSet::new(),
                },
                scope,
            );
        }
    }
    Ok(())
}

fn stable_id(kind: &str, identity: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in kind
        .bytes()
        .chain(std::iter::once(0))
        .chain(identity.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("research-{hash:016x}")
}

fn attach_candidate_co_blockers(candidates: &mut BTreeMap<String, Accumulator>) {
    let mut by_function = BTreeMap::<(&'static str, String), BTreeSet<String>>::new();
    for candidate in candidates.values() {
        let domain = candidate_domain(&candidate.kind);
        for function in &candidate.direct {
            by_function
                .entry((domain, function.clone()))
                .or_default()
                .insert(candidate.id.clone());
        }
    }
    for candidate in candidates.values_mut() {
        let domain = candidate_domain(&candidate.kind);
        for function in &candidate.direct {
            candidate.co_blockers.extend(
                by_function[&(domain, function.clone())]
                    .iter()
                    .filter(|id| *id != &candidate.id)
                    .cloned(),
            );
        }
    }
}

fn candidate_domain(kind: &str) -> &'static str {
    match kind {
        "register-model" | "register-write-semantics" => "register",
        "interface-layout" => "interface",
        kind if kind.starts_with("replacement-") => "replacement",
        _ => "analysis",
    }
}

fn verification_surfaces(
    project: &crate::ProjectSpec,
) -> (BTreeMap<String, BTreeSet<String>>, Option<String>) {
    match crate::verification::policy::evaluate(project) {
        Ok(Some(report)) => {
            let mut by_scope = BTreeMap::<String, BTreeSet<String>>::new();
            for surface in report.surfaces {
                for scope in surface.review_scopes {
                    by_scope
                        .entry(scope)
                        .or_default()
                        .insert(surface.id.clone());
                }
            }
            (by_scope, None)
        }
        Ok(None) => (BTreeMap::new(), None),
        Err(error) => (BTreeMap::new(), Some(error.to_string())),
    }
}

fn finalize(
    candidate: Accumulator,
    surfaces_by_scope: &BTreeMap<String, BTreeSet<String>>,
) -> ResearchCandidate {
    let surfaces = candidate
        .scopes
        .iter()
        .flat_map(|scope| surfaces_by_scope.get(scope).into_iter().flatten())
        .cloned()
        .collect::<BTreeSet<_>>();
    let cost = cost_units(&candidate.kind, candidate.direct.len());
    let score_breakdown = ResearchScoreBreakdown {
        guaranteed_weight: candidate.guaranteed.len() as u64 * 20,
        optimistic_weight: candidate.optimistic.len() as u64 * 3,
        marginal_weight: candidate.marginal.len() as u64 * 5,
        root_weight: candidate.roots.len() as u64 * 10,
        verification_weight: surfaces.len() as u64 * 15,
        publication_weight: candidate.publication_scopes.len() as u64 * 20,
        cost_penalty: cost * 10,
        co_blocker_penalty: candidate.co_blockers.len() as u64 * 5,
    };
    let benefit = score_breakdown.guaranteed_weight
        + score_breakdown.optimistic_weight
        + score_breakdown.marginal_weight
        + score_breakdown.root_weight
        + score_breakdown.verification_weight
        + score_breakdown.publication_weight;
    let score = benefit.saturating_mul(100)
        / (score_breakdown.cost_penalty + score_breakdown.co_blocker_penalty + 1);
    let next_command = next_command(&candidate);
    ResearchCandidate {
        rank: 0,
        id: candidate.id,
        kind: candidate.kind.clone(),
        severity: candidate.severity,
        score,
        direct_functions: candidate.direct.len(),
        guaranteed_unlock: candidate.guaranteed.len(),
        optimistic_unlock: candidate.optimistic.len(),
        marginal_unlock_after_co_blockers: candidate.marginal.len(),
        co_blockers: candidate.co_blockers.len(),
        affected_scope_roots: candidate.roots.into_iter().collect(),
        scopes: candidate.scopes.into_iter().collect(),
        verification_surfaces: surfaces.into_iter().collect(),
        publication_scopes: candidate.publication_scopes.into_iter().collect(),
        estimated_cost: match cost {
            0..=2 => "low",
            3..=5 => "medium",
            _ => "high",
        }
        .to_owned(),
        confidence: confidence(&candidate.kind, candidate.co_blockers.len()).to_owned(),
        knowledge_required: knowledge_required(&candidate.kind).to_owned(),
        next_command,
        summary: candidate.message,
        related_findings: Vec::new(),
        score_breakdown,
    }
}

fn coalesce_actions(candidates: Vec<ResearchCandidate>) -> Vec<ResearchCandidate> {
    let mut actions = Vec::<ResearchCandidate>::new();
    let mut by_command = BTreeMap::<String, usize>::new();
    for candidate in candidates {
        if let Some(index) = by_command.get(&candidate.next_command).copied() {
            let action = &mut actions[index];
            action.related_findings.push(RelatedResearchFinding {
                id: candidate.id,
                kind: candidate.kind,
                knowledge_required: candidate.knowledge_required,
                summary: candidate.summary,
            });
            merge_strings(&mut action.scopes, candidate.scopes);
            merge_strings(
                &mut action.verification_surfaces,
                candidate.verification_surfaces,
            );
            merge_strings(&mut action.publication_scopes, candidate.publication_scopes);
        } else {
            by_command.insert(candidate.next_command.clone(), actions.len());
            actions.push(candidate);
        }
    }
    actions
}

fn merge_strings(target: &mut Vec<String>, source: Vec<String>) {
    target.extend(source);
    target.sort();
    target.dedup();
}

fn cost_units(kind: &str, functions: usize) -> u64 {
    let base = match kind {
        "unresolved-call" => 2,
        "call-result-model" | "call-shape" => 3,
        "interface-layout" | "register-model" => 4,
        "register-write-semantics" => 6,
        "decode" => 7,
        kind if kind.starts_with("replacement-") => 5,
        _ => 4,
    };
    base + u64::from(functions > 8) + u64::from(functions > 32)
}

fn confidence(kind: &str, co_blockers: usize) -> &'static str {
    if kind == "register-write-semantics" {
        "low-until-hil"
    } else if co_blockers == 0 {
        "high"
    } else {
        "medium"
    }
}

fn knowledge_required(kind: &str) -> &'static str {
    match kind {
        "decode" => "ISA/backend decode support",
        "unresolved-call" => "symbol/linkage identity",
        "call-result-model" => "function return/effect contract",
        "call-shape" | "indirect-control-flow" | "interface-layout" => {
            "ABI and interface-table layout"
        }
        "register-model" => "register name, access role and bit candidates",
        "register-write-semantics" => "hardware semantics backed by HIL or authoritative docs",
        kind if kind.starts_with("replacement-") => "production binding and verification evidence",
        "memory-load" | "memory-store" | "memory-intrinsic" => "memory-object/type layout",
        _ => "reviewed semantic model",
    }
}

fn next_command(candidate: &Accumulator) -> String {
    if let Some(address) = candidate.id.strip_prefix("register-") {
        return format!(
            "blobray inspect register {} --project <project>",
            address.split('-').next().unwrap_or(address)
        );
    }
    if candidate.kind == "register-write-semantics"
        && let Some(address) = candidate
            .message
            .split_whitespace()
            .find(|word| word.starts_with("0x"))
    {
        return format!(
            "blobray inspect register {} --project <project>",
            address.split('/').next().unwrap_or(address)
        );
    }
    if let Some(function) = candidate.direct.first() {
        let selector = function.split_once("::").map_or_else(
            || function.clone(),
            |(source, symbol)| format!("{source}:{symbol}"),
        );
        format!("blobray inspect function {selector} --project <project>")
    } else if let Some(scope) = candidate.scopes.first() {
        format!("blobray inspect scope {scope} --project <project>")
    } else {
        "blobray project status --project <project>".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_impact_reaches_callers_but_not_unrelated_functions() {
        let graph = ScopeGraph {
            nodes: ["root", "middle", "leaf", "other"]
                .into_iter()
                .map(|id| {
                    (
                        id.to_owned(),
                        GraphNode {
                            source: "vendor".to_owned(),
                            symbol: id.to_owned(),
                            dependencies: BTreeSet::new(),
                            complete: false,
                        },
                    )
                })
                .collect(),
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::from([
                ("leaf".to_owned(), ["middle".to_owned()].into()),
                ("middle".to_owned(), ["root".to_owned()].into()),
            ]),
        };
        assert_eq!(
            reverse_reachable(&graph, &["leaf".to_owned()].into()),
            ["leaf", "middle", "root"].map(str::to_owned).into()
        );
    }

    #[test]
    fn score_exposes_benefit_and_cost_terms() {
        let candidate = Accumulator {
            id: "candidate".to_owned(),
            kind: "unresolved-call".to_owned(),
            severity: "error".to_owned(),
            message: "resolve call".to_owned(),
            direct: ["leaf".to_owned()].into(),
            guaranteed: ["leaf".to_owned()].into(),
            optimistic: ["leaf".to_owned(), "root".to_owned()].into(),
            marginal: ["leaf".to_owned()].into(),
            co_blockers: BTreeSet::new(),
            roots: ["root".to_owned()].into(),
            scopes: ["radio".to_owned()].into(),
            publication_scopes: ["radio".to_owned()].into(),
        };
        let result = finalize(
            candidate,
            &BTreeMap::from([("radio".to_owned(), ["surface".to_owned()].into())]),
        );
        assert_eq!(result.guaranteed_unlock, 1);
        assert_eq!(result.optimistic_unlock, 2);
        assert_eq!(result.score_breakdown.cost_penalty, 20);
        assert!(result.score > 100);
        assert_eq!(
            result.next_command,
            "blobray inspect function leaf --project <project>"
        );
    }

    #[test]
    fn one_user_action_keeps_all_related_findings_without_duplicate_ranks() {
        fn candidate(id: &str, message: &str) -> Accumulator {
            Accumulator {
                id: id.to_owned(),
                kind: "interface-layout".to_owned(),
                severity: "warning".to_owned(),
                message: message.to_owned(),
                direct: ["ble-controller::logger".to_owned()].into(),
                guaranteed: BTreeSet::new(),
                optimistic: ["ble-controller::logger".to_owned()].into(),
                marginal: ["ble-controller::logger".to_owned()].into(),
                co_blockers: BTreeSet::new(),
                roots: BTreeSet::new(),
                scopes: ["ble".to_owned()].into(),
                publication_scopes: ["ble".to_owned()].into(),
            }
        }

        let actions = coalesce_actions(vec![
            finalize(candidate("slot-a", "review slot A"), &BTreeMap::new()),
            finalize(candidate("slot-b", "review slot B"), &BTreeMap::new()),
        ]);

        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "slot-a");
        assert_eq!(actions[0].related_findings.len(), 1);
        assert_eq!(actions[0].related_findings[0].id, "slot-b");
        assert_eq!(actions[0].related_findings[0].summary, "review slot B");
    }

    #[test]
    fn next_command_converts_internal_identity_to_cli_selector() {
        let candidate = Accumulator {
            id: "candidate".to_owned(),
            kind: "unresolved-call".to_owned(),
            severity: "error".to_owned(),
            message: "resolve call".to_owned(),
            direct: ["archive::function".to_owned()].into(),
            guaranteed: BTreeSet::new(),
            optimistic: BTreeSet::new(),
            marginal: BTreeSet::new(),
            co_blockers: BTreeSet::new(),
            roots: BTreeSet::new(),
            scopes: BTreeSet::new(),
            publication_scopes: BTreeSet::new(),
        };
        assert_eq!(
            next_command(&candidate),
            "blobray inspect function archive:function --project <project>"
        );
    }

    #[test]
    fn overlapping_candidates_are_co_blockers_only_within_one_domain() {
        fn candidate(id: &str, kind: &str) -> Accumulator {
            Accumulator {
                id: id.to_owned(),
                kind: kind.to_owned(),
                severity: "warning".to_owned(),
                message: "review".to_owned(),
                direct: ["function".to_owned()].into(),
                guaranteed: BTreeSet::new(),
                optimistic: BTreeSet::new(),
                marginal: BTreeSet::new(),
                co_blockers: BTreeSet::new(),
                roots: BTreeSet::new(),
                scopes: BTreeSet::new(),
                publication_scopes: BTreeSet::new(),
            }
        }

        let mut candidates = BTreeMap::from([
            (
                "interface-a".to_owned(),
                candidate("interface-a", "interface-layout"),
            ),
            (
                "interface-b".to_owned(),
                candidate("interface-b", "interface-layout"),
            ),
            (
                "register".to_owned(),
                candidate("register", "register-model"),
            ),
        ]);
        attach_candidate_co_blockers(&mut candidates);
        assert_eq!(
            candidates["interface-a"].co_blockers,
            ["interface-b".to_owned()].into()
        );
        assert_eq!(
            candidates["interface-b"].co_blockers,
            ["interface-a".to_owned()].into()
        );
        assert!(candidates["register"].co_blockers.is_empty());
    }
}
