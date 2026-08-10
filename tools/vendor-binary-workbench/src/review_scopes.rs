//! Project-owned, reproducible review surfaces over artifact-wide linked IR.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
};

use crate::{
    ProjectSpec, Result, artifacts::parse_linked_ir, project::ReviewScopeSpec,
    registers::RegisterFacts,
};

pub(crate) const REVIEW_SCOPES_SCHEMA: u32 = 2;

mod model;
pub(crate) use model::{ReviewScopeMmio, ReviewScopeReport, ReviewScopesDocument};
use model::{StoredReplacement, VerificationDocument};

impl ReviewScopesDocument {
    pub(crate) fn release_mmio(&self) -> BTreeSet<(u32, u8)> {
        self.scopes
            .iter()
            .filter(|scope| scope.release)
            .flat_map(|scope| scope.mmio.iter().map(|mmio| (mmio.address, mmio.width)))
            .collect()
    }
}

#[derive(Debug)]
struct FunctionNode {
    source: String,
    symbol: String,
    dependencies: Vec<String>,
    mmio: Vec<(u32, u8)>,
    table_calls: usize,
    context_fields: usize,
    memory_fields: usize,
    decode_blockers: usize,
    direct_blockers: usize,
    call_graph_blockers: usize,
    reference_blockers: usize,
    unresolved_calls: usize,
    complete: bool,
}

pub(crate) fn analyze(project: &ProjectSpec) -> Result<Vec<ReviewScopeReport>> {
    let Some(workspace) = &project.review else {
        return Ok(Vec::new());
    };
    let replacements = load_replacements(project)?;
    let register_facts = project
        .registers
        .as_ref()
        .filter(|paths| paths.facts.is_file())
        .map(|paths| RegisterFacts::load(&paths.facts))
        .transpose()?;
    workspace
        .scopes
        .iter()
        .map(|scope| {
            analyze_scope(
                project,
                scope,
                workspace.release_scopes.contains(&scope.id),
                &replacements,
                register_facts.as_ref(),
            )
        })
        .collect()
}

pub(crate) fn build_document(project: &ProjectSpec) -> Result<ReviewScopesDocument> {
    Ok(ReviewScopesDocument {
        schema_version: REVIEW_SCOPES_SCHEMA,
        command: "project review scopes".to_owned(),
        project: project.id.clone(),
        scopes: analyze(project)?,
    })
}

pub(crate) fn render_document(document: &ReviewScopesDocument) -> Result<String> {
    Ok(serde_json::to_string_pretty(document)? + "\n")
}

pub(crate) fn parse_document(input: &str) -> Result<ReviewScopesDocument> {
    let document: ReviewScopesDocument = serde_json::from_str(input)?;
    if document.schema_version != REVIEW_SCOPES_SCHEMA
        || document.command != "project review scopes"
    {
        return Err(crate::Error::invalid(format!(
            "expected project review scopes schema {REVIEW_SCOPES_SCHEMA}"
        )));
    }
    Ok(document)
}

pub(crate) fn load(path: &std::path::Path) -> Result<ReviewScopesDocument> {
    let input = fs::read_to_string(path).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot read review scope report {}: {error}",
            path.display()
        ))
    })?;
    parse_document(&input)
}

pub(crate) fn load_for_project(project: &ProjectSpec) -> Result<ReviewScopesDocument> {
    let workspace = project
        .review
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[review] is required for project publication"))?;
    let document = load(&workspace.output)?;
    if document.project != project.id {
        return Err(crate::Error::invalid(format!(
            "review scope report belongs to project {:?}, expected {:?}",
            document.project, project.id
        )));
    }
    let actual = document
        .scopes
        .iter()
        .map(|scope| (scope.id.as_str(), scope.profiles.as_slice(), scope.release))
        .collect::<Vec<_>>();
    let expected = workspace
        .scopes
        .iter()
        .map(|scope| {
            (
                scope.id.as_str(),
                scope.profiles.as_slice(),
                workspace.release_scopes.contains(&scope.id),
            )
        })
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(crate::Error::invalid(format!(
            "review scope report {} is stale for the configured [review] scopes; run project analyze",
            workspace.output.display()
        )));
    }
    Ok(document)
}

fn analyze_scope(
    project: &ProjectSpec,
    scope: &ReviewScopeSpec,
    release: bool,
    replacements: &[StoredReplacement],
    register_facts: Option<&RegisterFacts>,
) -> Result<ReviewScopeReport> {
    let mut nodes = BTreeMap::<String, FunctionNode>::new();
    for profile_id in &scope.profiles {
        let profile = project
            .ir_profiles
            .iter()
            .find(|profile| profile.id == *profile_id)
            .ok_or_else(|| {
                crate::Error::invalid(format!("unknown review profile {profile_id:?}"))
            })?;
        let input = fs::read_to_string(&profile.output).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot read review scope IR {}: {error}",
                profile.output.display()
            ))
        })?;
        let document = parse_linked_ir(&input)?;
        for function in document.functions {
            let mut dependencies = function.dependencies().to_vec();
            dependencies.extend(
                function
                    .calls
                    .iter()
                    .filter(|call| matches!(call.kind.as_str(), "internal" | "project-linked"))
                    .map(|call| call.target.clone()),
            );
            dependencies.sort();
            dependencies.dedup();
            let unresolved_calls = function
                .calls
                .iter()
                .filter(|call| matches!(call.kind.as_str(), "unresolved" | "ambiguous-project"))
                .count();
            let table_calls = function
                .calls
                .iter()
                .filter(|call| {
                    !matches!(
                        call.kind.as_str(),
                        "internal" | "project-linked" | "unresolved" | "ambiguous-project"
                    )
                })
                .count();
            let context_fields = function.context_field_count();
            let memory_fields = function.memory_field_count();
            let direct_blockers = function.direct_blocker_count();
            let call_graph_blockers = function.call_graph_blocker_count();
            let reference_blockers = function.reference_blocker_count();
            let node = FunctionNode {
                source: function.source,
                symbol: function.symbol,
                dependencies,
                mmio: function
                    .mmio_accesses
                    .iter()
                    .map(|access| (access.address, access.width()))
                    .collect(),
                table_calls,
                context_fields,
                memory_fields,
                decode_blockers: function.decode_blockers.len(),
                direct_blockers,
                call_graph_blockers,
                reference_blockers,
                unresolved_calls,
                complete: function.complete,
            };
            if nodes.insert(function.identity.clone(), node).is_some() {
                return Err(crate::Error::invalid(format!(
                    "review scope {:?} loads duplicate function identity {:?}",
                    scope.id, function.identity
                )));
            }
        }
    }

    let mut selected = BTreeSet::new();
    let mut queue = VecDeque::new();
    for root in &scope.roots {
        let identity = resolve_root(root, &nodes)?;
        if selected.insert(identity.clone()) {
            queue.push_back(identity);
        }
    }
    if scope.include_reachable {
        while let Some(identity) = queue.pop_front() {
            let node = &nodes[&identity];
            for dependency in &node.dependencies {
                if let Some(target) = resolve_dependency(dependency, &nodes)
                    && selected.insert(target.clone())
                {
                    queue.push_back(target);
                }
            }
        }
    }

    let mut mmio = BTreeMap::<(u32, u8), (bool, bool)>::new();
    let mut vendors = BTreeSet::<(String, String)>::new();
    let function_identities = selected.iter().cloned().collect::<Vec<_>>();
    let function_keys = selected
        .iter()
        .map(|identity| {
            let node = &nodes[identity];
            format!("{}:{}", node.source, node.symbol)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut report = ReviewScopeReport {
        id: scope.id.clone(),
        release,
        profiles: scope.profiles.clone(),
        roots: scope.roots.len(),
        functions: selected.len(),
        function_identities,
        function_keys: function_keys.clone(),
        complete_functions: 0,
        mmio_registers: 0,
        linked_mmio_registers: 0,
        static_mmio_registers: 0,
        mmio: Vec::new(),
        table_calls: 0,
        context_fields: 0,
        memory_fields: 0,
        decode_blockers: 0,
        decode_blocker_functions: 0,
        direct_blockers: 0,
        call_graph_blockers: 0,
        reference_blockers: 0,
        unresolved_calls: 0,
        replacement_behavioral_matches: 0,
        replacement_production_matches: 0,
        replacement_probe_only_matches: 0,
        replacement_unmapped_matches: 0,
        replacement_mismatches: 0,
        replacement_incomplete: 0,
        replacement_unqualified: 0,
        replacement_uncovered: 0,
    };
    for identity in selected {
        let node = &nodes[&identity];
        for key in &node.mmio {
            mmio.entry(*key).or_default().0 = true;
        }
        vendors.insert((node.source.clone(), node.symbol.clone()));
        report.complete_functions += usize::from(node.complete);
        report.table_calls += node.table_calls;
        report.context_fields += node.context_fields;
        report.memory_fields += node.memory_fields;
        report.decode_blockers += node.decode_blockers;
        report.decode_blocker_functions += usize::from(node.decode_blockers != 0);
        report.direct_blockers += node.direct_blockers;
        report.call_graph_blockers += node.call_graph_blockers;
        report.reference_blockers += node.reference_blockers;
        report.unresolved_calls += node.unresolved_calls;
    }
    if let Some(register_facts) = register_facts {
        let function_keys = function_keys.iter().collect::<BTreeSet<_>>();
        for fact in &register_facts.registers {
            let used = fact
                .read_functions
                .iter()
                .chain(&fact.write_functions)
                .any(|function| function_keys.contains(function));
            if used {
                mmio.entry((fact.address, fact.width)).or_default().1 = true;
            }
        }
    }
    report.linked_mmio_registers = mmio.values().filter(|(linked, _)| *linked).count();
    report.static_mmio_registers = mmio
        .values()
        .filter(|(_, static_fact)| *static_fact)
        .count();
    report.mmio_registers = mmio.len();
    report.mmio = mmio
        .into_iter()
        .map(
            |((address, width), (linked_ir, static_discovery))| ReviewScopeMmio {
                address,
                width,
                linked_ir,
                static_discovery,
            },
        )
        .collect();
    for (source, symbol) in &vendors {
        let Some(replacement) = replacements.iter().find(|replacement| {
            replacement.vendor.source == *source && replacement.vendor.symbol == *symbol
        }) else {
            report.replacement_uncovered += 1;
            continue;
        };
        match replacement.status.as_str() {
            "match" => {
                report.replacement_behavioral_matches += 1;
                match replacement.rust.as_ref() {
                    Some(rust) if rust.production_component.is_some() => {
                        report.replacement_production_matches += 1;
                    }
                    Some(rust) if !rust.verification_probes.is_empty() => {
                        report.replacement_probe_only_matches += 1;
                    }
                    _ => {
                        report.replacement_unmapped_matches += 1;
                    }
                }
            }
            "mismatch" => report.replacement_mismatches += 1,
            "incomplete" => report.replacement_incomplete += 1,
            "implemented-unqualified" => report.replacement_unqualified += 1,
            "uncovered" => report.replacement_uncovered += 1,
            status => {
                return Err(crate::Error::invalid(format!(
                    "unknown replacement status {status:?}"
                )));
            }
        }
    }
    Ok(report)
}

fn resolve_root(root: &str, nodes: &BTreeMap<String, FunctionNode>) -> Result<String> {
    if nodes.contains_key(root) {
        return Ok(root.to_owned());
    }
    let normalized = root
        .split_once(':')
        .filter(|(source, symbol)| !source.is_empty() && !symbol.starts_with(':'))
        .map_or_else(
            || root.to_owned(),
            |(source, symbol)| format!("{source}::{symbol}"),
        );
    let matches = nodes
        .iter()
        .filter(|(identity, node)| {
            identity.as_str() == normalized
                || format!("{}::{}", node.source, node.symbol) == normalized
        })
        .map(|(identity, _)| identity.clone())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [identity] => Ok(identity.clone()),
        [] => Err(crate::Error::invalid(format!(
            "review root {root:?} does not identify a function in the selected profiles"
        ))),
        _ => Err(crate::Error::invalid(format!(
            "review root {root:?} is ambiguous; use one of: {}",
            matches.join(", ")
        ))),
    }
}

fn resolve_dependency(dependency: &str, nodes: &BTreeMap<String, FunctionNode>) -> Option<String> {
    if nodes.contains_key(dependency) {
        return Some(dependency.to_owned());
    }
    let mut matches = nodes
        .keys()
        .filter(|identity| identity.starts_with(&format!("{dependency}@0x")));
    let first = matches.next()?.clone();
    matches.next().is_none().then_some(first)
}

fn load_replacements(project: &ProjectSpec) -> Result<Vec<StoredReplacement>> {
    let Some(workspace) = &project.verification else {
        return Ok(Vec::new());
    };
    if !workspace.report.is_file() {
        return Ok(Vec::new());
    }
    let input = fs::read_to_string(&workspace.report).map_err(|error| {
        crate::Error::invalid(format!(
            "cannot read project verification report {}: {error}",
            workspace.report.display()
        ))
    })?;
    let document: VerificationDocument = serde_json::from_str(&input)?;
    if document.schema_version != crate::verification::PROJECT_VERIFICATION_REPORT_SCHEMA
        || document.command != "project verify"
    {
        return Err(crate::Error::invalid(format!(
            "{} is not a project verification report schema {}",
            workspace.report.display(),
            crate::verification::PROJECT_VERIFICATION_REPORT_SCHEMA,
        )));
    }
    Ok(document.replacement_graph.replacements)
}
