//! Bounded host validation for the IEEE 802.15.4 `EVENT_STATUS` HIL probe.
//!
//! The accepted result observes selective acknowledgement of two enabled
//! timer bits and a later, distinct arrival of the second timer bit while the
//! source-132 CPU routes retain their reset-detached controls. It does not
//! prove full W1C semantics, concurrent arrival of the same bit, or active
//! level-triggered interrupt-route behavior.

use std::{fs, path::Path, time::Duration};

use open_esp_radio_hil_protocol::{
    Ieee802154EventStatusProbeEvidence, Ieee802154EventStatusProbeRequest,
    Ieee802154EventStatusProbeStop, Ieee802154ObservedEventState,
    Ieee802154ValidationEventEnableState,
};
use serde::Serialize;

use crate::{Result, lab::config::LabConfig, session::SerialCapture};

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const RESULT: &str = "route-detached-enabled-selective-ack-and-distinct-arrival-observed";

pub(crate) struct Config {
    pub(crate) boots: u8,
    pub(crate) poll_limit: u32,
    pub(crate) timer_threshold: u32,
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    target: Ieee802154EventStatusProbeEvidence,
}

pub(crate) fn run(config: Config, output: &Path, lab: &LabConfig) -> Result<()> {
    fs::create_dir_all(output)?;
    let request = Ieee802154EventStatusProbeRequest {
        poll_limit: config.poll_limit,
        timer_threshold: config.timer_threshold,
    };
    if !request.validate() {
        return Err("invalid IEEE 802.15.4 EVENT_STATUS probe bounds".into());
    }

    let mut reports = Vec::with_capacity(usize::from(config.boots));
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let capture = SerialCapture::start_with_reset(&lab.device.serial);
        let result = probe(&capture, request, boot);
        let capture_result = capture.finish_to(&boot_output);
        let report = result?;
        let target = report.target;
        reports.push(report);
        if let Err(error) = capture_result {
            write_failed_report(output, &reports, &error.to_string())?;
            return Err(error);
        }
        if let Err(error) = validate(target) {
            write_failed_report(output, &reports, &error.to_string())?;
            return Err(error);
        }
    }

    fs::write(
        output.join("ieee802154-event-status.json"),
        serde_json::to_vec_pretty(&report_document(&reports))?,
    )?;
    eprintln!(
        "ieee802154_event_status=PASS result={RESULT} boots={}",
        config.boots
    );
    Ok(())
}

fn write_failed_report(output: &Path, reports: &[BootReport], failure: &str) -> Result<()> {
    fs::write(
        output.join("ieee802154-event-status.json"),
        serde_json::to_vec_pretty(&failed_report_document(reports, failure))?,
    )?;
    Ok(())
}

fn report_document(reports: &[BootReport]) -> serde_json::Value {
    serde_json::json!({
        "schema": 2,
        "status": "passed",
        "result": RESULT,
        "not_proven": [
            "full-w1c-semantics",
            "event-enable-generation-vs-visibility-semantics",
            "concurrent-same-bit-arrival",
            "level-triggered-route-behavior",
            "masked-final-status-means-physical-cleanup",
        ],
        "boots": reports,
    })
}

fn failed_report_document(reports: &[BootReport], failure: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": 2,
        "status": "failed",
        "result": "incomplete",
        "failure": failure,
        "boots": reports,
    })
}

fn probe(
    capture: &SerialCapture,
    request: Ieee802154EventStatusProbeRequest,
    boot: u8,
) -> Result<BootReport> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.features.ieee802154_event_status_probe {
        return Err("firmware does not advertise the IEEE 802.15.4 EVENT_STATUS probe".into());
    }
    let target = capture.probe_ieee802154_event_status(request, COMMAND_TIMEOUT)?;
    Ok(BootReport { boot, target })
}

fn validate(evidence: Ieee802154EventStatusProbeEvidence) -> Result<()> {
    if evidence.stop != Ieee802154EventStatusProbeStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 EVENT_STATUS probe stopped at {:?}: {evidence:?}",
            evidence.stop,
        )
        .into());
    }
    if evidence.event_enable_before != Ieee802154ValidationEventEnableState::AllMasked {
        return checkpoint_error(
            "event_enable_before",
            evidence.event_enable_before,
            "be AllMasked",
        );
    }
    if evidence.event_enable_active != Ieee802154ValidationEventEnableState::TimerPairOnly {
        return checkpoint_error(
            "event_enable_active",
            evidence.event_enable_active,
            "be TimerPairOnly",
        );
    }
    if evidence.event_enable_after != Ieee802154ValidationEventEnableState::AllMasked {
        return checkpoint_error(
            "event_enable_after",
            evidence.event_enable_after,
            "be AllMasked",
        );
    }

    for (checkpoint, observed, expected) in [
        (
            "reset_events",
            evidence.reset_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "post_enable_events",
            evidence.post_enable_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "dual_observed_events",
            evidence.dual_observed_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "dual_latched_events",
            evidence.dual_latched_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "after_timer0_ack_events",
            evidence.after_timer0_ack_events,
            Ieee802154ObservedEventState::Timer1Only,
        ),
        (
            "after_timer1_ack_events",
            evidence.after_timer1_ack_events,
            Ieee802154ObservedEventState::Clear,
        ),
        (
            "distinct_snapshot_events",
            evidence.distinct_snapshot_events,
            Ieee802154ObservedEventState::Timer0Only,
        ),
        (
            "distinct_before_ack_events",
            evidence.distinct_before_ack_events,
            Ieee802154ObservedEventState::Timer0AndTimer1,
        ),
        (
            "distinct_after_ack_events",
            evidence.distinct_after_ack_events,
            Ieee802154ObservedEventState::Timer1Only,
        ),
        (
            "final_events",
            evidence.final_events,
            Ieee802154ObservedEventState::Clear,
        ),
    ] {
        if observed != expected {
            return checkpoint_error(checkpoint, observed, "equal its exact semantic state");
        }
    }

    if evidence.cleanup_pending_events != Ieee802154ObservedEventState::Clear
        && evidence.cleanup_pending_events != Ieee802154ObservedEventState::Timer1Only
    {
        return checkpoint_error(
            "cleanup_pending_events",
            evidence.cleanup_pending_events,
            "be Clear or Timer1Only after delivery is masked",
        );
    }

    for (timer, before, minimum, maximum) in [
        (
            "timer0",
            evidence.timer0_value_before_start,
            evidence.timer0_value_min,
            evidence.timer0_value_max,
        ),
        (
            "timer1",
            evidence.timer1_value_before_start,
            evidence.timer1_value_min,
            evidence.timer1_value_max,
        ),
    ] {
        if minimum >= maximum || before < minimum || before > maximum {
            return Err(format!(
                "IEEE 802.15.4 EVENT_STATUS {timer} counter did not show bounded activity: before={before}, min={minimum}, max={maximum}"
            )
            .into());
        }
    }
    Ok(())
}

fn checkpoint_error<T, Observation: core::fmt::Debug>(
    checkpoint: &str,
    observed: Observation,
    expected: &str,
) -> Result<T> {
    Err(
        format!("IEEE 802.15.4 EVENT_STATUS {checkpoint}={observed:?}, expected it to {expected}")
            .into(),
    )
}

#[cfg(test)]
mod tests;
