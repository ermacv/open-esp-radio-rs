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
    Result,
    controlled_ap::{ControlledAp, require_credentials_environment},
    traffic_capture::SerialCapture,
};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const QUALIFIED_ATTEMPTS: u16 = 3;

struct Options {
    serial: PathBuf,
    timeout: Duration,
}

pub(crate) fn run(arguments: Vec<String>, root: &Path) -> Result<()> {
    if arguments
        .first()
        .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    let options = parse_options(&arguments)?;
    require_credentials_environment()?;
    let output = root.join("target/hil/esp32s31/qualification/station-ap-absence");
    fs::create_dir_all(&output)?;

    let mut ap = ControlledAp::start()?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let mut cursor = capture.station_lifecycle_cursor();
    let result = qualify(&capture, &mut cursor, &mut ap, options.timeout);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    drop(ap);

    result?;
    println!("station_ap_absence=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    cursor: &mut usize,
    ap: &mut ControlledAp,
    timeout: Duration,
) -> Result<()> {
    let capabilities = capture.prepare_protocol()?;
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

    for attempt in 1..=QUALIFIED_ATTEMPTS {
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

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
        timeout: DEFAULT_TIMEOUT,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--serial" => {
                index += 1;
                options.serial = PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or("--serial requires a device path")?,
                );
            }
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

fn print_help() {
    println!(
        "cargo hil station ap-absence [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --timeout-seconds <n>    deadline for each lifecycle edge, 30..=300 (default 120)\n\n\
         Starts the controlled HE20 AP, waits for generation zero, then keeps\n\
         the AP absent through three complete NoCandidate attempts and typed\n\
         retry exhaustion. Managed host networking is restored on every exit."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_keeps_one_bounded_edge_deadline() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--timeout-seconds".into(),
            "75".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
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
