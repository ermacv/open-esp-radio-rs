//! Runner consumes evaluator decisions and refreshes them before any fixture effects.
use super::*;
use oer_process::CommandExt as _;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Selection {
    schema: u16,
    kind: String,
    target: String,
    pub scope_sha256: String,
    obligations: Vec<Obligation>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Action {
    Satisfied,
    Run,
    Review,
    Investigate,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Obligation {
    capability: String,
    scenario: String,
    checks: Vec<String>,
    minimum_repetitions: u8,
    property_sha256: String,
    procedure_sha256: String,
    action: Action,
    reason: String,
    evidence: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    manifest: PathBuf,
    capability: Option<String>,
    selection: Selection,
}

impl Selection {
    fn load(root: &Path, manifest: &Path, capability: Option<&str>) -> Result<Self> {
        let mut command = Command::new("cargo");
        command
            .current_dir(root)
            .args(["qualification", "plan", "--manifest"])
            .arg(manifest)
            .arg("--root")
            .arg(root);
        if let Some(capability) = capability {
            command.args(["--capability", capability]);
        }
        let output = command.supervised_output()?;
        if !output.status.success() {
            return Err(format!(
                "qualification planning failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let selection: Self = serde_json::from_slice(&output.stdout)?;
        if selection.schema != 1 || selection.kind != "open-esp-radio-hil-selection" {
            return Err("unsupported evaluator selection".into());
        }
        Ok(selection)
    }
    fn runnable<'a>(&self, catalog: &'a Catalog) -> Result<Vec<&'a Scenario>> {
        let mut candidates = BTreeSet::new();
        let mut withheld = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for obligation in &self.obligations {
            if !seen.insert((&obligation.capability, &obligation.scenario)) {
                return Err("evaluator selection repeats an obligation".into());
            }
            let scenario = catalog.get(&obligation.scenario)?;
            let procedure_sha256 = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&procedure(scenario)?)?)
            );
            if obligation.procedure_sha256 != procedure_sha256
                || obligation.minimum_repetitions == 0
                || obligation.minimum_repetitions > scenario.repetitions
                || obligation
                    .checks
                    .iter()
                    .any(|c| !scenario.supported_checks().contains(&c.as_str()))
            {
                return Err("evaluator obligation does not match the execution catalog".into());
            }
            match obligation.action {
                Action::Satisfied => {
                    if obligation.evidence.is_none() {
                        return Err("satisfied obligation has no evidence reference".into());
                    }
                }
                Action::Run => {
                    candidates.insert(&obligation.scenario);
                }
                Action::Review | Action::Investigate => {
                    withheld.insert(&obligation.scenario);
                }
            }
        }
        candidates
            .difference(&withheld)
            .map(|s| catalog.get(s))
            .collect()
    }
}
impl Plan {
    pub fn from_qualification(
        root: &Path,
        catalog: &Catalog,
        manifest: PathBuf,
        capability: Option<String>,
        network: Integration,
    ) -> Result<Self> {
        let selection = Selection::load(root, &manifest, capability.as_deref())?;
        Self::from_selection(
            catalog,
            Binding {
                manifest,
                capability,
                selection,
            },
            network,
        )
    }
    pub(super) fn from_selection(
        catalog: &Catalog,
        binding: Binding,
        network: Integration,
    ) -> Result<Self> {
        let selected = binding.selection.runnable(catalog)?;
        let mut plan = if selected.is_empty() {
            Self {
                schema: 5,
                network: network.id().into(),
                requested: vec![],
                requested_checks: vec![],
                requirements: Requirements::default(),
                scenarios: vec![],
                qualification: None,
            }
        } else {
            Self::create_for_checks(catalog, &selected, network, &[])?
        };
        plan.qualification = Some(binding);
        Ok(plan)
    }
    /// Retain the requested program scope, but recompute completion/applicability.
    /// New seals can reduce execution to zero without opening the lab or building.
    pub fn refresh(&self, root: &Path, catalog: &Catalog) -> Result<Self> {
        let Some(binding) = &self.qualification else {
            return Ok(self.clone());
        };
        let updated = Selection::load(root, &binding.manifest, binding.capability.as_deref())?;
        if updated.scope_sha256 != binding.selection.scope_sha256
            || updated.target != binding.selection.target
        {
            return Err("qualification program scope changed; generate a new plan".into());
        }
        Self::from_selection(
            catalog,
            Binding {
                manifest: binding.manifest.clone(),
                capability: binding.capability.clone(),
                selection: updated,
            },
            self.network.parse()?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding(action: Action) -> Binding {
        Binding {
            manifest: "program.toml".into(),
            capability: None,
            selection: Selection {
                schema: 1,
                kind: "open-esp-radio-hil-selection".into(),
                target: "test".into(),
                scope_sha256: "aa".repeat(32),
                obligations: vec![Obligation {
                    capability: "boot".into(),
                    scenario: "boot-smoke".into(),
                    checks: vec![],
                    minimum_repetitions: 1,
                    property_sha256: "bb".repeat(32),
                    procedure_sha256: format!(
                        "{:x}",
                        Sha256::digest(
                            serde_json::to_vec(
                                &procedure(
                                    super::super::tests::catalog().get("boot-smoke").unwrap()
                                )
                                .unwrap()
                            )
                            .unwrap()
                        )
                    ),
                    action,
                    reason: "test decision".into(),
                    evidence: Some("sealed observation".into()),
                }],
            },
        }
    }
    #[test]
    fn satisfied_obligations_and_reviews_do_not_expand_into_hardware_runs() {
        let catalog = super::super::tests::catalog();
        for action in [Action::Satisfied, Action::Review, Action::Investigate] {
            let plan = Plan::from_selection(&catalog, binding(action), Integration::UpstreamXarxa)
                .unwrap();
            assert!(plan.resolve(&catalog).unwrap().0.is_empty());
        }
        let plan = Plan::from_selection(&catalog, binding(Action::Run), Integration::UpstreamXarxa)
            .unwrap();
        assert_eq!(
            plan.resolve(&catalog)
                .unwrap()
                .0
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            vec!["boot-smoke"]
        );
        let closed = Plan::from_selection(
            &catalog,
            binding(Action::Satisfied),
            Integration::UpstreamXarxa,
        )
        .unwrap();
        assert!(closed.resolve(&catalog).unwrap().0.is_empty());
    }
    #[test]
    fn mismatched_execution_catalog_cannot_consume_an_evaluator_decision() {
        let catalog = super::super::tests::catalog();
        let mut decision = binding(Action::Run);
        decision.selection.obligations[0].procedure_sha256 = "00".repeat(32);
        assert!(Plan::from_selection(&catalog, decision, Integration::UpstreamXarxa).is_err());
    }
    #[test]
    fn another_capability_cannot_hide_an_unsatisfied_obligation_for_the_same_scenario() {
        let catalog = super::super::tests::catalog();
        let mut selection = binding(Action::Satisfied);
        let mut required = selection.selection.obligations[0].clone();
        required.capability = "second".into();
        required.action = Action::Run;
        selection.selection.obligations.push(required);
        let plan = Plan::from_selection(&catalog, selection, Integration::UpstreamXarxa).unwrap();
        assert_eq!(plan.resolve(&catalog).unwrap().0.len(), 1);
    }
}
