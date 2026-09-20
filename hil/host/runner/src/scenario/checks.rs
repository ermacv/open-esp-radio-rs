//! Named checks actually produced by workloads, not inferred from scenario tags.
//!
//! Names describe observed properties. Their acceptance limits remain in the
//! scenario criteria; listing a check is not evidence that it passed.

use super::{Direction, Scenario, Workload};

impl Scenario {
    pub(crate) fn supported_checks(&self) -> Vec<&'static str> {
        let mut checks = Vec::new();
        if let Workload::Udp {
            direction: Direction::Rx,
            station_pause,
            ..
        } = self.workload
        {
            if self.criteria.minimum_rx_bps.is_some() {
                checks.push("udp.rx.target-rate");
                checks.push("udp.rx.host-offer-rate");
            }
            if self.criteria.maximum_rx_silence_ms.is_some() {
                checks.push("udp.rx.maximum-silence");
            }
            if station_pause.is_some() {
                checks.push("wifi.maintenance.transaction-valid");
                checks.push("wifi.maintenance.same-link");
                if self.criteria.require_post_maintenance_echo {
                    checks.push("wifi.maintenance.ip-exchange-resumed");
                }
            }
        }
        match self.workload {
            Workload::StationApLoss {
                require_recovery_echo,
                ..
            } => {
                checks.extend([
                    "wifi.station.ap-loss-reconnected",
                    "wifi.station.control-responsive",
                ]);
                if require_recovery_echo {
                    checks.push("wifi.station.recovered-ip-exchange");
                }
            }
            Workload::StationApAbsence {
                initially_absent, ..
            } => {
                checks.push(if initially_absent {
                    "wifi.station.initial-retry-exhausted"
                } else {
                    "wifi.station.recovery-retry-exhausted"
                });
                checks.push("wifi.station.control-responsive");
            }
            _ => {}
        }
        checks.sort_unstable();
        checks
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn initial_absence_and_recovery_publish_distinct_checks() {
        let catalog =
            super::super::Catalog::load(&crate::repository_root().unwrap().join("hil/scenarios"))
                .unwrap();
        let initial = catalog
            .get("station-ap-initial-absence")
            .unwrap()
            .supported_checks();
        let recovery = catalog
            .get("station-ap-absence")
            .unwrap()
            .supported_checks();
        assert!(initial.contains(&"wifi.station.initial-retry-exhausted"));
        assert!(!initial.contains(&"wifi.station.recovery-retry-exhausted"));
        assert!(recovery.contains(&"wifi.station.recovery-retry-exhausted"));
        assert!(!recovery.contains(&"wifi.station.initial-retry-exhausted"));
        assert!(
            catalog
                .get("station-ap-loss")
                .unwrap()
                .supported_checks()
                .contains(&"wifi.station.recovered-ip-exchange")
        );
    }

    #[test]
    fn post_maintenance_exchange_is_explicit_and_requires_an_intervention() {
        let mut experiment: super::Scenario = toml::from_str(include_str!(
            "../../../../scenarios/ieee80211/station/station-phy-maintenance-continuity.toml"
        ))
        .unwrap();
        experiment.validate().unwrap();
        assert!(
            experiment
                .supported_checks()
                .contains(&"wifi.maintenance.ip-exchange-resumed")
        );
        experiment.criteria.require_post_maintenance_echo = false;
        assert!(
            !experiment
                .supported_checks()
                .contains(&"wifi.maintenance.ip-exchange-resumed")
        );
        experiment.criteria.require_post_maintenance_echo = true;
        if let super::Workload::Udp { station_pause, .. } = &mut experiment.workload {
            *station_pause = None;
        }
        assert!(experiment.validate().is_err());
    }

    #[test]
    fn control_does_not_claim_an_unexecuted_maintenance_transaction() {
        let catalog =
            super::super::Catalog::load(&crate::repository_root().unwrap().join("hil/scenarios"))
                .unwrap();
        let control = catalog.get("udp-rx-he20-ceiling").unwrap();
        let experiment = catalog.get("udp-rx-he20-calibration").unwrap();
        assert!(
            !control
                .supported_checks()
                .contains(&"wifi.maintenance.same-link")
        );
        assert!(
            experiment
                .supported_checks()
                .contains(&"wifi.maintenance.same-link")
        );
        assert!(
            !experiment
                .supported_checks()
                .contains(&"udp.rx.maximum-silence")
        );
        assert!(
            catalog
                .get("boot-smoke")
                .unwrap()
                .supported_checks()
                .is_empty()
        );
    }
}
