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
    controlled_ap::{ControlledAp, require_station_credentials},
    lab_config::LabConfig,
    scenario::PhyExpectation,
    traffic_capture::SerialCapture,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_MONITOR_DURATION: Duration = Duration::from_secs(3);
const DEFAULT_SCAN_DWELL_MILLIS: u16 = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Stop,
    Start,
    Scan,
    Monitor,
    AccessPoint,
    Roundtrip,
}

impl Operation {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "stop" => Ok(Self::Stop),
            "start" => Ok(Self::Start),
            "scan" => Ok(Self::Scan),
            "monitor" => Ok(Self::Monitor),
            "ap" => Ok(Self::AccessPoint),
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
            Self::AccessPoint => "ap",
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
}

pub(crate) fn run(
    operation: &str,
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    phy: PhyExpectation,
) -> Result<()> {
    let operation = Operation::parse(operation)?;
    let options = parse_options(&arguments, lab)?;
    let _access_point = if operation == Operation::AccessPoint {
        require_station_credentials(&lab.station)?;
        None
    } else {
        Some(ControlledAp::start(
            &lab.station,
            &lab.station_fixture,
            phy,
        )?)
    };
    fs::create_dir_all(output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, lab, operation, &options);
    capture.finish_to(output)?;
    result?;
    println!("wifi_{}=PASS", operation.name());
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(
    capture: &SerialCapture,
    lab: &LabConfig,
    operation: Operation,
    options: &Options,
) -> Result<()> {
    let capabilities = capture.prepare_station(lab, options.timeout)?;
    if !capabilities.features.wifi_role_control {
        return Err("firmware does not advertise explicit Wi-Fi role control".into());
    }
    report_stack(capture, options.timeout, "connected")?;

    let stopped = stop_station(capture, options.timeout)?;
    report_stack(capture, options.timeout, "station-stopped")?;
    println!("wifi_station_stopped_generation={}", stopped.generation);
    if operation == Operation::Stop {
        return Ok(());
    }
    if operation == Operation::Start {
        start_station(capture, lab, options.timeout)?;
        report_stack(capture, options.timeout, "station-started")?;
        return Ok(());
    }

    if operation == Operation::AccessPoint {
        if !capabilities.features.wifi_access_point {
            return Err("firmware does not advertise the access-point role".into());
        }
        let mut request = lab.access_point.protocol_request()?;
        if let Some(channel) = options.monitor_channel {
            request.channel = channel;
        }
        let channel = request.channel;
        let started = capture.wait_access_point_start(
            capture.request_access_point_start(request)?,
            options.timeout,
        )?;
        require_transition(started, WifiRole::Idle, WifiRole::AccessPoint)?;
        report_stack(capture, options.timeout, "access-point-started")?;
        thread::sleep(options.monitor_duration);
        let stopped = capture
            .wait_access_point_stop(capture.request_access_point_stop()?, options.timeout)?;
        if stopped.generation != started.generation
            || stopped.channel != channel
            || stopped.beacons_transmitted == 0
        {
            return Err(format!("access point returned inconsistent evidence: {stopped:?}").into());
        }
        println!(
            "wifi_ap_generation={} channel={} beacons={} auth_responses={} assoc_responses={} authorizations={} max_associated={} max_authorized={} peer_removals={} auth_timeouts={} inactivity_timeouts={} disassoc_prepared={} disassoc_published={} disassoc_acked={} deauth_prepared={} deauth_published={} deauth_acked={} rx_units={} rx_descriptors={} recycled_rx_descriptors={} discarded_rx_units={} ignored_rx={} control_staged={} control_busy_drops={} ethernet_staged={} network_tx_rejected={} data_tx={} tx_hardware_failures={} tx_hardware_timeouts={} tx_collision_limits={} tx_last_hardware_status={}",
            stopped.generation,
            stopped.channel,
            stopped.beacons_transmitted,
            stopped.authentication_responses,
            stopped.association_responses,
            stopped.authorized_peers,
            stopped.maximum_associated_peers,
            stopped.maximum_authorized_peers,
            stopped.peer_removals,
            stopped.authentication_timeouts,
            stopped.inactivity_timeouts,
            stopped.disassociations_prepared,
            stopped.disassociations_published,
            stopped.disassociations_acknowledged,
            stopped.deauthentications_prepared,
            stopped.deauthentications_published,
            stopped.deauthentications_acknowledged,
            stopped.completed_rx_units,
            stopped.completed_rx_descriptors,
            stopped.recycled_rx_descriptors,
            stopped.discarded_rx_units,
            stopped.ignored_rx_frames,
            stopped.control_frames_staged,
            stopped.control_frames_dropped_while_busy,
            stopped.ethernet_frames_staged,
            stopped.network_tx_frames_rejected,
            stopped.data_frames_transmitted,
            stopped.tx_hardware_failures,
            stopped.tx_hardware_timeouts,
            stopped.tx_collision_limits,
            stopped.tx_last_hardware_status,
        );
        start_station(capture, lab, options.timeout)?;
        report_stack(capture, options.timeout, "station-restarted")?;
        return Ok(());
    }

    let scan = scan(capture, options.timeout)?;
    report_stack(capture, options.timeout, "scan-complete")?;
    println!(
        "wifi_scan_generation={} elapsed_micros={} observed_frames={} unique_bss={} configured_ssid_channel={} configured_ssid_rssi_dbm={}",
        scan.generation,
        scan.elapsed_micros,
        scan.observed_frames,
        scan.unique_bss,
        scan.configured_ssid_channel,
        scan.configured_ssid_rssi_dbm,
    );
    if operation == Operation::Scan {
        start_station(capture, lab, options.timeout)?;
        report_stack(capture, options.timeout, "station-restarted")?;
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
    report_stack(capture, options.timeout, "monitor-stopped")?;
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
    validate_elapsed(
        "monitor lifecycle",
        stopped.elapsed_micros,
        options.monitor_duration,
        150,
    )?;
    println!(
        "wifi_monitor_generation={} elapsed_micros={} channel={} captured_frames={} captured_bytes={} channel_unavailable={} last_observed_channel={}",
        stopped.generation,
        stopped.elapsed_micros,
        stopped.channel,
        stopped.captured_frames,
        stopped.captured_bytes,
        stopped.channel_unavailable,
        stopped.last_observed_channel,
    );
    start_station(capture, lab, options.timeout)?;
    report_stack(capture, options.timeout, "station-restarted")?;
    Ok(())
}

pub(crate) fn report_stack(capture: &SerialCapture, timeout: Duration, stage: &str) -> Result<()> {
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

pub(crate) fn stop_station(
    capture: &SerialCapture,
    timeout: Duration,
) -> Result<WifiRoleTransitionEvidence> {
    let evidence = capture.wait_wifi_role_transition(capture.request_station_stop()?, timeout)?;
    require_transition(evidence, WifiRole::Station, WifiRole::Idle)?;
    Ok(evidence)
}

pub(crate) fn start_station(
    capture: &SerialCapture,
    lab: &LabConfig,
    timeout: Duration,
) -> Result<()> {
    let evidence =
        capture.wait_wifi_role_transition(capture.request_station_start(lab)?, timeout)?;
    require_transition(evidence, WifiRole::Idle, WifiRole::Station)?;
    Ok(())
}

pub(crate) fn scan(capture: &SerialCapture, timeout: Duration) -> Result<WifiScanEvidence> {
    let request = WifiScanRequest {
        channel_mask_2_4_ghz: 0x1fff,
        dwell_millis: DEFAULT_SCAN_DWELL_MILLIS,
    };
    let evidence = capture.wait_wifi_scan(capture.request_wifi_scan(request)?, timeout)?;
    if !evidence.configured_ssid_found {
        return Err(format!("scan did not find the configured SSID: {evidence:?}").into());
    }
    let expected = Duration::from_millis(
        u64::from(request.dwell_millis) * u64::from(request.channel_mask_2_4_ghz.count_ones()),
    );
    validate_elapsed("standalone scan", evidence.elapsed_micros, expected, 150)?;
    Ok(evidence)
}

fn validate_elapsed(
    operation: &str,
    observed_micros: u64,
    expected: Duration,
    maximum_percent: u64,
) -> Result<()> {
    let expected_micros = expected.as_micros().min(u128::from(u64::MAX)) as u64;
    let minimum = expected_micros * 95 / 100;
    let maximum = expected_micros * maximum_percent / 100;
    if observed_micros < minimum || observed_micros > maximum {
        return Err(format!(
            "{operation} timing is outside the qualified range: expected_us={expected_micros} observed_us={observed_micros} range={minimum}..={maximum}"
        )
        .into());
    }
    Ok(())
}

pub(crate) fn require_transition(
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

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        serial: lab.device.serial.clone(),
        timeout: DEFAULT_TIMEOUT,
        monitor_duration: DEFAULT_MONITOR_DURATION,
        monitor_channel: None,
        snapshot_length: 256,
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
            argument => return Err(format!("unknown Wi-Fi lifecycle option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_monitor_controls() {
        let options = parse_options(
            &[
                "--channel".into(),
                "11".into(),
                "--monitor-seconds".into(),
                "5".into(),
                "--snapshot-length".into(),
                "512".into(),
            ],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.monitor_channel, Some(11));
        assert_eq!(options.monitor_duration, Duration::from_secs(5));
        assert_eq!(options.snapshot_length, 512);
    }

    #[test]
    fn rejects_invalid_monitor_channel() {
        assert!(parse_options(&["--channel".into(), "14".into()], &LabConfig::for_test()).is_err());
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
