//! Host control for bounded connected-STA lifecycle qualification.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::{Result, traffic_capture::SerialCapture};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_CYCLES: u8 = 1;
const MAX_CYCLES: u8 = 8;
const RECONNECTED_MARKER: &str = "result=PASS stage=production-reconnect-connected-enter";
const RECONNECT_FAILURE_MARKER: &str = "result=FAIL stage=production-reconnect";
const RUNNING_SCAN_FAILURE_MARKER: &str = "result=FAIL stage=production-running-scan";
const RUNNER_STOP_MARKER: &str = "result=PASS stage=production-runner-stop";
const RUNNING_SCAN_MARKER: &str = "result=PASS stage=production-running-scan channels=13";
const RUNNING_SCAN_LIFECYCLE_MARKER: &str = "refresh_candidate=1 phase=running-scan";
const RUNNING_SCAN_OWNER_RETURN_MARKER: &str =
    "result=PASS stage=production-running-scan-owner-return";
const RECONNECT_AUTHENTICATION_MARKER: &str =
    "result=PASS stage=production-reconnect-authentication";
const CONNECTED_READY_MARKER: &str = "result=PASS stage=embassy-task-topology";

struct Options {
    serial: PathBuf,
    timeout: Duration,
    cycles: u8,
}

#[derive(Clone, Copy)]
struct CycleEvidence {
    reconnected: usize,
    reconnect_failure: usize,
    runner_stop: usize,
    running_scan: usize,
    running_scan_failure: usize,
    running_scan_lifecycle: usize,
    running_scan_owner_return: usize,
    reconnect_authentication: usize,
    connected_ready: usize,
}

impl CycleEvidence {
    fn capture(serial: &SerialCapture) -> Self {
        Self {
            reconnected: serial.marker_count(RECONNECTED_MARKER),
            reconnect_failure: serial.marker_count(RECONNECT_FAILURE_MARKER),
            runner_stop: serial.marker_count(RUNNER_STOP_MARKER),
            running_scan: serial.marker_count(RUNNING_SCAN_MARKER),
            running_scan_failure: serial.marker_count(RUNNING_SCAN_FAILURE_MARKER),
            running_scan_lifecycle: serial.marker_count(RUNNING_SCAN_LIFECYCLE_MARKER),
            running_scan_owner_return: serial.marker_count(RUNNING_SCAN_OWNER_RETURN_MARKER),
            reconnect_authentication: serial.marker_count(RECONNECT_AUTHENTICATION_MARKER),
            connected_ready: serial.marker_count(CONNECTED_READY_MARKER),
        }
    }
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
    let output = root.join("target/hil/esp32s31/qualification/station-reconnect");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, options.timeout, options.cycles);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    result?;
    println!("station_reconnect=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(capture: &SerialCapture, timeout: Duration, cycles: u8) -> Result<()> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.station_epoch_control {
        return Err("firmware does not advertise station epoch control".into());
    }
    for cycle in 1..=cycles {
        qualify_cycle(capture, timeout, cycle)?;
        println!("station_reconnect_cycle={cycle}/{cycles} status=PASS");
    }
    Ok(())
}

fn qualify_cycle(capture: &SerialCapture, timeout: Duration, cycle: u8) -> Result<()> {
    let before = CycleEvidence::capture(capture);
    capture.request_station_epoch_cycle()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let after = CycleEvidence::capture(capture);
        if validate_cycle_progress(before, after, cycle)? {
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

fn validate_cycle_progress(before: CycleEvidence, after: CycleEvidence, cycle: u8) -> Result<bool> {
    if after.reconnect_failure > before.reconnect_failure {
        return Err(format!("target reported a reconnect failure in cycle {cycle}").into());
    }
    if after.running_scan_failure > before.running_scan_failure {
        return Err(format!("target reported a running-scan failure in cycle {cycle}").into());
    }
    if after.reconnected <= before.reconnected || after.connected_ready <= before.connected_ready {
        return Ok(false);
    }
    require_new_marker(
        after.runner_stop,
        before.runner_stop,
        cycle,
        "qualified runner stop",
    )?;
    require_new_marker(
        after.running_scan,
        before.running_scan,
        cycle,
        "complete running scan",
    )?;
    require_new_marker(
        after.running_scan_lifecycle,
        before.running_scan_lifecycle,
        cycle,
        "outer lifecycle scan phase",
    )?;
    require_new_marker(
        after.running_scan_owner_return,
        before.running_scan_owner_return,
        cycle,
        "returned running-scan owners",
    )?;
    require_new_marker(
        after.reconnect_authentication,
        before.reconnect_authentication,
        cycle,
        "refreshed Open Authentication",
    )?;
    Ok(true)
}

fn require_new_marker(after: usize, before: usize, cycle: u8, evidence: &str) -> Result<()> {
    if after > before {
        Ok(())
    } else {
        Err(format!("cycle {cycle} entered reconnect without {evidence}").into())
    }
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
        timeout: DEFAULT_TIMEOUT,
        cycles: DEFAULT_CYCLES,
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
            argument => return Err(format!("unknown station reconnect option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn print_help() {
    println!(
        "cargo hil station reconnect [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --timeout-seconds <n>    per-cycle deadline, 10..=300 (default 90)\n\
         --cycles <n>             sequential lifecycle cycles, 1..=8 (default 1)\n\n\
         Resets the flashed radio image, provisions credentials through the \n\
         typed UART protocol, and qualifies every requested stop, running scan,\n\
         fresh Authentication/Association/WPA2, and connected epoch."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycle_evidence(count: usize) -> CycleEvidence {
        CycleEvidence {
            reconnected: count,
            reconnect_failure: 0,
            runner_stop: count,
            running_scan: count,
            running_scan_failure: 0,
            running_scan_lifecycle: count,
            running_scan_owner_return: count,
            reconnect_authentication: count,
            connected_ready: count,
        }
    }

    #[test]
    fn parses_bounded_station_reconnect_options() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--timeout-seconds".into(),
            "120".into(),
            "--cycles".into(),
            "3".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
        assert_eq!(options.timeout, Duration::from_secs(120));
        assert_eq!(options.cycles, 3);
    }

    #[test]
    fn rejects_unbounded_station_reconnect_timeout() {
        assert!(parse_options(&["--timeout-seconds".into(), "301".into()]).is_err());
    }

    #[test]
    fn rejects_unbounded_station_reconnect_cycles() {
        assert!(parse_options(&["--cycles".into(), "0".into()]).is_err());
        assert!(parse_options(&["--cycles".into(), "9".into()]).is_err());
    }

    #[test]
    fn stale_cycle_markers_cannot_qualify_another_cycle() {
        let evidence = cycle_evidence(1);
        assert!(!validate_cycle_progress(evidence, evidence, 2).unwrap());
    }

    #[test]
    fn connected_entry_waits_for_the_new_task_topology() {
        let before = cycle_evidence(1);
        let mut after = cycle_evidence(2);
        after.connected_ready = before.connected_ready;
        assert!(!validate_cycle_progress(before, after, 2).unwrap());
    }

    #[test]
    fn each_completed_cycle_requires_fresh_owner_evidence() {
        let before = cycle_evidence(1);
        let mut after = cycle_evidence(2);
        after.running_scan_owner_return = before.running_scan_owner_return;
        assert!(validate_cycle_progress(before, after, 2).is_err());
        assert!(validate_cycle_progress(before, cycle_evidence(2), 2).unwrap());
    }
}
