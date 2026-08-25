//! Bounded host validation for the IEEE 802.15.4 ED-DONE/TIMER0 probe.
//!
//! The accepted result observes one exact selected-write relation in a
//! reset-isolated, route-detached transaction. It does not classify the full
//! `EVENT_STATUS` register or claim operational PHY/RF/BTBB readiness.

use std::{fs, path::Path, time::Duration};

use open_esp_radio_hil_protocol::{
    Ieee802154EdEventProbeEvidence, Ieee802154EdEventProbeRequest, Ieee802154EdEventProbeStop,
    Ieee802154PolledEdOutcome,
};
use serde::Serialize;

use crate::{Result, evidence::traffic_capture::SerialCapture, transport::lab_config::LabConfig};

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const RX_ABORT_EVENT: u16 = 1 << 4;
const ED_DONE_EVENT: u16 = 1 << 6;
const TIMER0_EVENT: u16 = 1 << 8;
const ED_TIMER_EVENTS: u16 = ED_DONE_EVENT | TIMER0_EVENT;
const VALIDATION_EVENTS: u16 = RX_ABORT_EVENT | ED_TIMER_EVENTS;
const ED_ABORT_REASONS: u32 = (1 << 23) | (1 << 24) | (1 << 25);
const PUBLIC_EVENT_MASK: u16 = 0x3fff;
const ED_DURATION: u32 = 8;
const RESULT: &str = "back-to-back-production-ed-and-selected-write-recovery";
const REPORT_NAME: &str = "ieee802154-ed-event.json";

pub(crate) struct Config {
    pub(crate) boots: u8,
    pub(crate) poll_limit: u32,
    pub(crate) timer_threshold: u32,
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    target: Ieee802154EdEventProbeEvidence,
}

pub(crate) fn run(config: Config, output: &Path, lab: &LabConfig) -> Result<()> {
    fs::create_dir_all(output)?;
    let request = Ieee802154EdEventProbeRequest {
        poll_limit: config.poll_limit,
        timer_threshold: config.timer_threshold,
    };
    if !request.validate() {
        return Err("invalid IEEE 802.15.4 ED event probe bounds".into());
    }

    let mut reports = Vec::with_capacity(usize::from(config.boots));
    for boot in 1..=config.boots {
        let boot_output = output.join(format!("boot-{boot:03}"));
        let capture = SerialCapture::start_with_reset(&lab.device.serial);
        let probe_result = probe(&capture, request, boot);
        let capture_result = capture.finish_to(&boot_output);

        let report = match probe_result {
            Ok(report) => report,
            Err(error) => {
                let failure = error.to_string();
                write_failed_report(output, &reports, &failure)?;
                capture_result?;
                return Err(error);
            }
        };
        let target = report.target;
        reports.push(report);
        if let Err(error) = capture_result {
            write_failed_report(output, &reports, &error.to_string())?;
            return Err(error);
        }
        if let Err(error) = validate(target, config.poll_limit) {
            write_failed_report(output, &reports, &error.to_string())?;
            return Err(error);
        }
    }

    fs::write(
        output.join(REPORT_NAME),
        serde_json::to_vec_pretty(&report_document(&reports))?,
    )?;
    println!(
        "ieee802154_ed_event=PASS result={RESULT} boots={}",
        config.boots
    );
    Ok(())
}

fn write_failed_report(output: &Path, reports: &[BootReport], failure: &str) -> Result<()> {
    fs::write(
        output.join(REPORT_NAME),
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
            "full-register-w1c-semantics",
            "non-ed-event-write-semantics",
            "concurrent-same-bit-arrival",
            "level-triggered-route-behavior",
            "production-phy-rf-btbb-readiness",
            "synchronous-stop-semantics",
            "calibrated-rss-or-dbm-conversion",
            "rf-channel-retune",
        ],
        "boots": reports,
    })
}

fn failed_report_document(reports: &[BootReport], failure: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "status": "failed",
        "result": "incomplete",
        "failure": failure,
        "boots": reports,
    })
}

fn probe(
    capture: &SerialCapture,
    request: Ieee802154EdEventProbeRequest,
    boot: u8,
) -> Result<BootReport> {
    let capabilities = capture.request_capabilities(CAPABILITIES_TIMEOUT)?;
    if !capabilities.features.ieee802154_ed_event_probe {
        return Err("firmware does not advertise the IEEE 802.15.4 ED event probe".into());
    }
    let target = capture.probe_ieee802154_ed_event(request, COMMAND_TIMEOUT)?;
    Ok(BootReport { boot, target })
}

fn validate(evidence: Ieee802154EdEventProbeEvidence, poll_limit: u32) -> Result<()> {
    let validate_production =
        |attempt: &str, outcome: Ieee802154PolledEdOutcome| -> Result<()> {
            match outcome {
        Ieee802154PolledEdOutcome::Complete { polls, .. } if polls >= 1 && polls <= poll_limit => {
            Ok(())
        }
        Ieee802154PolledEdOutcome::Complete { polls, .. } => Err(format!(
            "IEEE 802.15.4 {attempt} production ED used {polls} polls outside 1..={poll_limit}"
        )
        .into()),
        failure => Err(format!("IEEE 802.15.4 {attempt} production ED failed: {failure:?}").into()),
            }
        };
    validate_production("first", evidence.production_ed_first)?;
    let Some(second) = evidence.production_ed_second else {
        return Err("IEEE 802.15.4 second production ED was not run".into());
    };
    validate_production("second", second)?;

    for (checkpoint, events) in [
        ("reset_events", evidence.reset_events),
        ("post_enable_events", evidence.post_enable_events),
        ("observed_events", evidence.observed_events),
        ("terminal_events", evidence.terminal_events),
        (
            "after_ed_done_write_events",
            evidence.after_ed_done_write_events,
        ),
        (
            "after_timer0_write_events",
            evidence.after_timer0_write_events,
        ),
        ("cleanup_pending_events", evidence.cleanup_pending_events),
        ("final_events", evidence.final_events),
    ] {
        let unsupported = events & !PUBLIC_EVENT_MASK;
        if unsupported != 0 {
            return Err(format!(
                "IEEE 802.15.4 ED event {checkpoint} contains unsupported bits {unsupported:#06x}"
            )
            .into());
        }
    }

    if evidence.stop != Ieee802154EdEventProbeStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 ED event probe stopped at {:?}: {evidence:?}",
            evidence.stop,
        )
        .into());
    }
    for (checkpoint, observed, expected) in [
        ("event_enable_before", evidence.event_enable_before, 0),
        (
            "event_enable_active",
            evidence.event_enable_active,
            VALIDATION_EVENTS,
        ),
        ("event_enable_after", evidence.event_enable_after, 0),
        ("reset_events", evidence.reset_events, 0),
        ("post_enable_events", evidence.post_enable_events, 0),
        ("observed_events", evidence.observed_events, ED_TIMER_EVENTS),
        ("terminal_events", evidence.terminal_events, ED_TIMER_EVENTS),
        (
            "after_ed_done_write_events",
            evidence.after_ed_done_write_events,
            TIMER0_EVENT,
        ),
        (
            "after_timer0_write_events",
            evidence.after_timer0_write_events,
            0,
        ),
        ("cleanup_pending_events", evidence.cleanup_pending_events, 0),
        ("final_events", evidence.final_events, 0),
    ] {
        if observed != expected {
            return event_checkpoint_error(checkpoint, observed, expected);
        }
    }

    if evidence.rx_abort_enable_active != ED_ABORT_REASONS {
        return word_checkpoint_error(
            "rx_abort_enable_active",
            evidence.rx_abort_enable_active,
            ED_ABORT_REASONS,
        );
    }
    if evidence.rx_abort_enable_before != 0 {
        return word_checkpoint_error("rx_abort_enable_before", evidence.rx_abort_enable_before, 0);
    }
    if evidence.rx_abort_enable_after != evidence.rx_abort_enable_before {
        return word_checkpoint_error(
            "rx_abort_enable_after",
            evidence.rx_abort_enable_after,
            evidence.rx_abort_enable_before,
        );
    }
    if evidence.ed_duration_active != ED_DURATION {
        return word_checkpoint_error(
            "ed_duration_active",
            evidence.ed_duration_active,
            ED_DURATION,
        );
    }
    if evidence.ed_duration_after != ED_DURATION {
        return word_checkpoint_error("ed_duration_after", evidence.ed_duration_after, ED_DURATION);
    }

    for (checkpoint, route_word) in [
        (
            "route_core0_before_enable",
            evidence.route_core0_before_enable,
        ),
        (
            "route_core1_before_enable",
            evidence.route_core1_before_enable,
        ),
        (
            "route_core0_with_events_enabled",
            evidence.route_core0_with_events_enabled,
        ),
        (
            "route_core1_with_events_enabled",
            evidence.route_core1_with_events_enabled,
        ),
        (
            "route_core0_after_cleanup",
            evidence.route_core0_after_cleanup,
        ),
        (
            "route_core1_after_cleanup",
            evidence.route_core1_after_cleanup,
        ),
    ] {
        if route_word != 0 {
            return word_checkpoint_error(checkpoint, route_word, 0);
        }
    }

    // The HAL terminal state proves only the selected-write relation. HIL
    // additionally qualifies that TIMER0 showed bounded activity while the
    // paired ED-DONE/TIMER0 image was acquired on this physical target.
    if evidence.timer0_value_min >= evidence.timer0_value_max
        || evidence.timer0_value_before_start < evidence.timer0_value_min
        || evidence.timer0_value_before_start > evidence.timer0_value_max
    {
        return Err(format!(
            "IEEE 802.15.4 ED event TIMER0 did not show bounded activity: before={}, min={}, max={}, after_stop={}",
            evidence.timer0_value_before_start,
            evidence.timer0_value_min,
            evidence.timer0_value_max,
            evidence.timer0_value_after_stop,
        )
        .into());
    }
    if evidence.rx_status_at_abort.is_some() {
        return Err(format!(
            "IEEE 802.15.4 ED event probe retained unexpected RX_ABORT status {:?}",
            evidence.rx_status_at_abort
        )
        .into());
    }
    if evidence.stop_command_issued {
        return Err("nominal IEEE 802.15.4 ED event probe issued timeout STOP".into());
    }
    if !evidence.cleanup_clear {
        return Err("IEEE 802.15.4 ED event probe did not prove its closed cleanup".into());
    }
    Ok(())
}

fn event_checkpoint_error<T>(checkpoint: &str, observed: u16, expected: u16) -> Result<T> {
    Err(
        format!("IEEE 802.15.4 ED event {checkpoint}={observed:#06x}, expected {expected:#06x}")
            .into(),
    )
}

fn word_checkpoint_error<T>(checkpoint: &str, observed: u32, expected: u32) -> Result<T> {
    Err(
        format!("IEEE 802.15.4 ED event {checkpoint}={observed:#010x}, expected {expected:#010x}")
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nominal() -> Ieee802154EdEventProbeEvidence {
        Ieee802154EdEventProbeEvidence {
            stop: Ieee802154EdEventProbeStop::Complete,
            production_ed_first: Ieee802154PolledEdOutcome::Complete {
                rss_code: -17,
                polls: 2,
            },
            production_ed_second: Some(Ieee802154PolledEdOutcome::Complete {
                rss_code: -18,
                polls: 3,
            }),
            event_enable_before: 0,
            event_enable_active: VALIDATION_EVENTS,
            event_enable_after: 0,
            rx_abort_enable_before: 0,
            rx_abort_enable_active: ED_ABORT_REASONS,
            rx_abort_enable_after: 0,
            route_core0_before_enable: 0,
            route_core1_before_enable: 0,
            route_core0_with_events_enabled: 0,
            route_core1_with_events_enabled: 0,
            route_core0_after_cleanup: 0,
            route_core1_after_cleanup: 0,
            ed_duration_before: 0,
            ed_duration_active: ED_DURATION,
            ed_duration_after: ED_DURATION,
            timer0_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 1,
            timer0_value_after_stop: 1,
            reset_events: 0,
            post_enable_events: 0,
            observed_events: ED_TIMER_EVENTS,
            terminal_events: ED_TIMER_EVENTS,
            after_ed_done_write_events: TIMER0_EVENT,
            after_timer0_write_events: 0,
            cleanup_pending_events: 0,
            final_events: 0,
            rx_status_at_abort: None,
            stop_command_issued: false,
            cleanup_clear: true,
        }
    }

    #[test]
    fn accepts_only_the_exact_closed_selected_write_trace() {
        assert!(validate(nominal(), 100_000).is_ok());

        let mut unexpected = nominal();
        unexpected.observed_events |= RX_ABORT_EVENT;
        unexpected.rx_status_at_abort = Some(25 << 4);
        assert!(validate(unexpected, 100_000).is_err());

        let mut wrong_write = nominal();
        wrong_write.after_ed_done_write_events = ED_TIMER_EVENTS;
        assert!(validate(wrong_write, 100_000).is_err());

        let mut dirty_abort_mask = nominal();
        dirty_abort_mask.rx_abort_enable_before = ED_ABORT_REASONS;
        assert!(validate(dirty_abort_mask, 100_000).is_err());

        let mut restored_duration = nominal();
        restored_duration.ed_duration_after = restored_duration.ed_duration_before;
        assert!(validate(restored_duration, 100_000).is_err());

        let mut inactive_timer = nominal();
        inactive_timer.timer0_value_max = inactive_timer.timer0_value_min;
        assert!(validate(inactive_timer, 100_000).is_err());

        let mut missing_second = nominal();
        missing_second.production_ed_second = None;
        assert!(validate(missing_second, 100_000).is_err());

        let mut failed_first = nominal();
        failed_first.production_ed_first = Ieee802154PolledEdOutcome::Timeout { polls: 100_000 };
        assert!(validate(failed_first, 100_000).is_err());
    }

    #[test]
    fn rejects_every_non_complete_stop() {
        for stop in [
            Ieee802154EdEventProbeStop::ProductionEdFailed,
            Ieee802154EdEventProbeStop::UnsupportedSetup,
            Ieee802154EdEventProbeStop::RouteNotQuiesced,
            Ieee802154EdEventProbeStop::ResetNotClear,
            Ieee802154EdEventProbeStop::EdDurationReadbackMismatch,
            Ieee802154EdEventProbeStop::EventEnableReadbackMismatch,
            Ieee802154EdEventProbeStop::RxAbortEnableReadbackMismatch,
            Ieee802154EdEventProbeStop::PostEnableStatusNotClear,
            Ieee802154EdEventProbeStop::TimerActivityTimeout,
            Ieee802154EdEventProbeStop::PairLatchTimeout,
            Ieee802154EdEventProbeStop::EdAborted,
            Ieee802154EdEventProbeStop::UnexpectedEvent,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
            Ieee802154EdEventProbeStop::CleanupNotClear,
        ] {
            let mut evidence = nominal();
            evidence.stop = stop;
            assert!(
                validate(evidence, 100_000).is_err(),
                "accepted stop {stop:?}"
            );
        }
    }

    #[test]
    fn failed_report_preserves_raw_abort_diagnostics() {
        let mut evidence = nominal();
        evidence.stop = Ieee802154EdEventProbeStop::UnexpectedEvent;
        evidence.observed_events = RX_ABORT_EVENT;
        evidence.terminal_events = RX_ABORT_EVENT;
        evidence.rx_status_at_abort = Some(25 << 4);
        let report = failed_report_document(
            &[BootReport {
                boot: 1,
                target: evidence,
            }],
            "RX_ABORT",
        );

        assert_eq!(report["status"], "failed");
        assert_eq!(report["boots"][0]["target"]["terminal_events"], 16);
        assert_eq!(report["boots"][0]["target"]["rx_status_at_abort"], 400);
    }
}
