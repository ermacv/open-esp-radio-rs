//! Scenario-level controlled AP fixture for station-network workloads.
//!
//! Callers run the scenario's fixture preflight before [`Prepared::start`].

use super::controlled_ap::ControlledAp;
use crate::{
    Result,
    lab::{config::LabConfig, requirements::Requirements},
    scenario::Scenario,
};
use std::{
    cell::{RefCell, RefMut},
    path::Path,
};

pub(crate) struct Prepared {
    ap: Option<RefCell<ControlledAp>>,
}

impl Prepared {
    pub(crate) fn start(lab: &LabConfig, scenario: &Scenario, output: &Path) -> Result<Self> {
        let required = Requirements::for_scenario(scenario);
        let ap = if required.station_network {
            let phy = lab.fixture_phy(scenario);
            let mut ap = ControlledAp::start(&lab.station, &lab.station_fixture, phy)
                .map_err(super::Error::context)?;
            // A fresh selected-radio epoch is part of the multi-client fixture.
            // It never resets the router's other radios or the wired uplink.
            if required.openwrt_client {
                ap.stop()?;
                ap.restart()?;
            }
            if let ControlledAp::OpenWrt(owner) = &ap {
                crate::durable::atomic_json(
                    &output.join("fixture-applied.json"),
                    &owner.report()?,
                )?;
            }
            if let ControlledAp::Local(owner) = &ap {
                crate::durable::atomic_json(&output.join("fixture-applied.json"), &owner.report())?;
            }
            Some(RefCell::new(ap))
        } else {
            None
        };
        Ok(Self { ap })
    }

    pub(crate) fn ap(&self) -> Result<RefMut<'_, ControlledAp>> {
        self.ap
            .as_ref()
            .ok_or_else(|| "scenario did not prepare an AP fixture".into())
            .map(RefCell::borrow_mut)
    }
}
