//! Fixture prerequisites shared by doctor, execution and fixture-only checks.

use crate::{
    Result,
    scenario::{Family, Scenario},
};
use oer_hil_evidence::run::{Failure, FailureKind};
use oer_hil_stand::config::{LabConfig, StationFixtureConfig};

pub(crate) fn check(lab: &LabConfig, scenario: &Scenario) -> Result<()> {
    let plan = scenario.plan();
    let resolved = lab.resolve(plan.wifi);
    let lab = &resolved;
    if let Some(failure) = scenario_precondition(lab, scenario) {
        return Err(oer_hil_stand::Error::new(failure.message).into());
    }
    let required = plan.requirements;
    if let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture
        && config.read_only
        && (required.station_control || required.openwrt_client || required.openwrt_tx_monitor)
    {
        return Err(oer_hil_stand::Error::new(
            "scenario requires mutations forbidden by read-only OpenWrt fixture",
        )
        .into());
    }
    if required.air_observer && lab.air_observer.is_none() {
        return Err(
            oer_hil_stand::Error::new("scenario requires the independent [air_observer]").into(),
        );
    }
    if (required.non_ht_member || required.legacy_bss)
        && !matches!(lab.station_fixture, StationFixtureConfig::OpenWrt(_))
    {
        return Err(oer_hil_stand::Error::new(
            "induced BSS protection requires the OpenWrt station fixture",
        )
        .into());
    }
    if required.legacy_bss && lab.legacy_bss.is_none() {
        return Err(oer_hil_stand::Error::new(
            "scenario requires the [legacy_bss] laboratory capability",
        )
        .into());
    }
    if required.probe_load {
        if !std::path::Path::new("/usr/local/libexec/open-radio-probe").is_file() {
            return Err(oer_hil_stand::Error::new(
                "probe load requires cargo hil fixture install --provider linux-net",
            )
            .into());
        }
        oer_hil_image::require_program(std::ffi::OsStr::new("tshark"))?;
    }
    if required.station_network {
        oer_hil_execution::fixture::cleanup::require_healthy()?;
    }
    hil_wifi::fixture::local::network_helper::require_for(lab, required)?;
    if required.station_network {
        match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(config) => {
                let phy = lab.fixture_phy(plan.wifi);
                hil_wifi::fixture::openwrt::ap::probe(
                    config,
                    hil_wifi::fixture::openwrt::ap::Profile::new(
                        config,
                        phy,
                        plan.wifi.management_frame_protection,
                        plan.wifi.access_point_security,
                        plan.wifi.access_point_beacon,
                    ),
                )
                .map_err(oer_hil_stand::Error::context)?;
                // Both directions consume remote counters and command-line capture tools.
                if required.station_udp_rx_capture || required.station_udp_tx_capture {
                    hil_wifi::fixture::openwrt::evidence::doctor_tools(config)?;
                }
            }
            StationFixtureConfig::LocalLinux(config) => {
                hil_wifi::fixture::local::ap::check(
                    config,
                    &lab.station,
                    lab.fixture_phy(plan.wifi),
                )
                .map_err(oer_hil_stand::Error::context)?;
                if required.station_udp_rx_capture || required.station_udp_tx_capture {
                    oer_hil_image::require_program(std::ffi::OsStr::new("dumpcap"))?;
                }
            }
            StationFixtureConfig::External(_) if required.station_control => {
                return Err(oer_hil_stand::Error::new(
                    "scenario requires a controllable AP fixture",
                )
                .into());
            }
            StationFixtureConfig::External(_) => {}
        }
    }
    if required.laptop_client {
        hil_wifi::fixture::local::client::doctor()?;
    }
    if required.openwrt_client {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err(oer_hil_stand::Error::new("scenario requires an OpenWrt client").into());
        };
        hil_wifi::fixture::openwrt::client::doctor(&lab.access_point, config)?;
    }
    if required.openwrt_tx_monitor {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err(oer_hil_stand::Error::new("scenario requires an OpenWrt monitor").into());
        };
        hil_wifi::fixture::openwrt::tx_monitor::doctor(config)?;
    }
    if required.laptop_air_monitor {
        hil_wifi::fixture::local::air_monitor::doctor()?;
    }
    Ok(())
}

/// Scenario-specific Bluetooth adapter checks and the station fixture PHY.
pub(crate) fn scenario_precondition(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    let adapter_preflight = match &selected.family {
        Family::Bluetooth(bluetooth) => bluetooth.adapter_preflight(),
        Family::Coexistence(_) => {
            Some(hil_bluetooth::fixture::bluetooth::att::preflight as fn(_) -> crate::Result<()>)
        }
        Family::Wifi(_) | Family::System(_) | Family::Ieee802154(_) => None,
    };
    if let Some(preflight) = adapter_preflight {
        let result = lab
            .bluetooth_adapter
            .ok_or_else(|| "missing [bluetooth] adapter in lab config".into())
            .and_then(preflight);
        if let Err(error) = result {
            return Some(Failure::new(FailureKind::Precondition, error.to_string()));
        }
    }
    let plan = selected.plan();
    if let Some(image) = selected.family.peer_image()
        && let Some(peer) = &lab.peer
        && let Err(error) = peer.serial().and_then(|serial| {
            oer_hil_stand::lock::require_board_image(&serial, image.name, image.reflash)
        })
    {
        return Some(Failure::new(FailureKind::Precondition, error.to_string()));
    }
    if !plan.requirements.station_network {
        return None;
    }
    lab.station_fixture
        .require_phy(lab.fixture_phy(plan.wifi))
        .err()
        .map(|error| Failure::new(FailureKind::Precondition, error.to_string()))
}
