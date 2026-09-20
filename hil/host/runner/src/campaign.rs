//! Offline, executable selection with explicit control/experiment relations.
//!
//! The plan binds scenario semantics, not a firmware build. Execution builds
//! and records the current firmware normally. Neither a dependency nor a plan
//! confers qualification, and no previous PASS is synthesized here.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    image::{ImageClass, Integration},
    lab::requirements::Requirements,
    scenario::{Catalog, Scenario},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Reason {
    Requested,
    ProvidesChecks { checks: Vec<String> },
    ControlFor { experiment: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    scenario: String,
    scenario_sha256: String,
    image: ImageClass,
    repetitions: u8,
    requirements: Requirements,
    reasons: Vec<Reason>,
    supported_checks: Vec<String>,
}

/// Serializable execution inputs; unlike run provenance this is not evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    schema: u16,
    network: String,
    requested: Vec<String>,
    requested_checks: Vec<String>,
    requirements: Requirements,
    scenarios: Vec<Entry>,
}

impl Plan {
    #[cfg(test)]
    pub(crate) fn create(
        catalog: &Catalog,
        requested: &[&Scenario],
        network: Integration,
    ) -> Result<Self> {
        Self::create_for_checks(catalog, requested, network, &[])
    }

    pub(crate) fn create_for_checks(
        catalog: &Catalog,
        candidates: &[&Scenario],
        network: Integration,
        checks: &[String],
    ) -> Result<Self> {
        let unique = checks.iter().collect::<BTreeSet<_>>();
        if unique.len() != checks.len() {
            return Err("campaign repeats a requested check".into());
        }
        let checks = unique.into_iter().cloned().collect::<Vec<_>>();
        let requested = candidates
            .iter()
            .copied()
            .filter(|scenario| {
                let supported = scenario.supported_checks();
                checks
                    .iter()
                    .all(|check| supported.contains(&check.as_str()))
            })
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Err("no selected scenario supplies every requested check".into());
        }
        let ids = requested
            .iter()
            .map(|s| s.id.clone())
            .collect::<BTreeSet<_>>();
        if ids.len() != requested.len() {
            return Err("campaign repeats a requested scenario".into());
        }
        let mut ordered = Vec::<&Scenario>::new();
        let mut seen = BTreeSet::new();
        // Only execution controls expand selection. Product prerequisites do
        // not belong to this graph, and never trigger a baseline suite here.
        for id in &ids {
            let scenario = catalog.get(id)?;
            if let Some(comparison) = &scenario.comparison {
                comparison.validate(scenario, catalog)?;
                let control = catalog.get(comparison.control())?;
                if seen.insert(control.id.clone()) {
                    ordered.push(control);
                }
            }
            if seen.insert(id.clone()) {
                ordered.push(scenario);
            }
        }
        // Match the executor's single-flash-per-image grouping. Controls share
        // their experiment's image and retain their before-experiment order.
        let ordered = ImageClass::ALL
            .into_iter()
            .flat_map(|image| ordered.iter().copied().filter(move |s| s.image == image))
            .collect::<Vec<_>>();
        let scenarios = ordered
            .iter()
            .map(|scenario| {
                let mut reasons = Vec::new();
                if ids.contains(&scenario.id) {
                    reasons.push(Reason::Requested);
                    if !checks.is_empty() {
                        reasons.push(Reason::ProvidesChecks {
                            checks: checks.clone(),
                        });
                    }
                }
                for id in &ids {
                    let experiment = catalog.get(id)?;
                    if experiment
                        .comparison
                        .as_ref()
                        .is_some_and(|c| c.control() == scenario.id)
                    {
                        reasons.push(Reason::ControlFor {
                            experiment: id.clone(),
                        });
                    }
                }
                Ok(Entry {
                    scenario: scenario.id.clone(),
                    scenario_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(scenario)?)),
                    image: scenario.image,
                    repetitions: scenario.repetitions,
                    requirements: Requirements::for_scenario(scenario),
                    reasons,
                    supported_checks: scenario
                        .supported_checks()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            schema: 3,
            network: network.id().to_owned(),
            requested: ids.into_iter().collect(),
            requested_checks: checks,
            requirements: Requirements::union(&ordered),
            scenarios,
        })
    }

    /// Reconstruct the plan before acquiring fixture leases or touching a DUT.
    /// Editing the output cannot silently change its repetitions or workload.
    pub(crate) fn resolve<'a>(
        &self,
        catalog: &'a Catalog,
    ) -> Result<(Vec<&'a Scenario>, Integration)> {
        if self.schema != 3 {
            return Err("unsupported executable campaign schema".into());
        }
        let network: Integration = self.network.parse()?;
        let requested = self
            .requested
            .iter()
            .map(|id| catalog.get(id))
            .collect::<Result<Vec<_>>>()?;
        let expected =
            Self::create_for_checks(catalog, &requested, network, &self.requested_checks)?;
        if *self != expected {
            return Err(
                "campaign differs from current scenario contracts; generate and review a new plan"
                    .into(),
            );
        }
        Ok((
            self.scenarios
                .iter()
                .map(|entry| catalog.get(&entry.scenario))
                .collect::<Result<_>>()?,
            network,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(&crate::repository_root().unwrap().join("hil/scenarios")).unwrap()
    }

    const EXPERIMENT: &str = "diagnostic-station-phy-combined-high-load-delivery-rx";

    #[test]
    fn selecting_integration_adds_only_its_control_not_wifi_prerequisites() {
        let catalog = catalog();
        let experiment = catalog.get(EXPERIMENT).unwrap();
        let plan = Plan::create(&catalog, &[experiment], Integration::UpstreamXarxa).unwrap();
        let (selected, _) = plan.resolve(&catalog).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected[0].id,
            experiment.comparison.as_ref().unwrap().control()
        );
        assert_eq!(selected[1].id, EXPERIMENT);
        assert_eq!(
            plan.scenarios[0].reasons,
            vec![Reason::ControlFor {
                experiment: EXPERIMENT.into()
            }]
        );
        let roundtrip: Plan = serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        assert_eq!(plan, roundtrip);
    }

    #[test]
    fn selecting_control_directly_does_not_duplicate_it() {
        let catalog = catalog();
        let experiment = catalog.get(EXPERIMENT).unwrap();
        let control = catalog
            .get(experiment.comparison.as_ref().unwrap().control())
            .unwrap();
        let plan =
            Plan::create(&catalog, &[control, experiment], Integration::UpstreamXarxa).unwrap();
        assert_eq!(plan.scenarios.len(), 2);
        assert_eq!(plan.scenarios[0].reasons.len(), 2);
        assert!(Plan::create(&catalog, &[control, control], Integration::UpstreamXarxa).is_err());
    }

    #[test]
    fn he20_integration_uses_its_own_baseline_without_ht40_evidence() {
        let catalog = catalog();
        let plan = Plan::create(
            &catalog,
            &[catalog.get("udp-rx-he20-calibration").unwrap()],
            Integration::UpstreamXarxa,
        )
        .unwrap();
        let (selected, _) = plan.resolve(&catalog).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|scenario| scenario.id.as_str())
                .collect::<Vec<_>>(),
            ["udp-rx-he20-ceiling", "udp-rx-he20-calibration"]
        );
        assert!(selected.iter().all(|scenario| scenario.link.unwrap().phy == crate::scenario::PhyExpectation::He20));
    }

    #[test]
    fn named_check_selection_is_scoped_and_does_not_promote_controls() {
        let catalog = catalog();
        let selected = catalog
            .all()
            .iter()
            .filter(|scenario| scenario.tags.iter().any(|tag| tag == "he20"))
            .collect::<Vec<_>>();
        let plan = Plan::create_for_checks(
            &catalog,
            &selected,
            Integration::UpstreamXarxa,
            &["wifi.maintenance.same-link".into()],
        )
        .unwrap();
        let (resolved, _) = plan.resolve(&catalog).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(plan.requested, ["udp-rx-he20-calibration"]);
        assert!(
            !plan.scenarios[0]
                .supported_checks
                .contains(&"wifi.maintenance.same-link".into())
        );
        assert!(
            matches!(&plan.scenarios[1].reasons[1], Reason::ProvidesChecks { checks } if checks == &["wifi.maintenance.same-link"])
        );
        assert!(
            Plan::create_for_checks(
                &catalog,
                &selected,
                Integration::UpstreamXarxa,
                &["not-a-check".into()]
            )
            .is_err()
        );
        assert!(
            Plan::create_for_checks(
                &catalog,
                &selected,
                Integration::UpstreamXarxa,
                &[
                    "wifi.maintenance.same-link".into(),
                    "udp.rx.maximum-silence".into()
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn ordinary_selection_does_not_expand_and_modified_plans_fail_closed() {
        let catalog = catalog();
        let plan = Plan::create(
            &catalog,
            &[catalog.get("boot-smoke").unwrap()],
            Integration::UpstreamXarxa,
        )
        .unwrap();
        assert_eq!(plan.scenarios.len(), 1);
        let mut changed = plan.clone();
        changed.scenarios[0].repetitions += 1;
        assert!(changed.resolve(&catalog).is_err());
        let mut changed = plan.clone();
        changed.scenarios[0].scenario_sha256 = "00".repeat(32);
        assert!(changed.resolve(&catalog).is_err());
        let mut changed = plan.clone();
        changed.schema = 1;
        assert!(changed.resolve(&catalog).is_err());
        let mut changed = plan;
        changed.scenarios.clear();
        assert!(changed.resolve(&catalog).is_err());
    }
}
