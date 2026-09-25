//! Fixture prerequisites shared by doctor, execution and fixture-only checks.

use crate::{
    Result,
    evidence::run::{Failure, FailureKind},
    lab::{
        config::{LabConfig, StationFixtureConfig},
        requirements::Requirements,
    },
    scenario::{Scenario, Workload},
};

pub(crate) fn check(lab: &LabConfig, scenario: &Scenario) -> Result<()> {
    let resolved = lab.resolve_scenario(scenario);
    let lab = &resolved;
    if let Some(failure) = scenario_precondition(lab, scenario) {
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
            return Err(super::Error::new(
                "probe load requires cargo hil fixture install --provider linux-net",
            )
            .into());
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

/// Scenario-specific Bluetooth adapter checks and the station fixture PHY.
pub(crate) fn scenario_precondition(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    let bluetooth_preflight: Option<fn(super::bluetooth::model::Adapter) -> Result<()>> =
        match selected.workload {
            Workload::BluetoothSecureGatt
            | Workload::BluetoothSecureGattTiming
            | Workload::BluetoothSecureGattHciReadFailure => Some(super::bluetooth::att::preflight),
            Workload::BluetoothAclCalibration { .. } => {
                Some(super::bluetooth::att_parameters::preflight)
            }
            Workload::BluetoothGatt | Workload::BluetoothAclBackpressure { .. } => {
                Some(super::bluetooth::att::preflight)
            }
            Workload::BluetoothDtm { .. }
            | Workload::BluetoothWatchdogReset
            | Workload::BluetoothMaintenanceDeadline => Some(super::bluetooth::preflight),
            Workload::BluetoothPeripheral { .. }
            | Workload::BluetoothEncryptedAcl { .. }
            | Workload::BluetoothPhyWatchdog => Some(super::bluetooth::preflight_connect_reset),
            Workload::BluetoothSecurityFailure { .. } => {
                Some(super::bluetooth::preflight_security_failure)
            }
            _ => None,
        };
    if let Some(preflight) = bluetooth_preflight {
        let result = lab
            .bluetooth_adapter
            .ok_or_else(|| "missing [bluetooth] adapter in lab config".into())
            .and_then(preflight);
        if let Err(error) = result {
            return Some(Failure::new(FailureKind::Precondition, error.to_string()));
        }
    }
    if !Requirements::for_scenario(selected).station_network {
        return None;
    }
    lab.station_fixture
        .require_phy(lab.fixture_phy(selected))
        .err()
        .map(|error| Failure::new(FailureKind::Precondition, error.to_string()))
}
