//! Independent agreement check between target alarms and its monotonic clock.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{TimebaseProbeEvidence, TimebaseProbeRequest};
use serde::Serialize;

use crate::{Result, lab::config::LabConfig, session::SerialCapture};

const COMMAND_SLACK: Duration = Duration::from_secs(5);

pub(crate) struct Config {
    pub(crate) boots: u8,
    pub(crate) intervals: u16,
    pub(crate) period_millis: u16,
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    host_elapsed_micros: u64,
    target: TimebaseProbeEvidence,
}

pub(crate) fn run(config: Config, output: &Path, lab: &LabConfig) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut reports = Vec::with_capacity(usize::from(config.boots));
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let capture = SerialCapture::start_with_reset(&lab.device.serial);
        let result = probe(&capture, &config, boot);
        let capture_result = capture.finish_to(&boot_output);
        let report = result?;
        capture_result?;
        reports.push(report);
    }
    fs::write(
        output.join("timebase.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1,
            "status": "passed",
            "boots": reports,
        }))?,
    )?;
    eprintln!("timebase=PASS boots={}", config.boots);
    Ok(())
}

fn probe(capture: &SerialCapture, config: &Config, boot: u8) -> Result<BootReport> {
    let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
    if !capabilities.features.timebase_probe {
        return Err("firmware does not advertise the timebase probe".into());
    }
    let period_micros = u32::from(config.period_millis) * 1_000;
    let request = TimebaseProbeRequest {
        intervals: config.intervals,
        period_micros,
    };
    let expected_micros = u64::from(period_micros) * u64::from(config.intervals);
    let started = Instant::now();
    let target = capture.probe_timebase(
        request,
        Duration::from_micros(expected_micros).saturating_add(COMMAND_SLACK),
    )?;
    let host_elapsed_micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    validate(target, host_elapsed_micros, expected_micros)?;
    Ok(BootReport {
        boot,
        host_elapsed_micros,
        target,
    })
}

fn validate(
    evidence: TimebaseProbeEvidence,
    host_elapsed_micros: u64,
    expected_micros: u64,
) -> Result<()> {
    let device_min = expected_micros * 95 / 100;
    let device_max = expected_micros * 110 / 100;
    let host_min = expected_micros * 95 / 100;
    let host_max = expected_micros * 120 / 100;
    let interval_min = u64::from(evidence.period_micros) * 95 / 100;
    let interval_max = u64::from(evidence.period_micros) * 150 / 100;
    if evidence.elapsed_micros < device_min
        || evidence.elapsed_micros > device_max
        || u64::from(evidence.minimum_interval_micros) < interval_min
        || u64::from(evidence.maximum_interval_micros) > interval_max
        || evidence.early_intervals != 0
        || host_elapsed_micros < host_min
        || host_elapsed_micros > host_max
    {
        return Err(format!(
            "timebase agreement failed: expected_us={expected_micros} host_us={host_elapsed_micros} target={evidence:?}"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
