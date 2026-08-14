//! Controlled prolonged AP absence and bounded retry-exhaustion qualification.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationDisconnectReason, StationFailureStage,
    StationLifecycleEvent,
};

use crate::{
    Result, controlled_ap::ControlledAp, lab_config::LabConfig, scenario::PhyExpectation,
    traffic_capture::SerialCapture,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const QUALIFIED_ATTEMPTS: u16 = 3;

struct Options {
    serial: PathBuf,
    timeout: Duration,
}

pub(crate) fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    phy: PhyExpectation,
) -> Result<()> {
    let options = parse_options(&arguments, lab)?;
    fs::create_dir_all(output)?;

    let mut ap = ControlledAp::start(&lab.station, &lab.station_fixture, phy)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let mut cursor = capture.station_lifecycle_cursor();
    let result = qualify(&capture, lab, &mut cursor, &mut ap, options.timeout);
    capture.finish_to(output)?;
    drop(ap);

    result?;
    println!("station_ap_absence=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    lab: &LabConfig,
    cursor: &mut usize,
    ap: &mut ControlledAp,
    timeout: Duration,
) -> Result<()> {
    let capabilities = capture.prepare_station(lab, timeout)?;
    if !capabilities.features.station_lifecycle_events {
        return Err("firmware does not advertise reliable station lifecycle events".into());
    }

    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::Connected { generation: 0 },
        "initial connection",
    )?;
    let absence_started = Instant::now();
    ap.stop()?;
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::BeaconLoss,
        },
        "beacon-loss disconnect",
    )?;

    for attempt in 1..QUALIFIED_ATTEMPTS {
        expect_event(
            capture,
            cursor,
            timeout,
            StationLifecycleEvent::AttemptFailed {
                generation: 1,
                attempt,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            },
            "no-candidate attempt",
        )?;
    }
    expect_event(
        capture,
        cursor,
        timeout,
        StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: QUALIFIED_ATTEMPTS,
            stage: StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        },
        "retry exhaustion",
    )?;
    println!(
        "station_ap_absence_exhausted_ms={}",
        absence_started.elapsed().as_millis()
    );
    Ok(())
}

fn expect_event(
    capture: &SerialCapture,
    cursor: &mut usize,
    timeout: Duration,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    let actual = capture
        .wait_station_lifecycle_event(cursor, timeout)
        .map_err(|error| format!("station {transition}: {error}"))?;
    validate_event(actual, expected, transition)
}

fn validate_event(
    actual: StationLifecycleEvent,
    expected: StationLifecycleEvent,
    transition: &str,
) -> Result<()> {
    if actual != expected {
        return Err(
            format!("station {transition} reported {actual:?}, expected {expected:?}").into(),
        );
    }
    Ok(())
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        serial: lab.device.serial.clone(),
        timeout: DEFAULT_TIMEOUT,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--timeout-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--timeout-seconds requires a value")?
                    .parse::<u64>()?;
                if !(30..=300).contains(&seconds) {
                    return Err("--timeout-seconds must be in 30..=300".into());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            argument => {
                return Err(format!("unknown station AP-absence option `{argument}`").into());
            }
        }
        index += 1;
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_one_bounded_edge_deadline() {
        let options = parse_options(
            &["--timeout-seconds".into(), "75".into()],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/ttyACM0"));
        assert_eq!(options.timeout, Duration::from_secs(75));
    }

    #[test]
    fn another_attempt_cannot_qualify_retry_exhaustion() {
        let error = validate_event(
            StationLifecycleEvent::RetryExhausted {
                generation: 1,
                attempts: 2,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            },
            StationLifecycleEvent::RetryExhausted {
                generation: 1,
                attempts: QUALIFIED_ATTEMPTS,
                stage: StationFailureStage::CandidateSelection,
                reason: StationAttemptFailureReason::NoCandidate,
            },
            "retry exhaustion",
        )
        .unwrap_err();
        assert!(error.to_string().contains("attempts: 2"));
    }
}
