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
    Ieee802154EventStatusProbeStop,
};
use serde::Serialize;

use crate::{Result, evidence::traffic_capture::SerialCapture, transport::lab_config::LabConfig};

const CAPABILITIES_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const TIMER0_EVENT: u16 = 1 << 8;
const TIMER1_EVENT: u16 = 1 << 9;
const TIMER_EVENTS: u16 = TIMER0_EVENT | TIMER1_EVENT;
const PUBLIC_EVENT_MASK: u16 = 0x3fff;
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
    println!(
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
    for (checkpoint, events) in [
        ("reset_events", evidence.reset_events),
        ("post_enable_events", evidence.post_enable_events),
        ("dual_observed_events", evidence.dual_observed_events),
        ("dual_latched_events", evidence.dual_latched_events),
        ("after_timer0_ack_events", evidence.after_timer0_ack_events),
        ("after_timer1_ack_events", evidence.after_timer1_ack_events),
        (
            "distinct_snapshot_events",
            evidence.distinct_snapshot_events,
        ),
        (
            "distinct_before_ack_events",
            evidence.distinct_before_ack_events,
        ),
        (
            "distinct_after_ack_events",
            evidence.distinct_after_ack_events,
        ),
        ("cleanup_pending_events", evidence.cleanup_pending_events),
        ("final_events", evidence.final_events),
    ] {
        let unsupported = events & !PUBLIC_EVENT_MASK;
        if unsupported != 0 {
            return Err(format!(
                "IEEE 802.15.4 EVENT_STATUS {checkpoint} contains unsupported bits {unsupported:#06x}"
            )
            .into());
        }
    }

    if evidence.stop != Ieee802154EventStatusProbeStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 EVENT_STATUS probe stopped at {:?}: {evidence:?}",
            evidence.stop,
        )
        .into());
    }
    if evidence.event_enable_before != 0 {
        return checkpoint_error(
            "event_enable_before",
            evidence.event_enable_before,
            "equal zero",
        );
    }
    if evidence.event_enable_active != TIMER_EVENTS {
        return checkpoint_error(
            "event_enable_active",
            evidence.event_enable_active,
            "equal the exact timer-event mask 0x0300",
        );
    }
    if evidence.event_enable_after != 0 {
        return checkpoint_error(
            "event_enable_after",
            evidence.event_enable_after,
            "equal zero",
        );
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
            return route_checkpoint_error(checkpoint, route_word);
        }
    }

    for (checkpoint, observed, expected) in [
        ("reset_events", evidence.reset_events, 0),
        ("post_enable_events", evidence.post_enable_events, 0),
        (
            "dual_observed_events",
            evidence.dual_observed_events,
            TIMER_EVENTS,
        ),
        (
            "dual_latched_events",
            evidence.dual_latched_events,
            TIMER_EVENTS,
        ),
        (
            "after_timer0_ack_events",
            evidence.after_timer0_ack_events,
            TIMER1_EVENT,
        ),
        (
            "after_timer1_ack_events",
            evidence.after_timer1_ack_events,
            0,
        ),
        (
            "distinct_snapshot_events",
            evidence.distinct_snapshot_events,
            TIMER0_EVENT,
        ),
        (
            "distinct_before_ack_events",
            evidence.distinct_before_ack_events,
            TIMER_EVENTS,
        ),
        (
            "distinct_after_ack_events",
            evidence.distinct_after_ack_events,
            TIMER1_EVENT,
        ),
        ("final_events", evidence.final_events, 0),
    ] {
        if observed != expected {
            return checkpoint_error(checkpoint, observed, "equal its exact isolated image");
        }
    }

    if evidence.cleanup_pending_events != 0 && evidence.cleanup_pending_events != TIMER1_EVENT {
        return checkpoint_error(
            "cleanup_pending_events",
            evidence.cleanup_pending_events,
            "equal zero or the retained timer1 image after delivery is masked",
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

fn checkpoint_error<T>(checkpoint: &str, observed: u16, expected: &str) -> Result<T> {
    Err(format!(
        "IEEE 802.15.4 EVENT_STATUS {checkpoint}={observed:#06x}, expected it to {expected}"
    )
    .into())
}

fn route_checkpoint_error<T>(checkpoint: &str, observed: u32) -> Result<T> {
    Err(format!(
        "IEEE 802.15.4 source-132 route {checkpoint}={observed:#010x}, expected the complete reset word 0x00000000"
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nominal() -> Ieee802154EventStatusProbeEvidence {
        Ieee802154EventStatusProbeEvidence {
            stop: Ieee802154EventStatusProbeStop::Complete,
            event_enable_before: 0,
            event_enable_active: TIMER_EVENTS,
            event_enable_after: 0,
            route_core0_before_enable: 0,
            route_core1_before_enable: 0,
            route_core0_with_events_enabled: 0,
            route_core1_with_events_enabled: 0,
            route_core0_after_cleanup: 0,
            route_core1_after_cleanup: 0,
            post_enable_events: 0,
            timer0_value_before_start: 0,
            timer1_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 1,
            timer1_value_min: 0,
            timer1_value_max: 1,
            timer0_value_after_stop: 1,
            timer1_value_after_stop: 1,
            reset_events: 0,
            dual_observed_events: TIMER_EVENTS,
            dual_latched_events: TIMER_EVENTS,
            after_timer0_ack_events: TIMER1_EVENT,
            after_timer1_ack_events: 0,
            distinct_snapshot_events: TIMER0_EVENT,
            distinct_before_ack_events: TIMER_EVENTS,
            distinct_after_ack_events: TIMER1_EVENT,
            cleanup_pending_events: 0,
            final_events: 0,
        }
    }

    #[test]
    fn accepts_only_the_bounded_selective_ack_observation() {
        assert!(validate(nominal()).is_ok());
    }

    #[test]
    fn report_names_the_narrow_result_and_excluded_claims() {
        let report = report_document(&[BootReport {
            boot: 1,
            target: nominal(),
        }]);
        assert_eq!(report["result"], RESULT);
        assert_eq!(
            report["not_proven"],
            serde_json::json!([
                "full-w1c-semantics",
                "event-enable-generation-vs-visibility-semantics",
                "concurrent-same-bit-arrival",
                "level-triggered-route-behavior",
                "masked-final-status-means-physical-cleanup",
            ])
        );
    }

    #[test]
    fn rejects_every_non_complete_stop() {
        for stop in [
            Ieee802154EventStatusProbeStop::UnsupportedSetup,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced,
            Ieee802154EventStatusProbeStop::ResetNotClear,
            Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch,
            Ieee802154EventStatusProbeStop::PostEnableStatusNotClear,
            Ieee802154EventStatusProbeStop::TimerActivityTimeout,
            Ieee802154EventStatusProbeStop::DualLatchTimeout,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout,
            Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout,
            Ieee802154EventStatusProbeStop::CleanupNotClear,
        ] {
            let mut evidence = nominal();
            evidence.stop = stop;
            assert!(validate(evidence).is_err(), "accepted stop {stop:?}");
        }
    }

    #[test]
    fn failed_report_preserves_raw_target_evidence() {
        let mut evidence = nominal();
        evidence.stop = Ieee802154EventStatusProbeStop::TimerActivityTimeout;
        evidence.timer0_value_max = 0;
        let report = failed_report_document(
            &[BootReport {
                boot: 1,
                target: evidence,
            }],
            "timer activity was not observed",
        );

        assert_eq!(report["status"], "failed");
        assert_eq!(report["result"], "incomplete");
        assert_eq!(report["boots"][0]["target"]["timer0_value_max"], 0);
        assert_eq!(report["failure"], "timer activity was not observed");
    }

    #[test]
    fn rejects_each_wrong_checkpoint() {
        let mut cases = Vec::new();

        let mut evidence = nominal();
        evidence.event_enable_before = TIMER0_EVENT;
        cases.push(("entry event enable", evidence));

        let mut evidence = nominal();
        evidence.event_enable_active = TIMER0_EVENT;
        cases.push(("active event enable", evidence));

        let mut evidence = nominal();
        evidence.event_enable_after = TIMER1_EVENT;
        cases.push(("final event enable", evidence));

        let mut evidence = nominal();
        evidence.reset_events = TIMER0_EVENT;
        cases.push(("reset", evidence));

        let mut evidence = nominal();
        evidence.post_enable_events = TIMER0_EVENT;
        cases.push(("post enable", evidence));

        let mut evidence = nominal();
        evidence.dual_latched_events = TIMER0_EVENT;
        cases.push(("dual missing timer1", evidence));

        let mut evidence = nominal();
        evidence.dual_observed_events = TIMER0_EVENT;
        cases.push(("dual union omits terminal timer1", evidence));

        let mut evidence = nominal();
        evidence.after_timer0_ack_events = TIMER_EVENTS;
        cases.push(("timer0 not acknowledged", evidence));

        let mut evidence = nominal();
        evidence.after_timer0_ack_events = 0;
        cases.push(("timer1 removed with timer0", evidence));

        let mut evidence = nominal();
        evidence.after_timer1_ack_events = TIMER1_EVENT;
        cases.push(("timer1 not acknowledged", evidence));

        let mut evidence = nominal();
        evidence.distinct_snapshot_events = 0;
        cases.push(("distinct first timer missing", evidence));

        let mut evidence = nominal();
        evidence.distinct_snapshot_events = TIMER_EVENTS;
        cases.push(("distinct second timer arrived too early", evidence));

        let mut evidence = nominal();
        evidence.distinct_before_ack_events = TIMER0_EVENT;
        cases.push(("distinct second timer missing", evidence));

        let mut evidence = nominal();
        evidence.distinct_after_ack_events = TIMER_EVENTS;
        cases.push(("distinct timer0 not acknowledged", evidence));

        let mut evidence = nominal();
        evidence.distinct_after_ack_events = 0;
        cases.push(("distinct timer1 removed with timer0", evidence));

        let mut evidence = nominal();
        evidence.cleanup_pending_events = TIMER_EVENTS;
        cases.push(("masked cleanup observation", evidence));

        let mut evidence = nominal();
        evidence.final_events = TIMER1_EVENT;
        cases.push(("final cleanup", evidence));

        for (checkpoint, evidence) in cases {
            assert!(
                validate(evidence).is_err(),
                "accepted wrong {checkpoint} checkpoint"
            );
        }
    }

    #[test]
    fn accepts_either_masked_cleanup_visibility() {
        let mut visible = nominal();
        visible.cleanup_pending_events = TIMER1_EVENT;
        assert!(validate(visible).is_ok());
    }

    #[test]
    fn route_validation_rejects_each_raw_bit_on_each_core_and_checkpoint() {
        for bit in 0..u32::BITS {
            let word = 1_u32 << bit;
            for checkpoint in 0..6 {
                let mut evidence = nominal();
                match checkpoint {
                    0 => evidence.route_core0_before_enable = word,
                    1 => evidence.route_core1_before_enable = word,
                    2 => evidence.route_core0_with_events_enabled = word,
                    3 => evidence.route_core1_with_events_enabled = word,
                    4 => evidence.route_core0_after_cleanup = word,
                    5 => evidence.route_core1_after_cleanup = word,
                    _ => unreachable!(),
                }
                assert!(
                    validate(evidence).is_err(),
                    "accepted route bit {bit} at checkpoint {checkpoint}"
                );
            }
        }
    }

    #[test]
    fn requires_counter_activity_and_a_bounded_entry_sample() {
        let mut inactive = nominal();
        inactive.timer0_value_max = inactive.timer0_value_min;
        assert!(validate(inactive).is_err());

        let mut out_of_range = nominal();
        out_of_range.timer1_value_before_start = out_of_range.timer1_value_max + 1;
        assert!(validate(out_of_range).is_err());
    }

    #[test]
    fn rejects_unsupported_bits_at_every_checkpoint() {
        const UNSUPPORTED: u16 = 1 << 14;
        for checkpoint in 0..11 {
            let mut evidence = nominal();
            match checkpoint {
                0 => evidence.reset_events |= UNSUPPORTED,
                1 => evidence.post_enable_events |= UNSUPPORTED,
                2 => evidence.dual_observed_events |= UNSUPPORTED,
                3 => evidence.dual_latched_events |= UNSUPPORTED,
                4 => evidence.after_timer0_ack_events |= UNSUPPORTED,
                5 => evidence.after_timer1_ack_events |= UNSUPPORTED,
                6 => evidence.distinct_snapshot_events |= UNSUPPORTED,
                7 => evidence.distinct_before_ack_events |= UNSUPPORTED,
                8 => evidence.distinct_after_ack_events |= UNSUPPORTED,
                9 => evidence.cleanup_pending_events |= UNSUPPORTED,
                10 => evidence.final_events |= UNSUPPORTED,
                _ => unreachable!(),
            }
            assert!(
                validate(evidence).is_err(),
                "accepted unsupported bits at checkpoint {checkpoint}"
            );
        }
    }
}
