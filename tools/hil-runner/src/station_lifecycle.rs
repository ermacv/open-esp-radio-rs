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
const RECONNECTED_MARKER: &str = "result=PASS stage=production-reconnect-connected-enter";
const RECONNECT_FAILURE_MARKER: &str = "result=FAIL stage=production-reconnect";
const RUNNER_STOP_MARKER: &str = "result=PASS stage=production-runner-stop";
const RUNNING_SCAN_MARKER: &str = "result=PASS stage=production-running-scan channels=13";
const RUNNING_SCAN_LIFECYCLE_MARKER: &str = "refresh_candidate=1 phase=running-scan";
const RUNNING_SCAN_OWNER_RETURN_MARKER: &str =
    "result=PASS stage=production-running-scan-owner-return";
const RECONNECT_AUTHENTICATION_MARKER: &str =
    "result=PASS stage=production-reconnect-authentication";

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
    let output = root.join("target/hil/esp32s31/qualification/station-reconnect");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, options.timeout);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    result?;
    println!("station_reconnect=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(capture: &SerialCapture, timeout: Duration) -> Result<()> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.station_epoch_control {
        return Err("firmware does not advertise station epoch control".into());
    }
    capture.request_station_epoch_cycle()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.contains(RECONNECT_FAILURE_MARKER) {
            return Err("target reported a reconnect failure".into());
        }
        if capture.contains(RECONNECTED_MARKER) {
            if !capture.contains(RUNNER_STOP_MARKER) {
                return Err("target entered reconnect without a qualified runner stop".into());
            }
            if !capture.contains(RUNNING_SCAN_MARKER) {
                return Err("target entered reconnect without a complete running scan".into());
            }
            if !capture.contains(RUNNING_SCAN_LIFECYCLE_MARKER) {
                return Err(
                    "target entered reconnect without an outer lifecycle scan phase".into(),
                );
            }
            if !capture.contains(RUNNING_SCAN_OWNER_RETURN_MARKER) {
                return Err(
                    "target entered reconnect without returning running-scan owners".into(),
                );
            }
            if !capture.contains(RECONNECT_AUTHENTICATION_MARKER) {
                return Err(
                    "target entered reconnect without refreshed Open Authentication".into(),
                );
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "target did not enter the second connected epoch within {} seconds",
        timeout.as_secs()
    )
    .into())
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
                if !(10..=300).contains(&seconds) {
                    return Err("--timeout-seconds must be in 10..=300".into());
                }
                options.timeout = Duration::from_secs(seconds);
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
         --timeout-seconds <n>    complete-cycle deadline, 10..=300 (default 90)\n\n\
         Resets the flashed radio image, provisions credentials through the \n\
         typed UART protocol, requests a safe connected-runner stop, and \n\
         waits for the second Association/WPA2/connected epoch."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_station_reconnect_options() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--timeout-seconds".into(),
            "120".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
        assert_eq!(options.timeout, Duration::from_secs(120));
    }

    #[test]
    fn rejects_unbounded_station_reconnect_timeout() {
        assert!(parse_options(&["--timeout-seconds".into(), "301".into()]).is_err());
    }
}
