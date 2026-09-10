//! Fixture prerequisites shared by doctor, execution and fixture-only checks.

use crate::{
    Result,
    lab::{
        config::{LabConfig, StationFixtureConfig},
        requirements::Requirements,
    },
    scenario::Scenario,
};

pub(crate) fn check(lab: &LabConfig, scenario: &Scenario) -> Result<()> {
    let resolved = lab.resolve_scenario(scenario);
    let lab = &resolved;
    if let Some(failure) = crate::scenario_precondition(lab, scenario) {
        return Err(super::Error::new(failure.message).into());
    }
    let required = Requirements::for_scenario(scenario);
    if let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture
        && config.read_only
        && (required.station_control || required.openwrt_client || required.openwrt_tx_monitor)
    {
        return Err(super::Error::new(
            "scenario requires mutations forbidden by read-only OpenWrt fixture",
        )
        .into());
    }
    if required.probe_load {
        if !std::path::Path::new("/usr/local/libexec/open-radio-probe").is_file() {
            return Err(
                super::Error::new("probe load requires cargo hil fixture install-host").into(),
            );
        }
        crate::image::require_program(std::ffi::OsStr::new("tshark"))?;
    }
    if required.station_network {
        super::cleanup::require_healthy()?;
    }
    super::network_helper::require_for(lab, required)?;
    if required.station_network {
        match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) => {
                let phy = lab.fixture_phy(scenario);
                super::openwrt_ap::probe(config, super::openwrt_ap::Profile::new(config, phy))
                    .map_err(super::Error::context)?;
                // Both directions consume remote counters and command-line capture tools.
                if required.station_udp_rx_capture || required.station_udp_tx_capture {
                    super::openwrt_fixture::doctor_tools(config)?;
                }
                if matches!(
                    scenario.workload,
                    crate::scenario::Workload::Udp {
                        station_pause: Some(_),
                        ..
                    }
                ) {
                    crate::image::require_program(std::ffi::OsStr::new("tshark"))?;
                }
            }
            StationFixtureConfig::LocalLinux(config) => {
                super::local_ap::check(config, &lab.station, lab.fixture_phy(scenario))
                    .map_err(super::Error::context)?;
                if required.station_udp_rx_capture || required.station_udp_tx_capture {
                    crate::image::require_program(std::ffi::OsStr::new("dumpcap"))?;
                }
            }
            StationFixtureConfig::External(_) if required.station_control => {
                return Err(
                    super::Error::new("scenario requires a controllable AP fixture").into(),
                );
            }
            StationFixtureConfig::External(_) => {}
        }
    }
    if required.laptop_client {
        super::controlled_client::doctor()?;
    }
    if required.openwrt_client {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err(super::Error::new("scenario requires an OpenWrt client").into());
        };
        super::controlled_openwrt_client::doctor(&lab.access_point, config)?;
    }
    if required.openwrt_tx_monitor {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err(super::Error::new("scenario requires an OpenWrt monitor").into());
        };
        super::openwrt_tx_monitor::doctor(config)?;
    }
    if required.laptop_air_monitor {
        super::local_air_monitor::doctor()?;
    }
    Ok(())
}
