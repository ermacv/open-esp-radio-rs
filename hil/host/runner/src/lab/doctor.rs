//! Read-only, scenario-scoped preflight. Report independent failures together.

use std::path::Path;

use serde::Serialize;

use super::{
    config::{LabConfig, StationFixtureConfig},
    requirements::Requirements,
};
use crate::{Result, fixture, image, scenario::Scenario};

#[derive(Default, Serialize)]
struct Checks {
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Check {
    name: String,
    passed: bool,
    failure: Option<String>,
}

impl Checks {
    fn run(&mut self, name: impl Into<String>, check: impl FnOnce() -> Result<()>) -> Result<()> {
        oer_process::check_cancelled()?;
        let result = check();
        if let Err(error) = &result
            && oer_process::is_cancelled(&**error)
        {
            return result;
        }
        self.checks.push(Check {
            name: name.into(),
            passed: result.is_ok(),
            failure: result.err().map(|error| error.to_string()),
        });
        Ok(())
    }

    fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }
}

pub(crate) fn run(root: &Path, lab: &LabConfig, scenarios: &[&Scenario]) -> Result<()> {
    let required = Requirements::union(scenarios);
    let mut checks = Checks::default();
    checks.run("firmware-workspace", || {
        root.join("hil/targets/esp32s31/Cargo.toml")
            .is_file()
            .then_some(())
            .ok_or_else(|| "missing embedded HIL workspace".into())
    })?;
    checks.run("serial-device", || fs_device_exists(&lab.device.serial))?;
    for (variable, program) in [
        ("CARGO", "cargo"),
        ("LLVM_OBJCOPY", "llvm-objcopy"),
        ("LLVM_OBJDUMP", "llvm-objdump"),
        ("LLVM_NM", "llvm-nm"),
        ("ESPFLASH", "espflash"),
    ] {
        checks.run(format!("tool-{program}"), || {
            image::require_program(&image::program_from_env(variable, program))
        })?;
    }
    checks.run("source-dependencies", || {
        image::ensure_no_old_application_dependency(root)?;
        image::ensure_vendor_dependencies_absent(root)
    })?;
    for scenario in scenarios {
        checks.run(format!("scenario-{}", scenario.id), || {
            if let Some(failure) = crate::scenario_precondition(lab, scenario) {
                return Err(failure.message.into());
            }
            if Requirements::for_scenario(scenario).station_control
                && matches!(lab.station_fixture, StationFixtureConfig::External(_))
            {
                return Err("scenario requires a controllable station access point".into());
            }
            Ok(())
        })?;
    }
    checks.run("lab-provenance", || {
        super::provenance::LabProvenance::capture(lab, required).map(|_| ())
    })?;
    checks.run("resource-ownership", || {
        super::lock::FixtureLock::acquire_for(lab, required).map(|_| ())
    })?;
    if required.station_network {
        match &lab.station_fixture {
            StationFixtureConfig::LocalLinux(_) => {
                checks.run(
                    "local-station-fixture",
                    fixture::controlled_ap::doctor_local,
                )?;
            }
            StationFixtureConfig::OpenWrt(config) if required.station_udp_rx_capture => {
                checks.run("openwrt-rx-evidence", || {
                    fixture::openwrt_fixture::doctor(config)
                })?;
            }
            StationFixtureConfig::OpenWrt(_) | StationFixtureConfig::External(_) => {}
        }
    }
    if required.laptop_client {
        checks.run("laptop-client", fixture::controlled_client::doctor)?;
    }
    if required.openwrt_client {
        checks.run("openwrt-client", || match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) => {
                fixture::controlled_openwrt_client::doctor(&lab.access_point, config)
            }
            _ => Err("scenario requires an OpenWrt client fixture".into()),
        })?;
    }
    if required.openwrt_tx_monitor {
        checks.run("openwrt-tx-monitor", || match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) if config.monitor_interface.is_some() => {
                fixture::openwrt_tx_monitor::doctor(config)
            }
            _ => Err("scenario requires an OpenWrt monitor interface".into()),
        })?;
    }
    if required.laptop_air_monitor {
        checks.run("laptop-air-monitor", || {
            fixture::local_air_monitor::doctor()
        })?;
    }
    crate::emit_json(
        &serde_json::json!({
            "schema": 1,
            "status": if checks.passed() { "passed" } else { "failed" },
            "cell_id": lab.cell_id(),
            "device_id": lab.device.id,
            "lab_config": lab.path(),
            "requirements": required,
            "checks": checks.checks,
        }),
        true,
    )?;
    if checks.passed() {
        Ok(())
    } else {
        Err("HIL environment checks failed; see the JSON check report".into())
    }
}

fn fs_device_exists(path: &Path) -> Result<()> {
    path.exists()
        .then_some(())
        .ok_or_else(|| format!("serial device does not exist: {}", path.display()).into())
}

#[cfg(test)]
mod tests;
