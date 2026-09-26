//! Host validation of the single-device IEEE 802.15.4 on-air check.
//!
//! Each cycle starts IEEE 802.15.4 as a client of the shared radio arbiter,
//! runs an energy scan, a clear-channel assessment, a direct transmit without
//! an acknowledgement request, two transmits scheduled through the MAC timer
//! and ETM route, and a receive window, then stops the client. The accepted
//! result proves that the composed client starts, completes each operation
//! with a terminal event and stops again, and that scheduled transmits neither
//! start early nor miss their window. No peer observes the air.

use hil_core::{context::Context, session::SerialCapture};
use std::{fs, path::Path, time::Duration};

use oer_hil_protocol::{
    Ieee802154AirCcaOutcome, Ieee802154AirCheckEvidence, Ieee802154AirCheckRequest,
    Ieee802154AirCheckStop, Ieee802154AirCycle, Ieee802154AirEnergyOutcome, Ieee802154AirTransmit,
    Ieee802154AirTxOutcome,
};
use serde::Serialize;

use crate::Result;

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const RESULT: &str = "arbiter-client-start-operations-stop";
const REPORT_NAME: &str = "ieee802154-air-check.json";

/// Longest accepted delay from a scheduled start to its completion event:
/// the frame's airtime at 250 kbit/s plus event delivery to the task.
pub const SCHEDULED_COMPLETION_BOUND_MICROS: u64 = 10_000;

/// Plausible range of an averaged ED reading in dBm.
const ENERGY_RANGE_DBM: core::ops::RangeInclusive<i8> = -110..=10;

pub struct Config {
    pub boots: u8,
    pub request: Ieee802154AirCheckRequest,
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    target: Ieee802154AirCheckEvidence,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    fs::create_dir_all(output)?;
    if !config.request.validate() {
        return Err("invalid IEEE 802.15.4 air check bounds".into());
    }
    let timeout = command_timeout(config.request);
    let mut reports = Vec::with_capacity(usize::from(config.boots));
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let result = context.with_capture(&boot_output, |capture| {
            check(capture, config.request, timeout)
        });
        let target = match result {
            Ok(target) => target,
            Err(error) => {
                write_report(output, &reports, Some(&error.to_string()))?;
                return Err(error);
            }
        };
        reports.push(BootReport { boot, target });
        if let Err(error) = validate(target, config.request) {
            write_report(output, &reports, Some(&error.to_string()))?;
            return Err(error);
        }
    }
    write_report(output, &reports, None)?;
    println!(
        "ieee802154_air_check=PASS result={RESULT} boots={} cycles={}",
        config.boots, config.request.cycles
    );
    Ok(())
}

/// Every cycle runs its bounded operations plus bring-up, which may register
/// and calibrate the shared PHY.
fn command_timeout(request: Ieee802154AirCheckRequest) -> Duration {
    let per_cycle = Duration::from_secs(5)
        + Duration::from_millis(u64::from(request.receive_window_millis))
        + Duration::from_micros(2 * u64::from(request.scheduled_lead_micros));
    Duration::from_secs(20) + per_cycle * u32::from(request.cycles)
}

fn check(
    capture: &SerialCapture,
    request: Ieee802154AirCheckRequest,
    timeout: Duration,
) -> Result<Ieee802154AirCheckEvidence> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.features.ieee802154_air_check {
        return Err("firmware does not advertise the IEEE 802.15.4 air check".into());
    }
    capture.run_ieee802154_air_check(request, timeout)
}

fn write_report(output: &Path, reports: &[BootReport], failure: Option<&str>) -> Result<()> {
    let document = match failure {
        None => serde_json::json!({
            "schema": 1,
            "status": "passed",
            "result": RESULT,
            "not_proven": [
                "peer-reception-of-transmitted-frames",
                "acknowledged-exchange",
                "calibrated-output-power",
                "calibrated-energy-or-rssi",
                "receive-filtering-with-traffic",
                "periodic-phy-tracking-while-running",
            ],
            "boots": reports,
        }),
        Some(failure) => serde_json::json!({
            "schema": 1,
            "status": "failed",
            "result": "incomplete",
            "failure": failure,
            "boots": reports,
        }),
    };
    fs::write(
        output.join(REPORT_NAME),
        serde_json::to_vec_pretty(&document)?,
    )?;
    Ok(())
}

fn validate(
    evidence: Ieee802154AirCheckEvidence,
    request: Ieee802154AirCheckRequest,
) -> Result<()> {
    if evidence.stop != Ieee802154AirCheckStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 air check stopped at {:?} after {} cycles",
            evidence.stop, evidence.completed_cycles
        )
        .into());
    }
    if evidence.completed_cycles != request.cycles {
        return Err(format!(
            "IEEE 802.15.4 air check completed {} of {} cycles",
            evidence.completed_cycles, request.cycles
        )
        .into());
    }
    for (index, cycle) in evidence.cycles[..usize::from(request.cycles)]
        .iter()
        .enumerate()
    {
        validate_cycle(index + 1, cycle)?;
    }
    Ok(())
}

fn validate_cycle(cycle_number: usize, cycle: &Ieee802154AirCycle) -> Result<()> {
    let fail = |what: String| -> Result<()> {
        Err(format!("IEEE 802.15.4 air check cycle {cycle_number}: {what}").into())
    };
    match cycle.energy {
        Ieee802154AirEnergyOutcome::Energy(dbm) if ENERGY_RANGE_DBM.contains(&dbm) => {}
        other => return fail(format!("energy scan {other:?}")),
    }
    if !matches!(
        cycle.cca,
        Ieee802154AirCcaOutcome::Clear | Ieee802154AirCcaOutcome::Busy
    ) {
        return fail(format!("clear-channel assessment {:?}", cycle.cca));
    }
    validate_transmit(&cycle.direct).or_else(|what| fail(format!("direct transmit {what}")))?;
    for (index, scheduled) in cycle.scheduled.iter().enumerate() {
        validate_transmit(scheduled)
            .or_else(|what| fail(format!("scheduled transmit {} {what}", index + 1)))?;
        let late = scheduled.done_at_micros - scheduled.requested_at_micros;
        if late > SCHEDULED_COMPLETION_BOUND_MICROS {
            return fail(format!(
                "scheduled transmit {} completed {late} us after its start",
                index + 1
            ));
        }
    }
    if cycle.scheduled[1].requested_at_micros <= cycle.scheduled[0].done_at_micros {
        return fail("scheduled transmits overlap".into());
    }
    Ok(())
}

fn validate_transmit(transmit: &Ieee802154AirTransmit) -> core::result::Result<(), String> {
    if transmit.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!("ended {:?}", transmit.outcome));
    }
    if transmit.done_at_micros < transmit.requested_at_micros {
        return Err(format!(
            "completed at {} before its start {}",
            transmit.done_at_micros, transmit.requested_at_micros
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
