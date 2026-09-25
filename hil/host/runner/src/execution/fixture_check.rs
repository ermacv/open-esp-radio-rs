//! Fixture preparation and restoration without accessing the device.
use std::path::Path;

use crate::Result;
use hil_core::{lab::config::LabConfig, lab::requirements::Requirements, scenario::Scenario};

pub(crate) fn check_without_device(
    root: &Path,
    lab: &LabConfig,
    scenario: &Scenario,
) -> Result<()> {
    let resolved = lab.resolve_scenario(scenario);
    let lab = &resolved;
    let required = Requirements::for_scenario(scenario);
    let _lease = hil_core::lab::lock::FixtureLock::acquire_without_device(lab, required)?;
    let output = root.join("target/hil/fixture-checks").join(format!(
        "{}-{}",
        hil_core::durable::unix_millis()?,
        scenario.id
    ));
    std::fs::create_dir_all(&output)?;
    let cleanup = hil_core::fixture::cleanup::Scope::new(&output);
    let result = crate::fixture::preflight::check(lab, scenario)
        .and_then(|()| hil_wifi::fixture::prepared::Prepared::start(lab, scenario, &output))
        .and_then(|prepared| {
            if required.station_control {
                let mut ap = prepared.ap()?;
                ap.stop()?;
                ap.restart()?;
            }
            if let hil_core::lab::config::StationFixtureConfig::OpenWrt(config) =
                &lab.station_fixture
            {
                if required.station_udp_rx_capture || required.station_udp_tx_capture {
                    hil_wifi::fixture::openwrt::evidence::check_capture(config)?;
                }
                if required.laptop_air_monitor {
                    hil_wifi::fixture::local::air_monitor::check_without_device(config, &output)?;
                }
            }
            if required.station_udp_rx_capture
                && let Some(observer) = hil_wifi::fixture::openwrt::air_monitor::Capture::start(
                    lab,
                    None,
                    std::time::Duration::from_secs(1),
                    &output,
                )?
            {
                observer.finish()?;
            }
            Ok(())
        });
    let records = cleanup.finish()?;
    let restored = records.iter().all(|record| record.failure.is_none());
    let report = serde_json::json!({"schema": 1, "scenario": scenario.id, "device_accessed": false,
        "prepared": result.is_ok(), "restored": restored,
        "failure": result.as_ref().err().map(|error| error.to_string()), "cleanup": records});
    hil_core::durable::atomic_json(&output.join("result.json"), &report)?;
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
