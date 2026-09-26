//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use oer_hil_protocol::WifiApScheduler;

use crate::{
    Result, fixture,
    scenario::{Family, Scenario},
};
use hil_core::{
    evidence::run::Failure, image, image::ImageClass, image::Integration, lab::config::LabConfig,
    session::SerialCapture,
};
use hil_wifi::scenario::WifiWorkload;

pub(crate) fn configure_run_selection(
    selected: &mut Scenario,
    ap_scheduler: Option<WifiApScheduler>,
    replay: bool,
    network: Integration,
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
    if selected.plan().settings.ap_scheduler != WifiApScheduler::Disabled
        && !replay
        && network != Integration::OwnedXarxa
    {
        return Err(
            "--ap-scheduler requires --network owned-xarxa or a compatible archived image".into(),
        );
    }
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
