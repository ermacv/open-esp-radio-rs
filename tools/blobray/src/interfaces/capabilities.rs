//! Reusable capability rules evaluated over reviewed semantic interface evidence.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs,
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::{InterfaceWorkspace, ResolvedInterfaceSlot, SemanticCatalogs, validate_dotted_id};
use crate::{Result, error::BlobrayError};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityMatcherKind {
    Operation,
    Effect,
    Call,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CapabilityRequirement {
    kind: CapabilityMatcherKind,
    value: String,
    #[serde(default = "one_match")]
    min_matches: usize,
}

const fn one_match() -> usize {
    1
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CapabilityRule {
    id: String,
    /// Stable report classification, never an implicit matcher.
    protocol: String,
    /// Stable report classification, never a coverage boundary or filter.
    scope: String,
    summary: String,
    #[serde(default)]
    depends: Vec<String>,
    #[serde(default)]
    requirements: Vec<CapabilityRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct CapabilityPackDocument {
    schema: u32,
    id: String,
    rules: Vec<CapabilityRule>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CapabilityRuleSet {
    pack_ids: Vec<String>,
    rules: BTreeMap<String, CapabilityRule>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CapabilityMatchStatus {
    Matched,
    Incomplete,
    Unknown,
}

impl CapabilityMatchStatus {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CapabilityEvidence {
    pub(crate) binding: String,
    pub(crate) contract: String,
    pub(crate) anchor: String,
    pub(crate) source: String,
    pub(crate) layout_version: String,
    pub(crate) slot: String,
    pub(crate) offset: i32,
    pub(crate) operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) artifact: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) site: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CapabilityRequirementReport {
    pub(crate) kind: CapabilityMatcherKind,
    pub(crate) value: String,
    pub(crate) minimum_matches: usize,
    pub(crate) status: CapabilityMatchStatus,
    pub(crate) matches: Vec<CapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CapabilityRuleReport {
    pub(crate) id: String,
    pub(crate) protocol: String,
    pub(crate) scope: String,
    pub(crate) summary: String,
    pub(crate) status: CapabilityMatchStatus,
    pub(crate) dependencies: Vec<String>,
    pub(crate) requirements: Vec<CapabilityRequirementReport>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct CapabilityEvaluationReport {
    pub(crate) schema: u32,
    pub(crate) status: CapabilityMatchStatus,
    pub(crate) packs: Vec<String>,
    pub(crate) matched: usize,
    pub(crate) incomplete: usize,
    pub(crate) unknown: usize,
    pub(crate) rules: Vec<CapabilityRuleReport>,
}

impl CapabilityRuleSet {
    #[tracing::instrument(name = "load_capability_rules", skip_all, fields(packs = paths.len()))]
    pub(crate) fn load(paths: &[impl AsRef<Path>]) -> Result<Self> {
        let mut pack_ids = BTreeSet::new();
        let mut rules = BTreeMap::new();
        for path in paths {
            let path = path.as_ref();
            let input = fs::read_to_string(path)
                .map_err(|error| BlobrayError::read("capability pack", path, error))?;
            let document: CapabilityPackDocument =
                toml_edit::de::from_str(&input).map_err(|error| {
                    BlobrayError::manifest_source(
                        "capability pack",
                        path,
                        &input,
                        &error,
                        error.span(),
                    )
                })?;
            validate_pack(path, &document)?;
            if !pack_ids.insert(document.id.clone()) {
                return Err(crate::Error::invalid(format!(
                    "duplicate capability pack id {:?}",
                    document.id
                )));
            }
            for rule in document.rules {
                match rules.entry(rule.id.clone()) {
                    Entry::Vacant(entry) => {
                        entry.insert(rule);
                    }
                    Entry::Occupied(entry) if entry.get() == &rule => {
                        // Identical reusable rules compose independently of pack order.
                    }
                    Entry::Occupied(_) => {
                        return Err(crate::Error::invalid(format!(
                            "conflicting reusable capability rule {:?}",
                            rule.id
                        )));
                    }
                }
            }
        }
        validate_dependencies(&rules)?;
        Ok(Self {
            pack_ids: pack_ids.into_iter().collect(),
            rules,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    pub(crate) fn evaluate(&self, workspace: &InterfaceWorkspace) -> CapabilityEvaluationReport {
        self.evaluate_inputs(&workspace.semantic_catalogs, workspace.bindings())
    }

    fn evaluate_inputs(
        &self,
        catalogs: &SemanticCatalogs,
        bindings: &[ResolvedInterfaceSlot],
    ) -> CapabilityEvaluationReport {
        let requirements = self
            .rules
            .iter()
            .map(|(id, rule)| {
                (
                    id.clone(),
                    rule.requirements
                        .iter()
                        .map(|requirement| evaluate_requirement(requirement, catalogs, bindings))
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let own_status = requirements
            .iter()
            .map(|(id, requirements)| {
                (
                    id.clone(),
                    combined_status(requirements.iter().map(|requirement| requirement.status)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut resolved = BTreeMap::new();
        for id in self.rules.keys() {
            resolve_rule_status(id, &self.rules, &own_status, &mut resolved);
        }
        let rules = self
            .rules
            .values()
            .map(|rule| CapabilityRuleReport {
                id: rule.id.clone(),
                protocol: rule.protocol.clone(),
                scope: rule.scope.clone(),
                summary: rule.summary.clone(),
                status: resolved[&rule.id],
                dependencies: rule.depends.clone(),
                requirements: requirements[&rule.id].clone(),
            })
            .collect::<Vec<_>>();
        let matched = rules
            .iter()
            .filter(|rule| rule.status == CapabilityMatchStatus::Matched)
            .count();
        let incomplete = rules
            .iter()
            .filter(|rule| rule.status == CapabilityMatchStatus::Incomplete)
            .count();
        let unknown = rules
            .iter()
            .filter(|rule| rule.status == CapabilityMatchStatus::Unknown)
            .count();
        CapabilityEvaluationReport {
            schema: 1,
            status: combined_status(rules.iter().map(|rule| rule.status)),
            packs: self.pack_ids.clone(),
            matched,
            incomplete,
            unknown,
            rules,
        }
    }
}

fn validate_pack(path: &Path, document: &CapabilityPackDocument) -> Result<()> {
    if document.schema != 1 {
        return Err(crate::Error::invalid(format!(
            "capability pack {} requires schema = 1",
            path.display()
        )));
    }
    validate_dotted_id(&document.id, "capability pack id")?;
    if document.rules.is_empty() {
        return Err(crate::Error::invalid(format!(
            "capability pack {} has no rules",
            path.display()
        )));
    }
    let mut ids = BTreeSet::new();
    for rule in &document.rules {
        validate_dotted_id(&rule.id, "capability rule id")?;
        validate_dotted_id(&rule.protocol, "capability protocol")?;
        validate_dotted_id(&rule.scope, "capability scope")?;
        if rule.summary.trim().is_empty() {
            return Err(crate::Error::invalid(format!(
                "capability rule {:?} summary must not be empty",
                rule.id
            )));
        }
        if !ids.insert(rule.id.as_str()) {
            return Err(crate::Error::invalid(format!(
                "duplicate capability rule {:?} in {}",
                rule.id,
                path.display()
            )));
        }
        if rule.depends.is_empty() && rule.requirements.is_empty() {
            return Err(crate::Error::invalid(format!(
                "capability rule {:?} requires a dependency or matcher",
                rule.id
            )));
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &rule.depends {
            validate_dotted_id(dependency, "capability dependency")?;
            if !dependencies.insert(dependency) {
                return Err(crate::Error::invalid(format!(
                    "capability rule {:?} has duplicate dependency {dependency:?}",
                    rule.id
                )));
            }
        }
        let mut requirements = BTreeSet::new();
        for requirement in &rule.requirements {
            validate_dotted_id(&requirement.value, "capability matcher value")?;
            if requirement.min_matches == 0 {
                return Err(crate::Error::invalid(format!(
                    "capability rule {:?} matcher {:?} requires min-matches > 0",
                    rule.id, requirement.value
                )));
            }
            if !requirements.insert((requirement.kind, requirement.value.as_str())) {
                return Err(crate::Error::invalid(format!(
                    "capability rule {:?} has duplicate {:?} matcher {:?}",
                    rule.id, requirement.kind, requirement.value
                )));
            }
        }
    }
    Ok(())
}

fn validate_dependencies(rules: &BTreeMap<String, CapabilityRule>) -> Result<()> {
    for rule in rules.values() {
        for dependency in &rule.depends {
            if !rules.contains_key(dependency) {
                return Err(crate::Error::invalid(format!(
                    "capability rule {:?} depends on unknown rule {dependency:?}",
                    rule.id
                )));
            }
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in rules.keys() {
        visit_dependency(id, rules, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit_dependency<'a>(
    id: &'a str,
    rules: &'a BTreeMap<String, CapabilityRule>,
    visiting: &mut BTreeSet<&'a str>,
    visited: &mut BTreeSet<&'a str>,
) -> Result<()> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id) {
        return Err(crate::Error::invalid(format!(
            "capability dependency cycle includes {id:?}"
        )));
    }
    for dependency in &rules[id].depends {
        visit_dependency(dependency, rules, visiting, visited)?;
    }
    visiting.remove(id);
    visited.insert(id);
    Ok(())
}

fn evaluate_requirement(
    requirement: &CapabilityRequirement,
    catalogs: &SemanticCatalogs,
    bindings: &[ResolvedInterfaceSlot],
) -> CapabilityRequirementReport {
    let known = match requirement.kind {
        CapabilityMatcherKind::Operation | CapabilityMatcherKind::Call => {
            catalogs.get(&requirement.value).is_some()
        }
        CapabilityMatcherKind::Effect => catalogs.contains_effect(&requirement.value),
    };
    let matches = if known {
        matching_evidence(requirement, bindings)
    } else {
        Vec::new()
    };
    let status = if !known {
        CapabilityMatchStatus::Unknown
    } else if matches.len() < requirement.min_matches {
        CapabilityMatchStatus::Incomplete
    } else {
        CapabilityMatchStatus::Matched
    };
    CapabilityRequirementReport {
        kind: requirement.kind,
        value: requirement.value.clone(),
        minimum_matches: requirement.min_matches,
        status,
        matches,
    }
}

fn matching_evidence(
    requirement: &CapabilityRequirement,
    bindings: &[ResolvedInterfaceSlot],
) -> Vec<CapabilityEvidence> {
    let mut matches = BTreeSet::new();
    for binding in bindings {
        let Some(semantic) = &binding.semantic_annotation else {
            continue;
        };
        match requirement.kind {
            CapabilityMatcherKind::Operation if semantic.operation == requirement.value => {
                matches.insert(binding_evidence(binding, None));
            }
            CapabilityMatcherKind::Effect
                if semantic
                    .effects
                    .iter()
                    .any(|effect| effect == &requirement.value) =>
            {
                let mut evidence = binding_evidence(binding, None);
                evidence.effect = Some(requirement.value.clone());
                matches.insert(evidence);
            }
            CapabilityMatcherKind::Call if semantic.operation == requirement.value => {
                for call in &binding.calls {
                    matches.insert(binding_evidence(binding, Some(call)));
                }
            }
            _ => {}
        }
    }
    matches.into_iter().collect()
}

fn binding_evidence(
    binding: &ResolvedInterfaceSlot,
    call: Option<&super::ResolvedInterfaceCall>,
) -> CapabilityEvidence {
    CapabilityEvidence {
        binding: binding.id.clone(),
        contract: binding.contract.clone(),
        anchor: binding.anchor.clone(),
        source: binding.source.clone(),
        layout_version: binding.layout_version.clone(),
        slot: binding.name.clone(),
        offset: binding.offset,
        operation: binding
            .semantic_annotation
            .as_ref()
            .expect("semantic evidence was checked")
            .operation
            .clone(),
        effect: None,
        function: call.map(|call| call.function.clone()),
        artifact: call.map(|call| call.artifact),
        site: call.map(|call| call.site),
    }
}

fn resolve_rule_status(
    id: &str,
    rules: &BTreeMap<String, CapabilityRule>,
    own_status: &BTreeMap<String, CapabilityMatchStatus>,
    resolved: &mut BTreeMap<String, CapabilityMatchStatus>,
) -> CapabilityMatchStatus {
    if let Some(status) = resolved.get(id) {
        return *status;
    }
    let status = combined_status(
        std::iter::once(own_status[id]).chain(
            rules[id]
                .depends
                .iter()
                .map(|dependency| resolve_rule_status(dependency, rules, own_status, resolved)),
        ),
    );
    resolved.insert(id.to_owned(), status);
    status
}

fn combined_status(
    statuses: impl IntoIterator<Item = CapabilityMatchStatus>,
) -> CapabilityMatchStatus {
    statuses
        .into_iter()
        .max()
        .unwrap_or(CapabilityMatchStatus::Matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{ResolvedInterfaceCall, ResolvedSemanticAnnotation};

    fn root(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "blobray-capability-rules-{}-{name}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn catalog(path: &Path) -> SemanticCatalogs {
        std::fs::write(
            path,
            "schema = 1\nid = \"fixture.semantic\"\n\
             [[operations]]\n\
             id = \"wifi.rx.deliver\"\n\
             domain = \"wifi\"\n\
             summary = \"Deliver RX\"\n\
             argument-roles = [\"frame\"]\n\
             return-role = \"none\"\n\
             effects = [\"network.receive-delivery\"]\n\
             [[operations]]\n\
             id = \"bluetooth.hci.send\"\n\
             domain = \"bluetooth\"\n\
             summary = \"Send HCI\"\n\
             argument-roles = [\"packet\"]\n\
             return-role = \"status\"\n\
             effects = [\"bluetooth.hci-send\"]\n",
        )
        .unwrap();
        SemanticCatalogs::load(&[path]).unwrap()
    }

    fn wifi_binding() -> ResolvedInterfaceSlot {
        ResolvedInterfaceSlot {
            id: "fixture::wifi@+0x4".to_owned(),
            contract: "fixture::wifi".to_owned(),
            anchor: "wifi-table".to_owned(),
            source: "vendor".to_owned(),
            layout_version: "wifi-v1".to_owned(),
            offset: 4,
            width: 32,
            name: "receive".to_owned(),
            arguments: vec!["void *".to_owned()],
            return_type: "void".to_owned(),
            variadic: false,
            semantic: Some("wifi.rx.deliver".to_owned()),
            semantic_annotation: Some(ResolvedSemanticAnnotation {
                operation: "wifi.rx.deliver".to_owned(),
                domain: "wifi".to_owned(),
                summary: "Deliver RX".to_owned(),
                argument_roles: vec!["frame".to_owned()],
                return_role: "none".to_owned(),
                effects: vec!["network.receive-delivery".to_owned()],
                replacement: None,
            }),
            execution_model_set: None,
            execution_model: None,
            assignments: Vec::new(),
            functions: BTreeSet::from(["wifi_input".to_owned()]),
            calls: vec![ResolvedInterfaceCall {
                artifact: 2,
                member: Some("rx.o".to_owned()),
                function: "wifi_input".to_owned(),
                function_address: 0x1000,
                site: 0x1010,
                slot_load_site: Some(0x100c),
                kind: "indirect".to_owned(),
                jalr_offset: 0,
                slot_selector: None,
                slot_index: None,
                slot_index_domain: None,
                arguments: Vec::new(),
            }],
        }
    }

    #[test]
    fn dependencies_and_all_matcher_kinds_emit_deterministic_evidence() {
        let root = root("matched");
        let catalog_path = root.join("semantics.toml");
        let catalogs = catalog(&catalog_path);
        let rules_path = root.join("rules.toml");
        std::fs::write(
            &rules_path,
            "schema = 1\nid = \"fixture.capabilities\"\n\
             [[rules]]\n\
             id = \"fixture.wifi.binding\"\n\
             protocol = \"wifi\"\n\
             scope = \"application.receive\"\n\
             summary = \"Reviewed binding\"\n\
             [[rules.requirements]]\n\
             kind = \"operation\"\n\
             value = \"wifi.rx.deliver\"\n\
             [[rules.requirements]]\n\
             kind = \"effect\"\n\
             value = \"network.receive-delivery\"\n\
             [[rules]]\n\
             id = \"fixture.wifi.call\"\n\
             protocol = \"wifi\"\n\
             scope = \"application.receive\"\n\
             summary = \"Resolved call\"\n\
             depends = [\"fixture.wifi.binding\"]\n\
             [[rules.requirements]]\n\
             kind = \"call\"\n\
             value = \"wifi.rx.deliver\"\n",
        )
        .unwrap();
        let rules = CapabilityRuleSet::load(&[&rules_path]).unwrap();
        let bindings = [wifi_binding()];

        let report = rules.evaluate_inputs(&catalogs, &bindings);

        assert_eq!(report.status, CapabilityMatchStatus::Matched);
        assert_eq!(report.matched, 2);
        assert_eq!(report.incomplete, 0);
        assert_eq!(report.unknown, 0);
        assert_eq!(report.rules[0].id, "fixture.wifi.binding");
        assert_eq!(report.rules[1].dependencies, ["fixture.wifi.binding"]);
        assert_eq!(
            report.rules[1].requirements[0].matches[0].site,
            Some(0x1010)
        );
        assert_eq!(report, rules.evaluate_inputs(&catalogs, &bindings));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_evidence_and_unknown_vocabulary_propagate_fail_closed() {
        let root = root("gaps");
        let catalog_path = root.join("semantics.toml");
        let catalogs = catalog(&catalog_path);
        let rules_path = root.join("rules.toml");
        std::fs::write(
            &rules_path,
            "schema = 1\nid = \"fixture.capabilities\"\n\
             [[rules]]\n\
             id = \"fixture.bluetooth.binding\"\n\
             protocol = \"bluetooth\"\n\
             scope = \"controller.transport\"\n\
             summary = \"Known vocabulary without evidence\"\n\
             [[rules.requirements]]\n\
             kind = \"operation\"\n\
             value = \"bluetooth.hci.send\"\n\
             [[rules]]\n\
             id = \"fixture.bluetooth.ready\"\n\
             protocol = \"bluetooth\"\n\
             scope = \"controller.transport\"\n\
             summary = \"Depends on incomplete evidence\"\n\
             depends = [\"fixture.bluetooth.binding\"]\n\
             [[rules]]\n\
             id = \"fixture.ieee802154.binding\"\n\
             protocol = \"ieee802154\"\n\
             scope = \"application.receive\"\n\
             summary = \"Unknown vocabulary\"\n\
             [[rules.requirements]]\n\
             kind = \"operation\"\n\
             value = \"ieee802154.radio.receive\"\n\
             [[rules]]\n\
             id = \"fixture.ieee802154.ready\"\n\
             protocol = \"ieee802154\"\n\
             scope = \"application.receive\"\n\
             summary = \"Depends on unknown vocabulary\"\n\
             depends = [\"fixture.ieee802154.binding\"]\n",
        )
        .unwrap();
        let report = CapabilityRuleSet::load(&[&rules_path])
            .unwrap()
            .evaluate_inputs(&catalogs, &[]);

        assert_eq!(report.status, CapabilityMatchStatus::Unknown);
        assert_eq!(report.matched, 0);
        assert_eq!(report.incomplete, 2);
        assert_eq!(report.unknown, 2);
        assert_eq!(
            report.rules[0].requirements[0].status,
            CapabilityMatchStatus::Incomplete
        );
        assert_eq!(report.rules[1].status, CapabilityMatchStatus::Incomplete);
        assert_eq!(report.rules[2].status, CapabilityMatchStatus::Unknown);
        assert_eq!(report.rules[3].status, CapabilityMatchStatus::Unknown);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_cycles_are_rejected_before_evaluation() {
        let root = root("cycle");
        let rules_path = root.join("rules.toml");
        std::fs::write(
            &rules_path,
            "schema = 1\nid = \"fixture.capabilities\"\n\
             [[rules]]\n\
             id = \"fixture.first\"\n\
             protocol = \"radio\"\n\
             scope = \"runtime\"\n\
             summary = \"First\"\n\
             depends = [\"fixture.second\"]\n\
             [[rules]]\n\
             id = \"fixture.second\"\n\
             protocol = \"radio\"\n\
             scope = \"runtime\"\n\
             summary = \"Second\"\n\
             depends = [\"fixture.first\"]\n",
        )
        .unwrap();

        let error = CapabilityRuleSet::load(&[&rules_path]).unwrap_err();
        assert!(error.to_string().contains("dependency cycle"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
