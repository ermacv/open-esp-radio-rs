//! Host qualification for explicit Wi-Fi role ownership transitions.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    WifiMonitorRequest, WifiRole, WifiRoleTransitionEvidence, WifiScanEvidence, WifiScanRequest,
};

use crate::{
    Result,
    controlled_ap::{
        ControlledAp, require_controlled_ap_credentials_environment,
        require_station_credentials_environment,
    },
    traffic_capture::SerialCapture,
};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_MONITOR_DURATION: Duration = Duration::from_secs(3);
const DEFAULT_SCAN_DWELL_MILLIS: u16 = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Stop,
    Start,
    Scan,
    Monitor,
    Roundtrip,
}

impl Operation {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "stop" => Ok(Self::Stop),
            "start" => Ok(Self::Start),
            "scan" => Ok(Self::Scan),
            "monitor" => Ok(Self::Monitor),
            "roundtrip" => Ok(Self::Roundtrip),
            _ => Err(format!("unknown Wi-Fi lifecycle operation `{value}`").into()),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Scan => "scan",
            Self::Monitor => "monitor",
            Self::Roundtrip => "roundtrip",
        }
    }
}

struct Options {
    serial: PathBuf,
    timeout: Duration,
    monitor_duration: Duration,
    monitor_channel: Option<u8>,
    snapshot_length: u16,
    external_ap: bool,
}

pub(crate) fn run(operation: &str, arguments: Vec<String>, root: &Path) -> Result<()> {
    let operation = Operation::parse(operation)?;
    if arguments
        .first()
        .is_some_and(|value| matches!(value.as_str(), "help" | "--help" | "-h"))
    {
        print_help(operation);
        return Ok(());
    }
    let options = parse_options(&arguments)?;
    let _access_point = if options.external_ap {
        require_station_credentials_environment()?;
        None
    } else {
        require_controlled_ap_credentials_environment()?;
        Some(ControlledAp::start()?)
    };
    let output = root.join(format!(
        "target/hil/esp32s31/qualification/wifi-{}",
        operation.name()
    ));
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, operation, &options);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    result?;
    println!("wifi_{}=PASS", operation.name());
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(capture: &SerialCapture, operation: Operation, options: &Options) -> Result<()> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.wifi_role_control {
        return Err("firmware does not advertise explicit Wi-Fi role control".into());
    }
    capture.wait_for_connected_station(options.timeout)?;

    let stopped = stop_station(capture, options.timeout)?;
    println!("wifi_station_stopped_generation={}", stopped.generation);
    if operation == Operation::Stop {
        return Ok(());
    }
    if operation == Operation::Start {
        start_station(capture, options.timeout)?;
        return Ok(());
    }

    let scan = scan(capture, options.timeout)?;
    println!(
        "wifi_scan_generation={} observed_frames={} unique_bss={} configured_ssid_channel={} configured_ssid_rssi_dbm={}",
        scan.generation,
        scan.observed_frames,
        scan.unique_bss,
        scan.configured_ssid_channel,
        scan.configured_ssid_rssi_dbm,
    );
    if operation == Operation::Scan {
        start_station(capture, options.timeout)?;
        return Ok(());
    }

    let channel = options
        .monitor_channel
        .unwrap_or(scan.configured_ssid_channel);
    if !(1..=13).contains(&channel) {
        return Err("scan did not provide a valid configured-network channel".into());
    }
    let started = capture.wait_monitor_start(
        capture.request_monitor_start(WifiMonitorRequest {
            channel,
            snapshot_length: options.snapshot_length,
        })?,
        options.timeout,
    )?;
    require_transition(started, WifiRole::Idle, WifiRole::Monitor)?;
    thread::sleep(options.monitor_duration);
    let stopped = capture.wait_monitor_stop(capture.request_monitor_stop()?, options.timeout)?;
    if stopped.generation != started.generation
        || stopped.channel != channel
        || stopped.generation_mismatches != 0
        || stopped.channel_mismatches != 0
    {
        return Err(format!("monitor returned inconsistent evidence: {stopped:?}").into());
    }
    if stopped.captured_frames == 0 || stopped.captured_bytes == 0 {
        return Err("monitor epoch captured no frames".into());
    }
    println!(
        "wifi_monitor_generation={} channel={} captured_frames={} captured_bytes={} channel_unavailable={} last_observed_channel={}",
        stopped.generation,
        stopped.channel,
        stopped.captured_frames,
        stopped.captured_bytes,
        stopped.channel_unavailable,
        stopped.last_observed_channel,
    );
    start_station(capture, options.timeout)?;
    Ok(())
}

pub(crate) fn stop_station(
    capture: &SerialCapture,
    timeout: Duration,
) -> Result<WifiRoleTransitionEvidence> {
    let evidence = capture.wait_wifi_role_transition(capture.request_station_stop()?, timeout)?;
    require_transition(evidence, WifiRole::Station, WifiRole::Idle)?;
    Ok(evidence)
}

pub(crate) fn start_station(capture: &SerialCapture, timeout: Duration) -> Result<()> {
    let evidence = capture.wait_wifi_role_transition(capture.request_station_start()?, timeout)?;
    require_transition(evidence, WifiRole::Idle, WifiRole::Station)?;
    Ok(())
}

pub(crate) fn scan(capture: &SerialCapture, timeout: Duration) -> Result<WifiScanEvidence> {
    let evidence = capture.wait_wifi_scan(
        capture.request_wifi_scan(WifiScanRequest {
            channel_mask_2_4_ghz: 0x1fff,
            dwell_millis: DEFAULT_SCAN_DWELL_MILLIS,
        })?,
        timeout,
    )?;
    if !evidence.configured_ssid_found {
        return Err(format!("scan did not find the configured SSID: {evidence:?}").into());
    }
    Ok(evidence)
}

fn require_transition(
    evidence: WifiRoleTransitionEvidence,
    previous: WifiRole,
    current: WifiRole,
) -> Result<()> {
    if evidence.previous != previous || evidence.current != current {
        return Err(format!(
            "unexpected Wi-Fi role transition: expected {previous:?}->{current:?}, got {evidence:?}"
        )
        .into());
    }
    Ok(())
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
        timeout: DEFAULT_TIMEOUT,
        monitor_duration: DEFAULT_MONITOR_DURATION,
        monitor_channel: None,
        snapshot_length: 256,
        external_ap: false,
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
                if !(10..=180).contains(&seconds) {
                    return Err("--timeout-seconds must be in 10..=180".into());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            "--monitor-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--monitor-seconds requires a value")?
                    .parse::<u64>()?;
                if !(1..=30).contains(&seconds) {
                    return Err("--monitor-seconds must be in 1..=30".into());
                }
                options.monitor_duration = Duration::from_secs(seconds);
            }
            "--channel" => {
                index += 1;
                let channel = arguments
                    .get(index)
                    .ok_or("--channel requires a value")?
                    .parse::<u8>()?;
                if !(1..=13).contains(&channel) {
                    return Err("--channel must be in 1..=13".into());
                }
                options.monitor_channel = Some(channel);
            }
            "--snapshot-length" => {
                index += 1;
                let length = arguments
                    .get(index)
                    .ok_or("--snapshot-length requires a value")?
                    .parse::<u16>()?;
                if length > 2_304 {
                    return Err("--snapshot-length must be in 0..=2304".into());
                }
                options.snapshot_length = length;
            }
            "--external-ap" => options.external_ap = true,
            argument => return Err(format!("unknown Wi-Fi lifecycle option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn print_help(operation: Operation) {
    println!(
        "cargo hil wifi {} [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --timeout-seconds <n>    role-transition deadline, 10..=180 (default 90)\n\
         --external-ap            use a caller-owned access point\n\
         --monitor-seconds <n>    monitor dwell, 1..=30 (default 3)\n\
         --channel <1..=13>       monitor channel; default is discovered by scan\n\
         --snapshot-length <n>    0 for complete frames, otherwise 1..=2304",
        operation.name()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_controls() {
        let options = parse_options(&[
            "--channel".into(),
            "11".into(),
            "--monitor-seconds".into(),
            "5".into(),
            "--snapshot-length".into(),
            "512".into(),
            "--external-ap".into(),
        ])
        .unwrap();
        assert_eq!(options.monitor_channel, Some(11));
        assert_eq!(options.monitor_duration, Duration::from_secs(5));
        assert_eq!(options.snapshot_length, 512);
        assert!(options.external_ap);
    }

    #[test]
    fn rejects_invalid_monitor_channel() {
        assert!(parse_options(&["--channel".into(), "14".into()]).is_err());
    }

    #[test]
    fn role_transition_is_exact() {
        let evidence = WifiRoleTransitionEvidence {
            previous: WifiRole::Station,
            current: WifiRole::Idle,
            generation: 3,
        };
        assert!(require_transition(evidence, WifiRole::Station, WifiRole::Idle).is_ok());
        assert!(require_transition(evidence, WifiRole::Idle, WifiRole::Station).is_err());
    }
}
