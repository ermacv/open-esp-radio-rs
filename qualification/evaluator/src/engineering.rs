//! Read-only engineering views of canonical declarations and saved evidence.
//!
//! Selection follows capability dependencies for context, not execution or
//! change impact. No view runs tests, builds firmware or promotes evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    Result,
    model::{
        Axis, CapabilityDocument, CatalogView, Development, Qualification, SourceContract, WorkKind,
    },
};

mod actions;
mod render;

#[derive(Debug, Serialize)]
pub(crate) struct ProjectMap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) observer_configuration_problem: Option<String>,
    pub(crate) schema: u16,
    pub(crate) mode: &'static str,
    pub(crate) target: Option<String>,
    pub(crate) focus: Option<String>,
    pub(crate) repository_commit: Option<String>,
    pub(crate) repository_dirty: Option<bool>,
    pub(crate) sources: Vec<Input>,
    pub(crate) research_projects: BTreeSet<PathBuf>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) next: Vec<Action>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Input {
    id: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) implementation: String,
    pub(crate) level: Option<String>,
    pub(crate) limits: Vec<String>,
    pub(crate) documentation_base: Option<PathBuf>,
    pub(crate) owners: BTreeSet<PathBuf>,
    pub(crate) source_contracts: Vec<SourceContract>,
    pub(crate) source_facts: Vec<String>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) knowledge: BTreeSet<PathBuf>,
    pub(crate) knowledge_status: &'static str,
    pub(crate) development: Development,
    pub(crate) host_declaration: Option<String>,
    pub(crate) async_declaration: Option<String>,
    pub(crate) vendor_roots: Vec<VendorRoot>,
    pub(crate) vendor_evidence: Vec<VendorReference>,
    pub(crate) hil_requirements: Vec<HilRequirement>,
    pub(crate) hil_reviews: Vec<PathBuf>,
    pub(crate) not_applicable: BTreeMap<&'static str, String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) evidence: Option<Evidence>,
}

#[derive(Debug, Serialize)]
pub(crate) struct VendorRoot {
    source: String,
    symbol: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct VendorReference {
    suite: String,
    source: String,
    symbol: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct HilRequirement {
    pub(crate) scenario: String,
    pub(crate) checks: Vec<String>,
    pub(crate) minimum_repetitions: u8,
}

#[derive(Debug, Serialize)]
pub(crate) struct Gap {
    pub(crate) axis: &'static str,
    pub(crate) id: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct Evidence {
    pub(crate) vendor: &'static str,
    pub(crate) hil: &'static str,
    pub(crate) references: Vec<String>,
    pub(crate) hil_decisions: Vec<crate::hil::EvidenceDecision>,
    pub(crate) hil_observations: crate::hil::ObservationCounts,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Action {
    pub(crate) entry: String,
    pub(crate) entry_kind: &'static str,
    pub(crate) kind: WorkKind,
    pub(crate) subject: String,
    pub(crate) reason: String,
    pub(crate) origin: &'static str,
}

impl Entry {
    fn new(id: &str, kind: &'static str, title: &str, scope: &str, implementation: &str) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            scope: scope.into(),
            implementation: implementation.into(),
            level: None,
            limits: Vec::new(),
            documentation_base: None,
            owners: BTreeSet::new(),
            source_contracts: Vec::new(),
            source_facts: Vec::new(),
            dependencies: Vec::new(),
            knowledge: BTreeSet::new(),
            knowledge_status: "not-linked",
            development: Development::default(),
            host_declaration: None,
            async_declaration: None,
            vendor_roots: Vec::new(),
            vendor_evidence: Vec::new(),
            hil_requirements: Vec::new(),
            hil_reviews: Vec::new(),
            not_applicable: BTreeMap::new(),
            gaps: Vec::new(),
            evidence: None,
        }
    }

    fn contracts(&mut self, contracts: &[SourceContract]) {
        self.source_contracts = contracts.to_vec();
        for contract in contracts {
            self.owners.extend(contract.source_paths.iter().cloned());
            self.limits.push(contract.limits.clone());
        }
    }
}

impl ProjectMap {
    pub(crate) fn from_catalog(catalog: &CatalogView, focus: Option<&str>) -> Result<Self> {
        Self::build(catalog, &catalog.capabilities, None, focus)
    }

    pub(crate) fn from_program(program: &Qualification, focus: Option<&str>) -> Result<Self> {
        Self::build(
            &program.catalog,
            &program.declarations,
            Some(program),
            focus,
        )
    }

    fn build(
        catalog: &CatalogView,
        declarations: &BTreeMap<String, CapabilityDocument>,
        program: Option<&Qualification>,
        focus: Option<&str>,
    ) -> Result<Self> {
        let selected = select(declarations, focus)?;
        let mut map = Self {
            observer_configuration_problem: program
                .and_then(|p| p.evidence_inputs.hil.observer_configuration_problem.clone()),
            schema: 1,
            mode: if program.is_some() {
                "saved-evidence"
            } else {
                "declarations-only"
            },
            target: program.map(|p| p.target.clone()),
            focus: focus.map(str::to_owned),
            repository_commit: program.map(|p| p.repository.commit.clone()),
            repository_dirty: program.map(|p| p.repository.dirty),
            sources: catalog
                .sources
                .iter()
                .map(|s| Input {
                    id: s.id.clone(),
                    path: s.path.clone(),
                    sha256: s.sha256.clone(),
                })
                .collect(),
            research_projects: catalog.research_projects.clone(),
            entries: Vec::new(),
            next: Vec::new(),
        };
        if let Some(program) = program {
            let s = &program.program_source;
            map.sources.push(Input {
                id: s.id.clone(),
                path: s.path.clone(),
                sha256: s.sha256.clone(),
            });
            map.research_projects
                .insert(program.evidence_inputs.verification_project.clone());
        }
        let mut fact_ids = BTreeSet::new();
        for id in selected {
            let document = &declarations[&id];
            fact_ids.extend(document.source_fact_refs.iter().cloned());
            let mut entry = Entry::new(
                &id,
                "capability",
                &document.title,
                &document.scope,
                document.implementation.label(),
            );
            entry.contracts(&document.source_contracts);
            entry.source_facts = document.source_fact_refs.clone();
            entry.dependencies = document.depends_on.clone();
            entry.host_declaration = Some(document.host.label().into());
            entry.async_declaration = Some(document.async_proof.label().into());
            entry.development = document.development.clone();
            entry.knowledge.extend(
                document
                    .development
                    .knowledge
                    .iter()
                    .chain(&document.vendor_anchors)
                    .cloned(),
            );
            if !entry.knowledge.is_empty() {
                entry.knowledge_status = "linked-not-assessed";
            }
            if let Some(scope) = &document.catalog_scope {
                entry.level = Some(scope.level.label().into());
                entry.limits.push(scope.limitations.clone());
            }
            entry.vendor_roots = document
                .vendor_roots
                .iter()
                .map(|r| VendorRoot {
                    source: r.source.clone(),
                    symbol: r.symbol.clone(),
                })
                .collect();
            entry.vendor_evidence = document
                .vendor_evidence
                .iter()
                .map(|r| VendorReference {
                    suite: r.suite.clone(),
                    source: r.source.clone(),
                    symbol: r.symbol.clone(),
                })
                .collect();
            entry.hil_reviews = document.hil_reviews.clone();
            entry.hil_requirements = document
                .hil_requirements
                .iter()
                .map(|r| HilRequirement {
                    scenario: r.scenario.clone(),
                    checks: r.checks.clone(),
                    minimum_repetitions: r.minimum_repetitions,
                })
                .collect();
            for (axis, reason) in [
                ("vendor", &document.vendor_not_applicable),
                ("hil", &document.hil_not_applicable),
                ("async", &document.async_not_applicable),
            ] {
                if let Some(reason) = reason {
                    entry.not_applicable.insert(axis, reason.clone());
                }
            }
            entry.gaps = document
                .gaps
                .iter()
                .map(|g| Gap {
                    axis: g.axis.label(),
                    id: g.id.clone(),
                })
                .collect();
            if let Some(capability) = program.and_then(|p| p.capabilities.get(&id)) {
                entry.evidence = Some(Evidence {
                    vendor: capability.vendor.label(),
                    hil: capability.hil.label(),
                    references: capability.evidence.clone(),
                    hil_decisions: capability.hil_decisions.clone(),
                    hil_observations: capability.hil_decisions.iter().fold(
                        crate::hil::ObservationCounts::default(),
                        |mut total, decision| {
                            let count = decision.observation_counts();
                            total.total += count.total;
                            total.passed += count.passed;
                            total.failed += count.failed;
                            total.excluded += count.excluded;
                            total
                        },
                    ),
                });
                entry.gaps = capability
                    .gaps
                    .iter()
                    .map(|g| Gap {
                        axis: g.axis.label(),
                        id: g.id.clone(),
                    })
                    .collect();
            }
            map.next.extend(actions::capability(&entry, document));
            map.entries.push(entry);
        }
        let whole_catalog = focus.is_none() && program.is_none();
        for fact in catalog
            .source_facts
            .values()
            .filter(|f| whole_catalog || fact_ids.contains(&f.id))
        {
            let mut entry = Entry::new(
                &fact.id,
                "source-fact",
                &fact.id,
                &fact.source_contract.scope,
                fact.status.label(),
            );
            entry.level = Some(fact.level.label().into());
            entry.contracts(std::slice::from_ref(&fact.source_contract));
            map.next.extend(actions::source(&entry));
            map.entries.push(entry);
        }
        for item in &catalog.items {
            if !whole_catalog
                && !item
                    .source_fact
                    .as_ref()
                    .is_some_and(|id| fact_ids.contains(id))
            {
                continue;
            }
            let mut entry = Entry::new(
                &item.id,
                "inventory",
                &item.title,
                &item.scope_and_limitations,
                item.status.label(),
            );
            entry.level = Some(item.level.label().into());
            entry.documentation_base = catalog
                .sections
                .iter()
                .find(|s| s.id == item.section)
                .map(|s| s.source_document.clone());
            entry.owners.extend(item.source_paths.iter().cloned());
            entry.source_facts.extend(item.source_fact.iter().cloned());
            if item.source_fact.is_none() {
                map.next.extend(actions::source(&entry));
            }
            map.entries.push(entry);
        }
        if whole_catalog {
            for reference in &catalog.references {
                let mut entry = Entry::new(
                    &reference.id,
                    "reference",
                    &reference.title,
                    &reference.details,
                    "not-an-implementation-claim",
                );
                entry.owners.extend(reference.source_paths.iter().cloned());
                entry.documentation_base = catalog
                    .sections
                    .iter()
                    .find(|s| s.id == reference.section)
                    .map(|s| s.source_document.clone());
                map.entries.push(entry);
            }
        }
        map.entries
            .sort_by(|a, b| (a.kind, &a.id).cmp(&(b.kind, &b.id)));
        map.next.sort_by(|a, b| {
            (&a.entry_kind, &a.entry, a.kind, &a.subject).cmp(&(
                &b.entry_kind,
                &b.entry,
                b.kind,
                &b.subject,
            ))
        });
        Ok(map)
    }

    pub(crate) fn print(&self, next_only: bool, details: bool) {
        render::print(self, next_only, details);
    }

    pub(crate) fn markdown(&self, output: &Path, root: &Path) -> Result<String> {
        render::markdown(self, output, root)
    }
}

fn select(
    declarations: &BTreeMap<String, CapabilityDocument>,
    focus: Option<&str>,
) -> Result<BTreeSet<String>> {
    let Some(focus) = focus else {
        return Ok(declarations.keys().cloned().collect());
    };
    let mut selected = BTreeSet::new();
    let mut pending = vec![focus.to_owned()];
    while let Some(id) = pending.pop() {
        let declaration = declarations
            .get(&id)
            .ok_or_else(|| format!("unknown capability in selected scope: {id}"))?;
        if selected.insert(id) {
            pending.extend(declaration.depends_on.iter().cloned());
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests;
