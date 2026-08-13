//! Host control for bounded connected-STA lifecycle qualification.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{Result, lab_config::LabConfig, traffic_capture::SerialCapture};
use open_esp_radio_hil_protocol::{
    StationAttemptFailureReason, StationEpochEvidence, StationLifecycleEvent,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CYCLES: u8 = 1;
const MAX_CYCLES: u8 = 8;
const DEFAULT_BOOTS: u8 = 1;
const MAX_BOOTS: u8 = 100;
const MAX_INITIAL_HOLD: Duration = Duration::from_secs(30);
struct Options {
    serial: PathBuf,
    timeout: Duration,
    cycles: u8,
    boots: u8,
    initial_hold: Duration,
}

pub(crate) fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let options = parse_options(&arguments, lab)?;
    fs::create_dir_all(&output)?;
    for boot in 1..=options.boots {
        let capture = SerialCapture::start_with_reset(&options.serial);
        let result = qualify(
            &capture,
            lab,
            options.timeout,
            options.cycles,
            options.initial_hold,
        );
        let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
        let boot_output = output.join(format!("boot-{boot:03}"));
        capture.finish_to(&boot_output)?;
        let boot_log = boot_output.join("uart.log");
        result.map_err(|error| {
            format!(
                "station cold boot {boot}/{} failed; UART evidence: {}: {error}",
                options.boots,
                boot_log.display(),
            )
        })?;
        if let Some(result) = beacon_loss {
            result?;
        }
        println!("station_cold_boot={boot}/{} status=PASS", options.boots);
    }
    println!("station_reconnect=PASS boots={}", options.boots);
    println!("uart_logs={}", output.display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    lab: &LabConfig,
    timeout: Duration,
    cycles: u8,
    initial_hold: Duration,
) -> Result<()> {
    // Arm the unsolicited-event cursor before provisioning can make the
    // station connect. With a correctly clocked target, scan/join may finish
    // before `prepare_station` returns its correlated acknowledgement; taking
    // the cursor afterwards skips that already-published Connected edge and
    // waits forever for a second one.
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let capabilities = capture.prepare_station(lab, timeout)?;
    if !capabilities.features.station_epoch_control {
        return Err("firmware does not advertise station epoch control".into());
    }
    // Network provisioning is acknowledged before the radio task finishes
    // scan/join/WPA2.  A lifecycle command is valid only after the unsolicited
    // connected edge has published the Station owner at the public boundary.
    let mut generation = capture.wait_for_connected_station(timeout)?;
    wait_for_connected_generation(capture, &mut lifecycle_cursor, generation, timeout)?;
    qualify_initial_hold(capture, &mut lifecycle_cursor, generation, initial_hold)?;
    report_stack(capture, timeout, "initial-connected")?;
    for cycle in 1..=cycles {
        generation = qualify_cycle(capture, timeout, cycle, generation)?;
        report_stack(capture, timeout, "reconnect-complete")?;
        println!("station_reconnect_cycle={cycle}/{cycles} status=PASS");
    }
    Ok(())
}

fn wait_for_connected_generation(
    capture: &SerialCapture,
    cursor: &mut usize,
    expected_generation: u32,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = capture.wait_station_lifecycle_event(cursor, remaining)?;
        if event
            == (StationLifecycleEvent::Connected {
                generation: expected_generation,
            })
        {
            return Ok(());
        }
        if matches!(event, StationLifecycleEvent::Disconnected { .. }) {
            return Err(format!(
                "station disconnected before connected generation {expected_generation} could be held"
            )
            .into());
        }
    }
}

fn qualify_initial_hold(
    capture: &SerialCapture,
    cursor: &mut usize,
    generation: u32,
    duration: Duration,
) -> Result<()> {
    if duration.is_zero() {
        return Ok(());
    }
    if let Some(event) = capture.wait_station_lifecycle_event_optional(cursor, duration)? {
        return Err(format!(
            "station generation {generation} changed during the initial {}-second hold: {event:?}",
            duration.as_secs()
        )
        .into());
    }
    println!(
        "station_initial_hold_generation={generation} seconds={} status=PASS",
        duration.as_secs()
    );
    Ok(())
}

fn report_stack(capture: &SerialCapture, timeout: Duration, stage: &str) -> Result<()> {
    let usage = capture.query_stack_usage(timeout)?;
    println!(
        "stack_stage={stage} cpu0_free={}/{} cpu0_required={} cpu1_free={}/{} cpu1_required={}",
        usage.cpu0.free_bytes,
        usage.cpu0.capacity_bytes,
        usage.cpu0.minimum_free_bytes,
        usage.cpu1.free_bytes,
        usage.cpu1.capacity_bytes,
        usage.cpu1.minimum_free_bytes,
    );
    Ok(())
}

#[derive(Default)]
struct CycleProgress {
    disconnected: bool,
    connected: bool,
    owners_complete: bool,
}

impl CycleProgress {
    fn observe_lifecycle(
        &mut self,
        event: StationLifecycleEvent,
        cycle: u8,
        generation: u32,
    ) -> Result<()> {
        match event {
            StationLifecycleEvent::Disconnected {
                generation: observed,
                reason: open_esp_radio_hil_protocol::StationDisconnectReason::ReconnectRequested,
            } if observed == generation && !self.disconnected => {
                self.disconnected = true;
                Ok(())
            }
            StationLifecycleEvent::Connected {
                generation: observed,
            } if self.disconnected && observed == generation.wrapping_add(1) => {
                self.connected = true;
                Ok(())
            }
            event @ (StationLifecycleEvent::AttemptFailed { .. }
            | StationLifecycleEvent::RetryExhausted { .. }) => {
                validate_cycle_event(event, cycle)
            }
            event => Err(format!(
                    "station reconnect cycle {cycle} published an unexpected lifecycle edge: \
                     expected generation {generation} disconnect then generation {} connect, got {event:?}",
                    generation.wrapping_add(1),
                )
                .into()),
        }
    }

    const fn complete(&self) -> bool {
        self.disconnected && self.connected && self.owners_complete
    }
}

fn qualify_cycle(
    capture: &SerialCapture,
    timeout: Duration,
    cycle: u8,
    generation: u32,
) -> Result<u32> {
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let handle = capture.request_station_epoch_cycle()?;
    let mut progress = CycleProgress::default();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(20));
        if let Some(event) =
            capture.wait_station_lifecycle_event_optional(&mut lifecycle_cursor, wait)?
        {
            progress.observe_lifecycle(event, cycle, generation)?;
        }
        if !progress.owners_complete {
            progress.owners_complete = validate_cycle_completion(
                capture.observed_station_epoch_completion(handle),
                cycle,
            )?;
        }
        if progress.complete() {
            return Ok(generation.wrapping_add(1));
        }
    }
    Err(format!(
        "target did not complete station reconnect cycle {cycle} within {} seconds \
         (disconnected={}, connected={}, owners_complete={})",
        timeout.as_secs(),
        progress.disconnected,
        progress.connected,
        progress.owners_complete,
    )
    .into())
}

fn validate_cycle_event(event: StationLifecycleEvent, cycle: u8) -> Result<()> {
    match event {
        StationLifecycleEvent::RetryExhausted { .. } => {
            Err(format!("station lifecycle exhausted retries in cycle {cycle}: {event:?}").into())
        }
        StationLifecycleEvent::AttemptFailed {
            reason:
                StationAttemptFailureReason::Hardware | StationAttemptFailureReason::ContractViolation,
            ..
        } => Err(
            format!("station lifecycle reported fatal failure in cycle {cycle}: {event:?}").into(),
        ),
        _ => Ok(()),
    }
}

fn validate_cycle_completion(completion: Option<StationEpochEvidence>, cycle: u8) -> Result<bool> {
    let Some(completion) = completion else {
        return Ok(false);
    };
    if !completion.is_complete() {
        return Err(format!(
            "station epoch cycle {cycle} completed with incomplete typed evidence: {completion:?}"
        )
        .into());
    }
    Ok(true)
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        serial: lab.device.serial.clone(),
        timeout: DEFAULT_TIMEOUT,
        cycles: DEFAULT_CYCLES,
        boots: DEFAULT_BOOTS,
        initial_hold: Duration::ZERO,
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
                if !(10..=300).contains(&seconds) {
                    return Err("--timeout-seconds must be in 10..=300".into());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            "--cycles" => {
                index += 1;
                let cycles = arguments
                    .get(index)
                    .ok_or("--cycles requires a value")?
                    .parse::<u8>()?;
                if !(1..=MAX_CYCLES).contains(&cycles) {
                    return Err(format!("--cycles must be in 1..={MAX_CYCLES}").into());
                }
                options.cycles = cycles;
            }
            "--boots" => {
                index += 1;
                let boots = arguments
                    .get(index)
                    .ok_or("--boots requires a value")?
                    .parse::<u8>()?;
                if !(1..=MAX_BOOTS).contains(&boots) {
                    return Err(format!("--boots must be in 1..={MAX_BOOTS}").into());
                }
                options.boots = boots;
            }
            "--initial-hold-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--initial-hold-seconds requires a value")?
                    .parse::<u64>()?;
                let duration = Duration::from_secs(seconds);
                if duration > MAX_INITIAL_HOLD {
                    return Err("--initial-hold-seconds must be in 0..=30".into());
                }
                options.initial_hold = duration;
            }
            argument => return Err(format!("unknown station reconnect option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_station_reconnect_options() {
        let options = parse_options(
            &[
                "--timeout-seconds".into(),
                "120".into(),
                "--cycles".into(),
                "3".into(),
            ],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/ttyACM0"));
        assert_eq!(options.timeout, Duration::from_secs(120));
        assert_eq!(options.cycles, 3);
        assert_eq!(options.boots, 1);
        assert_eq!(options.initial_hold, Duration::ZERO);
    }

    #[test]
    fn rejects_unbounded_station_reconnect_timeout() {
        assert!(
            parse_options(
                &["--timeout-seconds".into(), "301".into()],
                &LabConfig::for_test()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unbounded_station_reconnect_cycles() {
        assert!(parse_options(&["--cycles".into(), "0".into()], &LabConfig::for_test()).is_err());
        assert!(parse_options(&["--cycles".into(), "9".into()], &LabConfig::for_test()).is_err());
    }

    #[test]
    fn parses_and_bounds_reset_separated_boots() {
        let options =
            parse_options(&["--boots".into(), "30".into()], &LabConfig::for_test()).unwrap();
        assert_eq!(options.boots, 30);
        assert!(parse_options(&["--boots".into(), "0".into()], &LabConfig::for_test()).is_err());
        assert!(parse_options(&["--boots".into(), "101".into()], &LabConfig::for_test()).is_err());
    }

    #[test]
    fn parses_and_bounds_initial_connected_hold() {
        let options = parse_options(
            &["--initial-hold-seconds".into(), "10".into()],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.initial_hold, Duration::from_secs(10));
        assert!(
            parse_options(
                &["--initial-hold-seconds".into(), "31".into()],
                &LabConfig::for_test()
            )
            .is_err()
        );
    }

    #[test]
    fn missing_typed_completion_cannot_qualify_a_cycle() {
        assert!(!validate_cycle_completion(None, 2).unwrap());
    }

    #[test]
    fn typed_completion_requires_every_owner_edge() {
        let mut incomplete = StationEpochEvidence::COMPLETE;
        incomplete.scan_owners_returned = false;
        assert!(validate_cycle_completion(Some(incomplete), 2).is_err());
        assert!(validate_cycle_completion(Some(StationEpochEvidence::COMPLETE), 2).unwrap());
    }

    #[test]
    fn retryable_peer_failure_does_not_preempt_lifecycle_policy() {
        let retrying = StationLifecycleEvent::AttemptFailed {
            generation: 1,
            attempt: 1,
            stage: open_esp_radio_hil_protocol::StationFailureStage::Association,
            reason: StationAttemptFailureReason::PeerProtocol,
        };
        assert!(validate_cycle_event(retrying, 2).is_ok());
    }

    #[test]
    fn fatal_or_exhausted_lifecycle_fails_immediately() {
        let fatal = StationLifecycleEvent::AttemptFailed {
            generation: 1,
            attempt: 1,
            stage: open_esp_radio_hil_protocol::StationFailureStage::Hardware,
            reason: StationAttemptFailureReason::ContractViolation,
        };
        assert!(validate_cycle_event(fatal, 2).is_err());
        let exhausted = StationLifecycleEvent::RetryExhausted {
            generation: 1,
            attempts: 3,
            stage: open_esp_radio_hil_protocol::StationFailureStage::CandidateSelection,
            reason: StationAttemptFailureReason::NoCandidate,
        };
        assert!(validate_cycle_event(exhausted, 2).is_err());
    }

    #[test]
    fn cycle_requires_ordered_disconnect_and_next_generation_connect() {
        let mut progress = CycleProgress::default();
        assert!(
            progress
                .observe_lifecycle(StationLifecycleEvent::Connected { generation: 2 }, 1, 1,)
                .is_err()
        );

        progress
            .observe_lifecycle(
                StationLifecycleEvent::Disconnected {
                    generation: 1,
                    reason:
                        open_esp_radio_hil_protocol::StationDisconnectReason::ReconnectRequested,
                },
                1,
                1,
            )
            .unwrap();
        progress
            .observe_lifecycle(StationLifecycleEvent::Connected { generation: 2 }, 1, 1)
            .unwrap();
        assert!(!progress.complete());
        progress.owners_complete = true;
        assert!(progress.complete());
    }
}
