//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use open_esp_radio_hil_protocol::WifiApScheduler;

use crate::{
    Result,
    evidence::run::{Failure, FailureKind},
    fixture,
    image::{self, ImageClass, Integration},
    lab::{config::LabConfig, requirements::Requirements},
    scenario::{Scenario, Workload},
    session::SerialCapture,
};

pub(crate) fn configure_run_selection(
    selected: &mut Scenario,
    ap_scheduler: Option<WifiApScheduler>,
    replay: bool,
    network: Integration,
) -> Result<()> {
    if let Some(policy) = ap_scheduler {
        selected.ap_scheduler = policy;
    }
    selected.validate()?;
    if selected.ap_scheduler != WifiApScheduler::Disabled {
        if !matches!(selected.workload, Workload::AccessPoint { .. }) {
            return Err("--ap-scheduler requires a standalone access-point scenario".into());
        }
        if !replay && network != Integration::OwnedXarxa {
            return Err(
                "--ap-scheduler requires --network owned-xarxa or a compatible archived image"
                    .into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn scenario_failure(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    scenario_precondition(lab, selected).or_else(|| {
        fixture::preflight::check(lab, selected)
            .err()
            .map(|error| super::classify(&*error))
    })
}

pub(crate) fn scenario_precondition(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    let bluetooth_preflight: Option<fn(fixture::bluetooth::model::Adapter) -> Result<()>> =
        match selected.workload {
            Workload::BluetoothSecureGatt | Workload::BluetoothSecureGattHciReadFailure => {
                Some(crate::workload::bluetooth::secure_gatt::preflight)
            }
            Workload::BluetoothAclCalibration { .. } => {
                Some(fixture::bluetooth::att_parameters::preflight)
            }
            Workload::BluetoothGatt | Workload::BluetoothAclBackpressure { .. } => {
                Some(fixture::bluetooth::att::preflight)
            }
            Workload::BluetoothDtm { .. }
            | Workload::BluetoothWatchdogReset
            | Workload::BluetoothMaintenanceDeadline => Some(fixture::bluetooth::preflight),
            Workload::BluetoothPeripheral { .. }
            | Workload::BluetoothEncryptedAcl { .. }
            | Workload::BluetoothPhyWatchdog => Some(fixture::bluetooth::preflight_connect_reset),
            Workload::BluetoothSecurityFailure { .. } => {
                Some(fixture::bluetooth::preflight_security_failure)
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

pub(crate) fn validate_flashed_image(
    lab: &LabConfig,
    expected: ImageClass,
    output: &Path,
) -> Result<()> {
    if expected == ImageClass::BootSmoke {
        return Ok(());
    }
    let capture =
        SerialCapture::start_with_reset(&lab.device.serial, &output.join("image-preflight"))?;
    let capabilities = capture.request_capabilities(Duration::from_secs(10));
    let capabilities = capture.finish_with(capabilities)?;

    let observed = image::classify_flashed_capabilities(&capabilities.features)
        .ok_or("flashed image advertises mutually exclusive diagnostic capabilities")?;
    if observed != expected {
        return Err(format!(
            "scenario requires `{}` image but flashed target advertises `{}` capabilities",
            expected.id(),
            observed.id()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
