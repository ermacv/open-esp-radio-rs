//! Project-owned, reproducible review surfaces over artifact-wide linked IR.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
};

use crate::{
    ProjectSpec, Result, artifacts::parse_linked_ir, project::ReviewScopeSpec,
    registers::RegisterFacts,
};

pub(crate) const REVIEW_SCOPES_SCHEMA: u32 = 3;

mod model;
pub(crate) use model::{ReviewScopeMmio, ReviewScopeReport, ReviewScopesDocument};
use model::{StoredReplacement, VerificationDocument};
mod queue;

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
    decode_blockers: Vec<DecodeIssue>,
    diagnostics: Vec<DiagnosticIssue>,
    unresolved_call_sites: Vec<CallIssue>,
    direct_blockers: usize,
    call_graph_blockers: usize,
    reference_blockers: usize,
    unresolved_calls: usize,
    complete: bool,
}

#[derive(Debug)]
struct DecodeIssue {
    address: u32,
    class: String,
}

#[derive(Debug)]
struct DiagnosticIssue {
    root_id: String,
    kind: String,
    site: Option<u32>,
    channel: &'static str,
    message: String,
}

#[derive(Debug)]
struct CallIssue {
    site: Option<u32>,
    target: String,
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
    let mut documents = Vec::with_capacity(scope.profiles.len());
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
        documents.push(parse_linked_ir(&input)?);
    }
    let project_definitions = project_definitions(&documents);
    let mut nodes = BTreeMap::<String, FunctionNode>::new();
    for document in documents {
        for function in document.functions {
            let mut dependencies = function.dependencies().to_vec();
            dependencies.extend(function.calls.iter().filter_map(|call| {
                effective_call_target(call, &project_definitions).map(ToOwned::to_owned)
            }));
            dependencies.sort();
            dependencies.dedup();
            let unresolved_call_sites = function
                .calls
                .iter()
                .filter(|call| call_is_unresolved(call, &project_definitions))
                .map(|call| CallIssue {
                    site: call.site,
                    target: call.target.clone(),
                })
                .collect::<Vec<_>>();
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
            let call_sites = function
                .calls
                .iter()
                .filter_map(|call| call.site)
                .collect::<BTreeSet<_>>();
            let diagnostics = function
                .diagnostics()
                .filter(|(_, diagnostic)| {
                    diagnostic.kind != "aggregate"
                        && diagnostic.kind != "unresolved-call"
                        && !(diagnostic.kind == "call-boundary"
                            && diagnostic
                                .site
                                .is_some_and(|site| call_sites.contains(&site)))
                })
                .map(|(channel, diagnostic)| DiagnosticIssue {
                    root_id: diagnostic.root_id.clone(),
                    kind: diagnostic.kind.clone(),
                    site: diagnostic.site,
                    channel,
                    message: diagnostic.rendered.clone(),
                })
                .collect::<Vec<_>>();
            let direct_blockers = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.channel == "direct")
                .count();
            let call_graph_blockers = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.channel == "call-graph")
                .count();
            let reference_blockers = diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.channel == "reference")
                .count();
            let decode_blockers = function
                .decode_blockers
                .iter()
                .map(|blocker| DecodeIssue {
                    address: blocker.address as u32,
                    class: blocker.class.clone(),
                })
                .collect::<Vec<_>>();
            let complete = function.complete
                || (decode_blockers.is_empty()
                    && diagnostics.is_empty()
                    && unresolved_call_sites.is_empty());
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
                decode_blockers,
                diagnostics,
                unresolved_call_sites,
                direct_blockers,
                call_graph_blockers,
                reference_blockers,
                unresolved_calls: function
                    .calls
                    .iter()
                    .filter(|call| call_is_unresolved(call, &project_definitions))
                    .count(),
                complete,
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
        review_queue: Vec::new(),
    };
    let mut review_queue = queue::new();
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
        report.decode_blockers += node.decode_blockers.len();
        report.decode_blocker_functions += usize::from(!node.decode_blockers.is_empty());
        report.direct_blockers += node.direct_blockers;
        report.call_graph_blockers += node.call_graph_blockers;
        report.reference_blockers += node.reference_blockers;
        report.unresolved_calls += node.unresolved_calls;
        for issue in &node.diagnostics {
            queue::insert(
                &mut review_queue,
                issue.root_id.clone(),
                &issue.kind,
                &identity,
                issue.site,
                issue.channel,
                issue.message.clone(),
            );
        }
        for issue in &node.decode_blockers {
            let key = format!("{}:{:#x}", issue.class, issue.address);
            queue::insert(
                &mut review_queue,
                queue::id("decode", &key),
                "decode",
                &identity,
                Some(issue.address),
                "decode",
                format!("unsupported instruction class {}", issue.class),
            );
        }
        for issue in &node.unresolved_call_sites {
            let key = issue.target.clone();
            queue::insert(
                &mut review_queue,
                queue::id("unresolved-call", &key),
                "unresolved-call",
                &identity,
                issue.site,
                "call",
                format!("unresolved call to {}", issue.target),
            );
        }
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
        let function = format!("{source}::{symbol}");
        let Some(replacement) = replacements.iter().find(|replacement| {
            replacement.vendor.source == *source && replacement.vendor.symbol == *symbol
        }) else {
            report.replacement_uncovered += 1;
            queue::insert(
                &mut review_queue,
                queue::id("replacement-uncovered", &function),
                "replacement-uncovered",
                &function,
                None,
                "replacement",
                "vendor function has no reviewed Rust replacement".to_owned(),
            );
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
                        queue::insert(
                            &mut review_queue,
                            queue::id("replacement-probe-only", &function),
                            "replacement-probe-only",
                            &function,
                            None,
                            "replacement",
                            "behavioral match is bound only to a verification probe".to_owned(),
                        );
                    }
                    _ => {
                        report.replacement_unmapped_matches += 1;
                        queue::insert(
                            &mut review_queue,
                            queue::id("replacement-unmapped", &function),
                            "replacement-unmapped",
                            &function,
                            None,
                            "replacement",
                            "behavioral match has no Rust component binding".to_owned(),
                        );
                    }
                }
            }
            "mismatch" | "incomplete" | "implemented-unqualified" | "uncovered" => {
                match replacement.status.as_str() {
                    "mismatch" => report.replacement_mismatches += 1,
                    "incomplete" => report.replacement_incomplete += 1,
                    "implemented-unqualified" => report.replacement_unqualified += 1,
                    "uncovered" => report.replacement_uncovered += 1,
                    _ => unreachable!(),
                }
                let kind = format!("replacement-{}", replacement.status);
                queue::insert(
                    &mut review_queue,
                    queue::id(&kind, &function),
                    &kind,
                    &function,
                    None,
                    "replacement",
                    format!("replacement status is {}", replacement.status),
                );
            }
            status => {
                return Err(crate::Error::invalid(format!(
                    "unknown replacement status {status:?}"
                )));
            }
        }
    }
    report.review_queue = queue::finish(review_queue);
    Ok(report)
}

fn project_definitions(
    documents: &[crate::artifacts::LinkedIrStoredDocument],
) -> BTreeMap<String, Vec<String>> {
    let mut definitions = BTreeMap::<String, BTreeSet<String>>::new();
    for function in documents
        .iter()
        .flat_map(|document| document.functions.iter())
        .filter(|function| function.is_exported())
    {
        definitions
            .entry(function.symbol.clone())
            .or_default()
            .insert(function.identity.clone());
    }
    definitions
        .into_iter()
        .map(|(symbol, identities)| (symbol, identities.into_iter().collect()))
        .collect()
}

fn effective_call_target<'a>(
    call: &'a crate::artifacts::StoredCall,
    definitions: &'a BTreeMap<String, Vec<String>>,
) -> Option<&'a str> {
    linked_call_target(&call.kind, &call.target, call.project_symbol(), definitions)
}

fn linked_call_target<'a>(
    kind: &str,
    target: &'a str,
    project_symbol: Option<&str>,
    definitions: &'a BTreeMap<String, Vec<String>>,
) -> Option<&'a str> {
    if matches!(kind, "internal" | "project-linked") {
        return Some(target);
    }
    let candidates = project_symbol.and_then(|symbol| definitions.get(symbol))?;
    match candidates.as_slice() {
        [target] => Some(target),
        _ => None,
    }
}

fn call_is_unresolved(
    call: &crate::artifacts::StoredCall,
    definitions: &BTreeMap<String, Vec<String>>,
) -> bool {
    matches!(call.kind.as_str(), "unresolved" | "ambiguous-project")
        && effective_call_target(call, definitions).is_none()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_exported_definition_links_an_unresolved_cross_profile_call() {
        let definitions = BTreeMap::from([(
            "external_call".to_owned(),
            vec!["provider::external_call".to_owned()],
        )]);

        assert_eq!(
            linked_call_target(
                "unresolved",
                "external_call",
                Some("external_call"),
                &definitions,
            ),
            Some("provider::external_call")
        );
    }

    #[test]
    fn ambiguous_exported_definitions_remain_unresolved() {
        let definitions = BTreeMap::from([(
            "external_call".to_owned(),
            vec![
                "first::external_call".to_owned(),
                "second::external_call".to_owned(),
            ],
        )]);

        assert_eq!(
            linked_call_target(
                "unresolved",
                "external_call",
                Some("external_call"),
                &definitions,
            ),
            None
        );
    }

    #[test]
    fn internal_calls_keep_their_authoritative_target() {
        assert_eq!(
            linked_call_target("internal", "consumer::child", None, &BTreeMap::new(),),
            Some("consumer::child")
        );
    }
}
