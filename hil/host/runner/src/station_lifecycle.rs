//! Host control for bounded connected-STA lifecycle qualification.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{Result, lab_config::LabConfig, traffic_capture::SerialCapture};
use open_esp_radio_hil_protocol::StationEpochEvidence;
use open_esp_radio_hil_protocol::StationLifecycleEvent;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CYCLES: u8 = 1;
const MAX_CYCLES: u8 = 8;
const DEFAULT_BOOTS: u8 = 1;
const MAX_BOOTS: u8 = 100;
const MAX_INITIAL_HOLD: Duration = Duration::from_secs(30);
const RECONNECT_FAILURE_MARKER: &str = "result=FAIL stage=production-reconnect";
const RUNNING_SCAN_FAILURE_MARKER: &str = "result=FAIL stage=production-running-scan";
const LIFECYCLE_EXHAUSTED_MARKER: &str = "result=FAIL stage=production-sta-lifecycle-exhausted";
const LIFECYCLE_TERMINAL_MARKER: &str = "result=FAIL stage=production-sta-lifecycle-terminal";
const FATAL_STARTUP_FAILURE_MARKERS: &[&str] = &[
    "result=FAIL stage=mac-cold-start",
    "result=FAIL stage=rx-ring-stage",
    "result=FAIL stage=initial-scan-service-stop",
    "result=FAIL stage=initial-scan-service ",
    "result=FAIL stage=initial-scan-plan",
    "result=FAIL stage=production-connected-task-stop",
];

struct Options {
    serial: PathBuf,
    timeout: Duration,
    cycles: u8,
    boots: u8,
    initial_hold: Duration,
}

#[derive(Clone, Copy)]
struct CycleEvidence {
    reconnect_failure: usize,
    running_scan_failure: usize,
    lifecycle_exhausted: usize,
    lifecycle_terminal: usize,
}

impl CycleEvidence {
    fn capture(serial: &SerialCapture) -> Self {
        Self {
            reconnect_failure: serial.marker_count(RECONNECT_FAILURE_MARKER),
            running_scan_failure: serial.marker_count(RUNNING_SCAN_FAILURE_MARKER),
            lifecycle_exhausted: serial.marker_count(LIFECYCLE_EXHAUSTED_MARKER),
            lifecycle_terminal: serial.marker_count(LIFECYCLE_TERMINAL_MARKER),
        }
    }
}

pub(crate) fn run(arguments: Vec<String>, root: &Path, lab: &LabConfig) -> Result<()> {
    if arguments
        .first()
        .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        print_help();
        return Ok(());
    }
    let options = parse_options(&arguments, lab)?;
    let output = station_reconnect_run_directory(root)?;
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
        let log = capture.finish();
        let boot_log = output.join(format!("uart-{boot:03}.log"));
        fs::write(&boot_log, &log)?;
        fs::write(output.join("uart.log"), &log)?;
        result.map_err(|error| {
            format!(
                "station cold boot {boot}/{} failed; UART evidence: {}: {error}",
                options.boots,
                boot_log.display(),
            )
        })?;
        println!("station_cold_boot={boot}/{} status=PASS", options.boots);
    }
    println!("station_reconnect=PASS boots={}", options.boots);
    println!("uart_logs={}", output.display());
    Ok(())
}

fn station_reconnect_run_directory(root: &Path) -> Result<PathBuf> {
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    Ok(root
        .join("target/hil/esp32s31/qualification/station-reconnect")
        .join(format!("run-{started}-{}", std::process::id())))
}

fn qualify(
    capture: &SerialCapture,
    lab: &LabConfig,
    timeout: Duration,
    cycles: u8,
    initial_hold: Duration,
) -> Result<()> {
    let capabilities = capture.prepare_protocol(lab)?;
    if !capabilities.features.station_epoch_control {
        return Err("firmware does not advertise station epoch control".into());
    }
    // Network provisioning is acknowledged before the radio task finishes
    // scan/join/WPA2.  A lifecycle command is valid only after the unsolicited
    // connected edge has published the Station owner at the public boundary.
    let mut lifecycle_cursor = capture.station_lifecycle_cursor();
    let generation = capture.wait_for_connected_station(timeout)?;
    wait_for_connected_generation(capture, &mut lifecycle_cursor, generation, timeout)?;
    qualify_initial_hold(capture, &mut lifecycle_cursor, generation, initial_hold)?;
    report_stack(capture, timeout, "initial-connected")?;
    for cycle in 1..=cycles {
        qualify_cycle(capture, timeout, cycle)?;
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
        "stack_stage={stage} cpu0_free={}/{} cpu1_free={}/{} required_percent={}",
        usage.cpu0.free_bytes,
        usage.cpu0.capacity_bytes,
        usage.cpu1.free_bytes,
        usage.cpu1.capacity_bytes,
        usage.minimum_free_percent,
    );
    Ok(())
}

fn qualify_cycle(capture: &SerialCapture, timeout: Duration, cycle: u8) -> Result<()> {
    let before = CycleEvidence::capture(capture);
    let handle = capture.request_station_epoch_cycle()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(marker) = FATAL_STARTUP_FAILURE_MARKERS
            .iter()
            .find(|marker| capture.contains(marker))
        {
            return Err(format!(
                "target reported fatal station startup failure in cycle {cycle}: {marker}"
            )
            .into());
        }
        let after = CycleEvidence::capture(capture);
        let completion = capture.observed_station_epoch_completion(handle);
        if validate_cycle_progress(before, after, completion, cycle)? {
            println!(
                "station_reconnect_cycle_retry_evidence={cycle} \
                 running_scan_failures={} reconnect_failures={}",
                after
                    .running_scan_failure
                    .saturating_sub(before.running_scan_failure),
                after
                    .reconnect_failure
                    .saturating_sub(before.reconnect_failure),
            );
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "target did not complete station reconnect cycle {cycle} within {} seconds",
        timeout.as_secs()
    )
    .into())
}

fn validate_cycle_progress(
    before: CycleEvidence,
    after: CycleEvidence,
    completion: Option<StationEpochEvidence>,
    cycle: u8,
) -> Result<bool> {
    if after.lifecycle_terminal > before.lifecycle_terminal {
        return Err(format!("station lifecycle reported terminal failure in cycle {cycle}").into());
    }
    if after.lifecycle_exhausted > before.lifecycle_exhausted {
        return Err(format!("station lifecycle exhausted retries in cycle {cycle}").into());
    }
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

fn print_help() {
    println!(
        "cargo hil station reconnect [options]\n\n\
         --timeout-seconds <n>    per-cycle deadline, 10..=300 (default 90)\n\
         --cycles <n>             sequential lifecycle cycles, 1..=8 (default 1)\n\n\
         --boots <n>              reset-separated boots, 1..=100 (default 1)\n\n\
         --initial-hold-seconds <n> fail on any lifecycle edge during 0..=30 seconds\n\n\
         Resets the flashed radio image, provisions credentials through the \n\
         typed UART protocol, and requires a reliable target acknowledgement\n\
         covering runner stop, returned scan owners, fresh join, and the next\n\
         connected runner. Text logs remain diagnostic-only."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle_evidence() -> CycleEvidence {
        CycleEvidence {
            reconnect_failure: 0,
            running_scan_failure: 0,
            lifecycle_exhausted: 0,
            lifecycle_terminal: 0,
        }
    }

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
    fn stale_cycle_markers_cannot_qualify_another_cycle() {
        let evidence = cycle_evidence();
        assert!(!validate_cycle_progress(evidence, evidence, None, 2).unwrap());
    }

    #[test]
    fn typed_completion_requires_every_owner_edge() {
        let evidence = cycle_evidence();
        let mut incomplete = StationEpochEvidence::COMPLETE;
        incomplete.scan_owners_returned = false;
        assert!(validate_cycle_progress(evidence, evidence, Some(incomplete), 2).is_err());
        assert!(
            validate_cycle_progress(evidence, evidence, Some(StationEpochEvidence::COMPLETE), 2,)
                .unwrap()
        );
    }

    #[test]
    fn retryable_scan_and_reconnect_failures_do_not_preempt_lifecycle_policy() {
        let before = cycle_evidence();
        let mut retrying = before;
        retrying.running_scan_failure += 1;
        retrying.reconnect_failure += 1;
        assert!(!validate_cycle_progress(before, retrying, None, 2).unwrap());

        let mut recovered = cycle_evidence();
        recovered.running_scan_failure = retrying.running_scan_failure;
        recovered.reconnect_failure = retrying.reconnect_failure;
        assert!(
            validate_cycle_progress(before, recovered, Some(StationEpochEvidence::COMPLETE), 2,)
                .unwrap()
        );
    }

    #[test]
    fn terminal_or_exhausted_lifecycle_still_fails_immediately() {
        let before = cycle_evidence();
        let mut terminal = before;
        terminal.lifecycle_terminal += 1;
        assert!(validate_cycle_progress(before, terminal, None, 2).is_err());

        let mut exhausted = before;
        exhausted.lifecycle_exhausted += 1;
        assert!(validate_cycle_progress(before, exhausted, None, 2).is_err());
    }
}
