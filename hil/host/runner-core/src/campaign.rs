//! Offline, executable selection with explicit control/experiment relations.
//!
//! The plan binds scenario semantics, not a firmware build. Execution builds
//! and records the current firmware normally. Neither a dependency nor a plan
//! confers qualification, and no previous PASS is synthesized here.

use std::collections::BTreeSet;
mod evidence;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    image::{ImageClass, Integration},
    lab::requirements::Requirements,
    scenario::{Catalog, Scenario, ScenarioFamily},
};

const CAMPAIGN_SCHEMA: u16 = 6;

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
pub struct Plan {
    schema: u16,
    network: String,
    requested: Vec<String>,
    requested_checks: Vec<String>,
    requirements: Requirements,
    scenarios: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    qualification: Option<evidence::Binding>,
}

// Descriptive metadata is preserved in run provenance, but is not executed.
fn procedure<F: ScenarioFamily>(scenario: &Scenario<F>) -> Result<serde_json::Value> {
    Ok(crate::scenario::identity::normalize(&serde_json::to_value(
        scenario,
    )?))
}

impl Plan {
    #[cfg(any(test, feature = "test-support"))]
    pub fn create<F: ScenarioFamily>(
        catalog: &Catalog<F>,
        requested: &[&Scenario<F>],
        network: Integration,
    ) -> Result<Self> {
        Self::create_for_checks(catalog, requested, network, &[])
    }

    pub fn create_for_checks<F: ScenarioFamily>(
        catalog: &Catalog<F>,
        candidates: &[&Scenario<F>],
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
                let supported = scenario.plan().checks;
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
            .map(|s| s.id().to_owned())
            .collect::<BTreeSet<_>>();
        if ids.len() != requested.len() {
            return Err("campaign repeats a requested scenario".into());
        }
        let mut ordered = Vec::<&Scenario<F>>::new();
        let mut seen = BTreeSet::new();
        // Only execution controls expand selection. Product prerequisites do
        // not belong to this graph, and never trigger a baseline suite here.
        for id in &ids {
            let scenario = catalog.get(id)?;
            if let Some(control) = scenario.control() {
                let control = catalog.get(control)?;
                if seen.insert(control.id().to_owned()) {
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
            .flat_map(|image| ordered.iter().copied().filter(move |s| s.image() == image))
            .collect::<Vec<_>>();
        let scenarios = ordered
            .iter()
            .map(|scenario| {
                let mut reasons = Vec::new();
                let plan = scenario.plan();
                if ids.contains(scenario.id()) {
                    reasons.push(Reason::Requested);
                    if !checks.is_empty() {
                        reasons.push(Reason::ProvidesChecks {
                            checks: checks.clone(),
                        });
                    }
                }
                for id in &ids {
                    let experiment = catalog.get(id)?;
                    if experiment.control() == Some(scenario.id()) {
                        reasons.push(Reason::ControlFor {
                            experiment: id.clone(),
                        });
                    }
                }
                Ok(Entry {
                    scenario: scenario.id().to_owned(),
                    scenario_sha256: format!(
                        "{:x}",
                        Sha256::digest(serde_json::to_vec(&procedure(scenario)?)?)
                    ),
                    image: plan.image,
                    repetitions: scenario.repetitions(),
                    requirements: plan.requirements,
                    reasons,
                    supported_checks: plan.checks.into_iter().map(str::to_owned).collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            schema: CAMPAIGN_SCHEMA,
            qualification: None,
            network: network.id().to_owned(),
            requested: ids.into_iter().collect(),
            requested_checks: checks,
            requirements: Requirements::union(ordered.iter().map(|s| s.requirements())),
            scenarios,
        })
    }

    /// Reconstruct the plan before acquiring fixture leases or touching a DUT.
    /// Editing the output cannot silently change its repetitions or workload.
    pub fn resolve<'a, F: ScenarioFamily>(
        &self,
        catalog: &'a Catalog<F>,
    ) -> Result<(Vec<&'a Scenario<F>>, Integration)> {
        if self.schema != CAMPAIGN_SCHEMA {
            return Err("unsupported executable campaign schema".into());
        }
        let network: Integration = self.network.parse()?;
        let requested = self
            .requested
            .iter()
            .map(|id| catalog.get(id))
            .collect::<Result<Vec<_>>>()?;
        let mut expected = if let Some(binding) = &self.qualification {
            Self::from_selection(catalog, binding.clone(), network)?
        } else {
            Self::create_for_checks(catalog, &requested, network, &self.requested_checks)?
        };
        expected.qualification = self.qualification.clone();
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
mod tests;
