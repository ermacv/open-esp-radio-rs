//! Relations between an experiment and its contemporaneous control.
//!
//! A control is an execution input. Qualification dependencies are not: they
//! must never be expanded into executions by this module. This first supported
//! experiment varies only the station PHY maintenance operation, keeping the
//! firmware, link, traffic, observation and absolute acceptance policy equal.

use serde::{Deserialize, Serialize};

use super::{Catalog, Scenario, Workload};
use crate::Result;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ControlledComparison {
    WifiPhyMaintenance { control: String },
}

impl ControlledComparison {
    pub fn control(&self) -> &str {
        match self {
            Self::WifiPhyMaintenance { control } => control,
        }
    }

    pub(crate) fn validate(&self, experiment: &Scenario, catalog: &Catalog) -> Result<()> {
        let control = catalog.get(self.control())?;
        if control.id == experiment.id || control.comparison.is_some() {
            return Err(
                format!("{}: control must be an independent scenario", experiment.id).into(),
            );
        }
        let mut normalized = experiment.clone();
        match (&mut normalized.workload, &control.workload) {
            (
                Workload::Udp { direction: super::Direction::Rx, station_pause, .. },
                Workload::Udp {
                    station_pause: None,
                    ..
                },
            ) if station_pause.is_some() && experiment.link.is_some() => {
                *station_pause = None;
            }
            _ => return Err(format!(
                "{}: PHY comparison requires station UDP RX with maintenance and a control without it",
                experiment.id
            )
            .into()),
        }
        // These identify/document the experiment; every executable setting,
        // including the observer image and fixture mutations, must still match.
        normalized.id.clone_from(&control.id);
        normalized.description.clone_from(&control.description);
        normalized.tags.clone_from(&control.tags);
        normalized.source.clone_from(&control.source);
        normalized.comparison = None;
        if normalized != *control {
            return Err(format!(
                "{}: control {} differs beyond the PHY maintenance intervention",
                experiment.id, control.id
            )
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn pair() -> (Scenario, Scenario) {
        let control = toml::from_str(include_str!("../../../../scenarios/ieee80211/station/diagnostic-station-phy-baseline-high-load-delivery-rx.toml")).unwrap();
        let experiment = toml::from_str(include_str!("../../../../scenarios/ieee80211/station/diagnostic-station-phy-combined-high-load-delivery-rx.toml")).unwrap();
        (control, experiment)
    }

    #[test]
    fn control_and_experiment_differ_only_in_intervention() {
        let (control, experiment) = pair();
        let catalog = Catalog {
            scenarios: vec![control, experiment.clone()],
        };
        experiment
            .comparison
            .as_ref()
            .unwrap()
            .validate(&experiment, &catalog)
            .unwrap();
    }

    #[test]
    fn rejects_different_link_observer_workload_policy_and_control_chains() {
        let (control, experiment) = pair();
        let mut changed = Vec::new();
        let mut link = control.clone();
        link.link.as_mut().unwrap().phy = super::super::PhyExpectation::He20;
        changed.push(link);
        let mut criteria = control.clone();
        criteria.criteria.minimum_rx_bps = Some(1);
        changed.push(criteria);
        let mut observer = control.clone();
        observer.l1_cache_counters = !observer.l1_cache_counters;
        changed.push(observer);
        let mut chain = control.clone();
        chain.comparison = experiment.comparison.clone();
        changed.push(chain);
        let mut load = control.clone();
        if let Workload::Udp { payload_bytes, .. } = &mut load.workload {
            *payload_bytes = 512;
        }
        changed.push(load);
        for control in changed {
            let catalog = Catalog {
                scenarios: vec![control],
            };
            assert!(
                experiment
                    .comparison
                    .as_ref()
                    .unwrap()
                    .validate(&experiment, &catalog)
                    .is_err()
            );
        }
        assert!(
            experiment
                .comparison
                .as_ref()
                .unwrap()
                .validate(&experiment, &Catalog { scenarios: vec![] })
                .is_err()
        );
    }
}
