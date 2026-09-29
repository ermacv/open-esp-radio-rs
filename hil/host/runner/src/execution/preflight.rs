//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use hil_core::scenario::ScenarioFamily as _;

use crate::{Result, fixture, scenario::Scenario};
use hil_core::{
    evidence::run::Failure,
    image,
    image::ImageClass,
    lab::config::LabConfig,
    session::{DeviceImageKeys, SerialCapture},
};

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
    let preflight = output.join("image-preflight");
    let error = match image_keys_of(lab, &preflight) {
        Ok(image_keys) => {
            return check_flashed_image_keys(lab.target(), selected, &image_keys);
        }
        Err(error) => error,
    };
    // A bootloader that resets in a loop keeps state an RTS reset and a
    // reflash leave: climb to the resets that clear it.
    let console = std::fs::read_to_string(preflight.join("uart.log")).unwrap_or_default();
    let Some(found) = hil_core::recovery::boot_loop(&console) else {
        return Err(error);
    };
    eprintln!(
        "hil: the bootloader resets in a loop ({}); escalating the reset",
        found.reset_line
    );
    let origin = output
        .ancestors()
        .find(|directory| directory.join("manifest.json").is_file())
        .and_then(|run| run.file_name())
        .map_or_else(String::new, |run| run.to_string_lossy().into_owned());
    let mut attempt = 0;
    let mut answered = None;
    let escalation = hil_core::recovery::escalate_boot_loop(
        &lab.device.serial,
        None,
        lab.target(),
        found,
        output,
        &origin,
        || {
            attempt += 1;
            let directory = output.join(format!("image-preflight-retry-{attempt}"));
            answered = image_keys_of(lab, &directory).ok();
            answered.is_some()
        },
    );
    match answered {
        Some(image_keys) => {
            eprintln!(
                "hil: {:?} cleared the boot loop",
                escalation.steps.last().map(|step| step.step)
            );
            check_flashed_image_keys(lab.target(), selected, &image_keys)
        }
        None => Err(format!(
            "{error}; the bootloader reset in a loop ({}) and {} did not clear it: the board is \
             quarantined for a person (see {})",
            escalation.boot_loop.reset_line,
            escalation
                .steps
                .iter()
                .map(|step| format!("{:?}", step.step))
                .collect::<Vec<_>>()
                .join(", "),
            hil_core::recovery::RESET_ESCALATION_FILE
        )
        .into()),
    }
}

/// Reset the device and ask its image for its image keys, capturing the
/// console into `directory`.
fn image_keys_of(lab: &LabConfig, directory: &Path) -> Result<DeviceImageKeys> {
    let capture = SerialCapture::start_with_reset(&lab.device.serial, directory)?;
    let image_keys = capture.request_image_keys(Duration::from_secs(10));
    capture.finish_with(image_keys)
}

/// Accept a flashed image only when it is the scenario's class and reports
/// every role the scenario drives.
fn check_flashed_image_keys(
    chip: &str,
    selected: &Scenario,
    image_keys: &DeviceImageKeys,
) -> Result<()> {
    let expected = selected.image();
    let observed = image::classify_flashed(chip, image_keys).ok_or_else(|| {
        format!("the flashed image reports image keys no {chip} image class builds")
    })?;
    if observed != expected {
        return Err(format!(
            "scenario requires `{}` image but flashed target advertises the image keys of `{}`",
            expected.id(),
            observed.id()
        )
        .into());
    }
    if !selected.family.served_by(image_keys) {
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
