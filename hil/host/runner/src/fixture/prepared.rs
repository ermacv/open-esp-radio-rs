//! Scenario-level fixture lifetime shared by every workload, including AP tests.

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

pub(crate) fn check_without_device(
    root: &Path,
    lab: &LabConfig,
    scenario: &Scenario,
) -> Result<()> {
    let resolved = lab.resolve_scenario(scenario);
    let lab = &resolved;
    let required = Requirements::for_scenario(scenario);
    let _lease = crate::lab::lock::FixtureLock::acquire_without_device(lab, required)?;
    let output = root.join("target/hil/fixture-checks").join(format!(
        "{}-{}",
        crate::evidence::run::unix_millis()?,
        scenario.id
    ));
    std::fs::create_dir_all(&output)?;
    let cleanup = super::cleanup::Scope::new(&output);
    let result = Prepared::start(lab, scenario, &output).and_then(|prepared| {
        if required.station_control {
            let mut ap = prepared.ap()?;
            ap.stop()?;
            ap.restart()?;
        }
        if let crate::lab::config::StationFixtureConfig::OpenWrt(config) = &lab.station_fixture {
            if required.station_udp_rx_capture || required.station_udp_tx_capture {
                super::openwrt_fixture::check_capture(config)?;
            }
            if required.laptop_air_monitor {
                super::local_air_monitor::check_without_device(config, &output)?;
            }
        }
        Ok(())
    });
    let records = cleanup.finish()?;
    let restored = records.iter().all(|record| record.failure.is_none());
    let report = serde_json::json!({"schema": 1, "scenario": scenario.id, "device_accessed": false,
        "prepared": result.is_ok(), "restored": restored,
        "failure": result.as_ref().err().map(|error| error.to_string()), "cleanup": records});
    crate::evidence::run::atomic_json(&output.join("result.json"), &report)?;
    crate::emit_json(
        &serde_json::json!({"report": report, "artifacts": output}),
        true,
    )?;
    result?;
    if !restored {
        return Err("fixture restoration failed; see fixture-check artifacts".into());
    }
    Ok(())
}

impl Prepared {
    pub(crate) fn start(lab: &LabConfig, scenario: &Scenario, output: &Path) -> Result<Self> {
        let required = Requirements::for_scenario(scenario);
        super::preflight::check(lab, scenario)?;
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
                crate::evidence::run::atomic_json(
                    &output.join("fixture-applied.json"),
                    &owner.report()?,
                )?;
            }
            if let ControlledAp::Local(owner) = &ap {
                crate::evidence::run::atomic_json(
                    &output.join("fixture-applied.json"),
                    &owner.report(),
                )?;
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
