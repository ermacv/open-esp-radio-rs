//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use hil_core::scenario::ScenarioFamily as _;
use oer_hil_protocol::{FeatureCapabilities, WifiApScheduler};

use crate::{
    Result, fixture,
    scenario::{Family, Scenario},
};
use hil_core::{
    evidence::run::Failure, image, image::ImageClass, lab::config::LabConfig,
    session::SerialCapture,
};
use hil_wifi::scenario::WifiWorkload;

pub(crate) fn configure_run_selection(
    selected: &mut Scenario,
    ap_scheduler: Option<WifiApScheduler>,
) -> Result<()> {
    if let Some(policy) = ap_scheduler {
        let Family::Wifi(wifi) = &mut selected.family else {
            return Err("--ap-scheduler requires a standalone access-point scenario".into());
        };
        let WifiWorkload::AccessPoint(access_point) = &mut wifi.workload else {
            return Err("--ap-scheduler requires a standalone access-point scenario".into());
        };
        access_point.scheduler = policy;
    }
    selected.validate()?;
    Ok(())
}

pub(crate) fn scenario_failure(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    fixture::preflight::scenario_precondition(lab, selected).or_else(|| {
        fixture::preflight::check(lab, selected)
            .err()
            .map(|error| super::classify(&*error))
    })
}

pub(crate) fn validate_flashed_image(
    lab: &LabConfig,
    selected: &Scenario,
    output: &Path,
) -> Result<()> {
    if selected.image() == ImageClass::BootSmoke {
        return Ok(());
    }
    let capture =
        SerialCapture::start_with_reset(&lab.device.serial, &output.join("image-preflight"))?;
    let capabilities = capture.request_capabilities(Duration::from_secs(10));
    let capabilities = capture.finish_with(capabilities)?;
    check_flashed_capabilities(selected, &capabilities.features)
}

/// Accept a flashed image only when it is the scenario's class and declares
/// every role the scenario drives.
fn check_flashed_capabilities(selected: &Scenario, features: &FeatureCapabilities) -> Result<()> {
    let expected = selected.image();
    let observed = image::classify_flashed_capabilities(features)
        .ok_or("flashed image advertises mutually exclusive diagnostic capabilities")?;
    if observed != expected {
        return Err(format!(
            "scenario requires `{}` image but flashed target advertises `{}` capabilities",
            expected.id(),
            observed.id()
        )
        .into());
    }
    if !selected.family.served_by(features) {
        return Err(format!(
            "flashed `{}` image does not declare a role scenario `{}` drives",
            observed.id(),
            selected.id()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
