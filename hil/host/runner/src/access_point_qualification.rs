//! Single-client WPA2 AP lifecycle and exact data-plane qualification.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    process::Command,
    thread,
    time::Duration,
};

use serde::Serialize;

use open_esp_radio_hil_protocol::{
    Completion, Direction as ProtocolDirection, FlowConfig, Ipv4Endpoint, SessionConfig,
    SessionLinkRequirements, Transport, WifiRole,
};

use crate::{
    Result,
    controlled_client::ControlledClient,
    lab_config::LabConfig,
    paced_tcp::{
        Config as TcpConfig, HostReception as TcpReception, HostTransmission as TcpTransmission,
        exchange as exchange_tcp, receive as receive_tcp, send as send_tcp,
    },
    paced_udp::{Config as UdpConfig, HostTransmission as UdpTransmission, send as send_udp},
    scenario::{AccessPointTraffic, Criteria, Direction},
    traffic_capture::{SerialCapture, SessionEvidence},
    tx_traffic::{Burst, receive_bursts},
    udp_socket::{configure_qualification_receive_buffer, open_reverse_flow},
    wifi_control::{report_stack, require_transition, start_station, stop_station},
};

const UDP_RX_PORT: u16 = 4_323;
const UDP_TX_SOURCE_PORT: u16 = 4_324;
const UDP_HOST_PORT: u16 = 9_002;
const TCP_PORT: u16 = 4_325;

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) cycles: u8,
    pub(crate) boots: u8,
    pub(crate) timeout: Duration,
    pub(crate) traffic: AccessPointTraffic,
    pub(crate) criteria: Criteria,
}

#[derive(Serialize)]
struct AccessPointReport {
    schema: u8,
    boots: Vec<BootReport>,
}

#[derive(Serialize)]
struct BootReport {
    boot: u8,
    cycles: Vec<CycleReport>,
}

#[derive(Serialize)]
struct CycleReport {
    cycle: u8,
    traffic: TrafficReport,
    access_point: open_esp_radio_hil_protocol::WifiAccessPointEvidence,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum TrafficReport {
    None,
    Icmp {
        transmitted: u16,
        received: u16,
        lost: u16,
        p95_micros: u64,
    },
    Udp(SessionReport),
    Tcp(SessionReport),
}

#[derive(Serialize)]
struct SessionReport {
    direction: Direction,
    rx_bytes: u64,
    tx_bytes: u64,
    rx_units: u64,
    tx_units: u64,
    elapsed_micros: u64,
}

#[derive(Clone, Copy)]
struct UdpWorkload {
    direction: Direction,
    duration: Duration,
    rx_rate_bps: Option<u64>,
    tx_rate_bps: Option<u64>,
    payload_bytes: usize,
}

#[derive(Clone, Copy)]
struct TcpWorkload {
    direction: Direction,
    duration: Duration,
    rx_rate_bps: Option<u64>,
    tx_rate_bps: Option<u64>,
    chunk_bytes: usize,
}

pub(crate) fn run(config: Config, output: &Path, lab: &LabConfig) -> Result<()> {
    fs::create_dir_all(output)?;
    let mut report = AccessPointReport {
        schema: 1,
        boots: Vec::with_capacity(usize::from(config.boots)),
    };
    for boot in 0..config.boots {
        let boot_output = if config.boots == 1 {
            output.to_owned()
        } else {
            output.join(format!("boot-{boot:02}"))
        };
        fs::create_dir_all(&boot_output)?;
        let capture = SerialCapture::start_with_reset(&lab.device.serial);
        let cycles = qualify(&capture, &config, lab);
        let capture_result = capture.finish_to(&boot_output);
        let cycles = cycles?;
        capture_result?;
        report.boots.push(BootReport { boot, cycles });
    }
    fs::write(
        output.join("access-point-report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(())
}

fn qualify(capture: &SerialCapture, config: &Config, lab: &LabConfig) -> Result<Vec<CycleReport>> {
    let capabilities = capture.prepare_station(lab, config.timeout)?;
    if !capabilities.features.wifi_role_control || !capabilities.features.wifi_access_point {
        return Err("firmware does not advertise AP role control".into());
    }
    capture.wait_for_connected_station(config.timeout)?;
    report_stack(capture, config.timeout, "ap-initial-station-connected")?;

    let mut cycles = Vec::with_capacity(usize::from(config.cycles));
    for cycle in 0..config.cycles {
        // Validate all host-owned inputs before releasing the connected STA.
        // An invalid AP request must not strand the target in Idle.
        let request = lab.access_point.protocol_request()?;
        let _ = stop_station(capture, config.timeout)?;
        if let Err(error) = report_stack(capture, config.timeout, "ap-station-stopped") {
            let restart = restore_connected_station(capture, config.timeout, lab).err();
            return Err(with_cleanup_errors(error, None, None, None, restart));
        }
        let start = match capture.request_access_point_start(request) {
            Ok(start) => start,
            Err(error) => {
                let restart = restore_connected_station(capture, config.timeout, lab).err();
                return Err(with_cleanup_errors(error, None, None, None, restart));
            }
        };
        let started = capture.wait_access_point_start(start, config.timeout);
        let started = match started {
            Ok(started) => started,
            Err(error) => {
                let restart = restore_connected_station(capture, config.timeout, lab).err();
                return Err(with_cleanup_errors(error, None, None, None, restart));
            }
        };
        if let Err(error) = require_transition(started, WifiRole::Idle, WifiRole::AccessPoint) {
            return Err(cleanup_after_client_failure(
                capture,
                config.timeout,
                started.generation,
                lab,
                error,
            ));
        }
        if let Err(error) = report_stack(capture, config.timeout, "ap-started") {
            return Err(cleanup_after_client_failure(
                capture,
                config.timeout,
                started.generation,
                lab,
                error,
            ));
        }

        let client = match ControlledClient::connect(&lab.access_point) {
            Ok(client) => client,
            Err(error) => {
                return Err(cleanup_after_client_failure(
                    capture,
                    config.timeout,
                    started.generation,
                    lab,
                    error,
                ));
            }
        };
        let data_result = qualify_data_plane(capture, config, lab);
        let client_restore = client.restore();
        let stop_result = stop_access_point(capture, config.timeout, started.generation, lab);
        let stop_stack_result = if stop_result.is_ok() {
            report_stack(capture, config.timeout, "ap-stopped")
        } else {
            Ok(())
        };
        let restart_result = if stop_result.is_ok() {
            restore_connected_station(capture, config.timeout, lab)
        } else {
            Ok(())
        };

        if let Err(error) = &data_result {
            let data_error = match stop_result.as_ref() {
                Ok(stopped) => format!(
                    "{error}; AP TX evidence: data={} hardware_failures={} hardware_timeouts={} \
                     collision_limits={} last_hardware_status={} beacons={}; AP RX evidence: \
                     descriptors={} protected={} mic_failures={} quarantined={} duplicates={} \
                     radio_rejected={} protocol_rejected={} ethernet_staged={} tcp_staged={}",
                    stopped.data_frames_transmitted,
                    stopped.tx_hardware_failures,
                    stopped.tx_hardware_timeouts,
                    stopped.tx_collision_limits,
                    stopped.tx_last_hardware_status,
                    stopped.beacons_transmitted,
                    stopped.completed_rx_descriptors,
                    stopped.protected_data_frames,
                    stopped.rx_mic_failures,
                    stopped.rx_quarantined_frames,
                    stopped.protected_data_duplicates,
                    stopped.protected_data_radio_rejected,
                    stopped.protected_data_protocol_rejected,
                    stopped.ethernet_frames_staged,
                    stopped.ethernet_tcp_frames_staged,
                ),
                Err(_) => error.to_string(),
            };
            return Err(with_cleanup_errors(
                data_error,
                client_restore.err(),
                stop_result.as_ref().err().map(|error| error.as_ref()),
                stop_stack_result.err(),
                restart_result.err(),
            ));
        }
        let traffic = data_result?;
        client_restore?;
        let stopped = stop_result?;
        stop_stack_result?;
        restart_result?;
        if stopped.beacons_transmitted == 0
            || stopped.missed_beacon_intervals != 0
            || stopped.maximum_beacon_lateness_micros >= 102_400
            || stopped.authentication_responses == 0
            || stopped.association_responses == 0
            || stopped.authorized_peers == 0
            || stopped.tx_hardware_failures != 0
            || stopped.tx_hardware_timeouts != 0
            || stopped.tx_collision_limits != 0
            || stopped.control_frames_dropped_while_busy != 0
            || stopped.rx_mic_failures != 0
            || stopped.rx_quarantined_frames != 0
            || stopped.protected_data_radio_rejected != 0
            || stopped.protected_data_protocol_rejected != 0
        {
            return Err(
                format!("AP cycle {cycle} lacks peer-visible MAC evidence: {stopped:?}").into(),
            );
        }
        cycles.push(CycleReport {
            cycle,
            traffic,
            access_point: stopped,
        });
    }
    Ok(cycles)
}

fn qualify_data_plane(
    capture: &SerialCapture,
    config: &Config,
    lab: &LabConfig,
) -> Result<TrafficReport> {
    match &config.traffic {
        AccessPointTraffic::None => Ok(TrafficReport::None),
        AccessPointTraffic::Icmp {
            count,
            interval_ms,
            timeout_ms,
            payload_bytes,
        } => qualify_icmp(
            lab.access_point.target_address(),
            *count,
            *interval_ms,
            *timeout_ms,
            *payload_bytes,
            &config.criteria,
        ),
        AccessPointTraffic::Udp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            payload_bytes,
        } => qualify_udp(
            capture,
            config,
            lab,
            UdpWorkload {
                direction: *direction,
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                rx_rate_bps: *rx_rate_bps,
                tx_rate_bps: *tx_rate_bps,
                payload_bytes: usize::from(*payload_bytes),
            },
        ),
        AccessPointTraffic::Tcp {
            direction,
            duration_seconds,
            rx_rate_bps,
            tx_rate_bps,
            chunk_bytes,
        } => qualify_tcp(
            capture,
            config,
            lab.access_point.target_address(),
            TcpWorkload {
                direction: *direction,
                duration: Duration::from_secs(u64::from(*duration_seconds)),
                rx_rate_bps: *rx_rate_bps,
                tx_rate_bps: *tx_rate_bps,
                chunk_bytes: usize::from(*chunk_bytes),
            },
        ),
    }
}

fn qualify_udp(
    capture: &SerialCapture,
    config: &Config,
    lab: &LabConfig,
    workload: UdpWorkload,
) -> Result<TrafficReport> {
    let UdpWorkload {
        direction,
        duration,
        rx_rate_bps,
        tx_rate_bps,
        payload_bytes,
    } = workload;
    let protocol_direction = protocol_direction(direction);
    let target = lab.access_point.target_address();
    let host = lab.access_point.client_address();
    let socket = if tx_rate_bps.is_some() {
        let socket = UdpSocket::bind(SocketAddrV4::new(host, UDP_HOST_PORT))?;
        configure_qualification_receive_buffer(&socket)?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        socket.connect(SocketAddrV4::new(target, UDP_TX_SOURCE_PORT))?;
        open_reverse_flow(&socket)?;
        Some(socket)
    } else {
        None
    };
    let duration_millis = u32::try_from(duration.as_millis())?;
    let session = capture.start_session(SessionConfig {
        transport: Transport::Udp,
        direction: protocol_direction,
        completion: Completion::DurationMillis(duration_millis),
        peer: tx_rate_bps.map(|_| Ipv4Endpoint {
            address: host.octets(),
            port: UDP_HOST_PORT,
        }),
        target_rx: rx_rate_bps.map(|rate| FlowConfig {
            payload_bytes: u16::try_from(payload_bytes).expect("validated UDP payload"),
            offered_rate_bps: Some(rate),
        }),
        target_tx: tx_rate_bps.map(|rate| FlowConfig {
            payload_bytes: u16::try_from(payload_bytes).expect("validated UDP payload"),
            offered_rate_bps: Some(rate),
        }),
        // AP v1 deliberately qualifies the legacy unicast TX path. Block Ack
        // is not part of its current public capability contract.
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let send_config = rx_rate_bps.map(|rate| UdpConfig {
        address: target,
        port: UDP_RX_PORT,
        rate_bps: rate,
        duration,
        payload: payload_bytes,
    });
    let receive_duration = duration.saturating_add(Duration::from_secs(2));
    let data_plane = match (send_config, socket) {
        (Some(send_config), Some(socket)) => {
            let sender = thread::spawn(move || send_udp(send_config).map_err(|e| e.to_string()));
            let received = receive_bursts(&socket, target, receive_duration);
            let sent = sender
                .join()
                .map_err(|_| "AP UDP sender thread panicked")??;
            Ok((Some(sent), Some(received?)))
        }
        (Some(send_config), None) => send_udp(send_config).map(|sent| (Some(sent), None)),
        (None, Some(socket)) => receive_bursts(&socket, target, receive_duration)
            .map(|received| (None, Some(received)))
            .map_err(Into::into),
        (None, None) => Err("AP UDP workload has no data direction".into()),
    };

    let structured = capture.wait_for_session(session, config.timeout);
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let (host_tx, host_rx) =
        data_plane.map_err(|error| format!("AP UDP host path failed: {error}"))?;
    let structured = structured.map_err(|error| format!("AP UDP target failed: {error}"))?;
    acknowledgement?;
    let report = session_report(direction, &structured);
    validate_udp(host_tx, host_rx.as_deref(), structured)?;
    validate_rate_criteria(&report, &config.criteria)?;
    Ok(TrafficReport::Udp(report))
}

fn validate_udp(
    host_tx: Option<UdpTransmission>,
    host_rx: Option<&[Burst]>,
    evidence: SessionEvidence,
) -> Result<()> {
    if !evidence.finished.summary.passed || evidence.transport.transport_errors != 0 {
        return Err(format!(
            "AP UDP target failed: passed={} errors={}",
            evidence.finished.summary.passed, evidence.transport.transport_errors,
        )
        .into());
    }
    match host_tx {
        Some(host) => {
            if host.bytes != evidence.transport.rx_bytes
                || host.datagrams != evidence.transport.rx_units
            {
                return Err(format!(
                    "AP UDP RX mismatch: host={}/{} target={}/{}",
                    host.bytes,
                    host.datagrams,
                    evidence.transport.rx_bytes,
                    evidence.transport.rx_units,
                )
                .into());
            }
        }
        None if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 => {
            return Err("AP UDP TX-only session reported received traffic".into());
        }
        None => {}
    }
    match host_rx {
        Some(bursts) => {
            let qualified: Vec<_> = bursts
                .iter()
                .copied()
                .filter(|burst| burst.started_at_zero)
                .collect();
            if qualified.len() != 1 {
                return Err(format!(
                    "AP UDP TX produced {} zero-started bursts instead of one",
                    qualified.len()
                )
                .into());
            }
            let host = qualified[0];
            if host.missing != 0 || host.reordered != 0 || host.duplicates != 0 {
                return Err(format!(
                    "AP UDP TX ordering defect: missing={} reordered={} duplicates={} \
                     sequence_range={}..={} missing_runs={} largest_missing_run={} \
                     largest_missing_range={:?}..={:?} maximum_interarrival_us={} \
                     sequence_after_maximum_interarrival={:?}",
                    host.missing,
                    host.reordered,
                    host.duplicates,
                    host.lowest_sequence,
                    host.highest_sequence,
                    host.missing_runs,
                    host.maximum_missing_run,
                    host.maximum_missing_run_start,
                    host.maximum_missing_run_end,
                    host.maximum_interarrival_us,
                    host.sequence_after_maximum_interarrival,
                )
                .into());
            }
            if host.bytes != evidence.transport.tx_bytes
                || host.datagrams != evidence.transport.tx_units
            {
                return Err(format!(
                    "AP UDP TX mismatch: target={}/{} host={}/{}",
                    evidence.transport.tx_bytes,
                    evidence.transport.tx_units,
                    host.bytes,
                    host.datagrams,
                )
                .into());
            }
        }
        None if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 => {
            return Err("AP UDP RX-only session reported transmitted traffic".into());
        }
        None => {}
    }
    Ok(())
}

fn qualify_tcp(
    capture: &SerialCapture,
    config: &Config,
    target: Ipv4Addr,
    workload: TcpWorkload,
) -> Result<TrafficReport> {
    let TcpWorkload {
        direction,
        duration,
        rx_rate_bps,
        tx_rate_bps,
        chunk_bytes,
    } = workload;
    let protocol_direction = protocol_direction(direction);
    let duration_millis = u32::try_from(duration.as_millis())?;
    let session = capture.start_session(SessionConfig {
        transport: Transport::Tcp,
        direction: protocol_direction,
        completion: Completion::DurationMillis(duration_millis),
        peer: None,
        target_rx: rx_rate_bps.map(|rate| FlowConfig {
            payload_bytes: u16::try_from(chunk_bytes).expect("validated TCP chunk"),
            offered_rate_bps: Some(rate),
        }),
        target_tx: tx_rate_bps.map(|rate| FlowConfig {
            payload_bytes: u16::try_from(chunk_bytes).expect("validated TCP chunk"),
            offered_rate_bps: Some(rate),
        }),
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let tcp = TcpConfig {
        address: target,
        port: TCP_PORT,
        rate_bps: rx_rate_bps
            .or(tx_rate_bps)
            .expect("validated AP TCP direction has a rate"),
        duration,
        chunk_bytes,
    };
    let data_plane = match direction {
        Direction::Rx => send_tcp(tcp).map(|sample| (Some(sample), None)),
        Direction::Tx => receive_tcp(tcp).map(|sample| (None, Some(sample))),
        Direction::Bidirectional => {
            exchange_tcp(tcp).map(|(sent, received)| (Some(sent), Some(received)))
        }
    };
    let structured = capture.wait_for_session(session, config.timeout);
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let (host_tx, host_rx) =
        data_plane.map_err(|error| format!("AP TCP host path failed: {error}"))?;
    let structured = structured.map_err(|error| format!("AP TCP target failed: {error}"))?;
    acknowledgement?;
    let report = session_report(direction, &structured);
    validate_tcp(host_tx, host_rx, structured)?;
    validate_rate_criteria(&report, &config.criteria)?;
    Ok(TrafficReport::Tcp(report))
}

fn validate_tcp(
    host_tx: Option<TcpTransmission>,
    host_rx: Option<TcpReception>,
    evidence: SessionEvidence,
) -> Result<()> {
    if !evidence.finished.summary.passed || evidence.transport.transport_errors != 0 {
        return Err(format!(
            "AP TCP target failed: passed={} errors={}",
            evidence.finished.summary.passed, evidence.transport.transport_errors,
        )
        .into());
    }
    match host_tx {
        Some(host)
            if host.bytes != evidence.transport.rx_bytes || evidence.transport.rx_units != 1 =>
        {
            return Err(format!(
                "AP TCP RX mismatch: host={} target={} units={}",
                host.bytes, evidence.transport.rx_bytes, evidence.transport.rx_units,
            )
            .into());
        }
        None if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 => {
            return Err("AP TCP TX-only session reported received traffic".into());
        }
        _ => {}
    }
    match host_rx {
        Some(host)
            if host.bytes != evidence.transport.tx_bytes
                || evidence.transport.tx_units != 1
                || !host.eof
                || host.pattern_errors != 0 =>
        {
            return Err(format!(
                "AP TCP TX mismatch: target={} host={} units={} eof={} pattern_errors={}",
                evidence.transport.tx_bytes,
                host.bytes,
                evidence.transport.tx_units,
                host.eof,
                host.pattern_errors,
            )
            .into());
        }
        None if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 => {
            return Err("AP TCP RX-only session reported transmitted traffic".into());
        }
        _ => {}
    }
    Ok(())
}

fn qualify_icmp(
    target: Ipv4Addr,
    count: u16,
    interval_ms: u16,
    timeout_ms: u16,
    payload_bytes: u16,
    criteria: &Criteria,
) -> Result<TrafficReport> {
    let interval_seconds = format!("{:.3}", f64::from(interval_ms) / 1_000.0);
    let timeout_seconds = format!("{:.3}", f64::from(timeout_ms) / 1_000.0);
    let output = Command::new("ping")
        .env("LC_ALL", "C")
        .args(["-I", "wlan0", "-c"])
        .arg(count.to_string())
        .args(["-i", &interval_seconds, "-W", &timeout_seconds, "-s"])
        .arg(payload_bytes.to_string())
        .arg(target.to_string())
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let mut samples_micros = stdout
        .lines()
        .filter_map(ping_sample_micros)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    samples_micros.sort_unstable();
    let received = u16::try_from(samples_micros.len())?;
    let lost = count.saturating_sub(received);
    let allowed_lost = criteria.maximum_lost.unwrap_or(0);
    if u32::from(lost) > allowed_lost {
        return Err(format!(
            "AP ICMP lost {lost}/{count} packets (allowed {allowed_lost}); output: {}",
            stdout.trim()
        )
        .into());
    }
    if received == 0 {
        return Err(format!("AP client received no ICMP replies from {target}").into());
    }
    let percentile_index = (samples_micros.len() * 95).div_ceil(100) - 1;
    let p95_micros = samples_micros[percentile_index];
    if let Some(maximum_ms) = criteria.maximum_p95_ms
        && p95_micros > u64::from(maximum_ms) * 1_000
    {
        return Err(format!("AP ICMP p95={} us exceeds {} ms", p95_micros, maximum_ms).into());
    }
    if !output.status.success() && lost == 0 {
        return Err(format!("ping failed despite complete replies: {}", stdout.trim()).into());
    }
    Ok(TrafficReport::Icmp {
        transmitted: count,
        received,
        lost,
        p95_micros,
    })
}

fn ping_sample_micros(line: &str) -> Option<std::result::Result<u64, std::num::ParseFloatError>> {
    let suffix = line
        .split_once("time=")
        .or_else(|| line.split_once("time<"))?
        .1;
    let value = suffix.split_whitespace().next()?;
    Some(
        value
            .parse::<f64>()
            .map(|millis| (millis * 1_000.0).round() as u64),
    )
}

fn session_report(direction: Direction, evidence: &SessionEvidence) -> SessionReport {
    SessionReport {
        direction,
        rx_bytes: evidence.transport.rx_bytes,
        tx_bytes: evidence.transport.tx_bytes,
        rx_units: evidence.transport.rx_units,
        tx_units: evidence.transport.tx_units,
        elapsed_micros: evidence.transport.elapsed_micros,
    }
}

fn validate_rate_criteria(report: &SessionReport, criteria: &Criteria) -> Result<()> {
    if report.elapsed_micros == 0 {
        return Err("AP transport reported zero elapsed time".into());
    }
    let bitrate = |bytes: u64| {
        u128::from(bytes)
            .saturating_mul(8_000_000)
            .checked_div(u128::from(report.elapsed_micros))
            .unwrap_or(0)
    };
    if let Some(minimum) = criteria.minimum_rx_bps
        && bitrate(report.rx_bytes) < u128::from(minimum)
    {
        return Err(format!(
            "AP RX bitrate {} is below required {minimum}",
            bitrate(report.rx_bytes)
        )
        .into());
    }
    if let Some(minimum) = criteria.minimum_tx_bps
        && bitrate(report.tx_bytes) < u128::from(minimum)
    {
        return Err(format!(
            "AP TX bitrate {} is below required {minimum}",
            bitrate(report.tx_bytes)
        )
        .into());
    }
    Ok(())
}

fn cleanup_after_client_failure(
    capture: &SerialCapture,
    timeout: Duration,
    generation: u32,
    lab: &LabConfig,
    primary: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    let stop = stop_access_point(capture, timeout, generation, lab);
    let stop_stack = if stop.is_ok() {
        report_stack(capture, timeout, "ap-stopped")
    } else {
        Ok(())
    };
    let restart = if stop.is_ok() {
        restore_connected_station(capture, timeout, lab)
    } else {
        Ok(())
    };
    with_cleanup_errors(
        primary,
        None,
        stop.as_ref().err().map(|error| error.as_ref()),
        stop_stack.err(),
        restart.err(),
    )
}

fn restore_connected_station(
    capture: &SerialCapture,
    timeout: Duration,
    lab: &LabConfig,
) -> Result<()> {
    start_station(capture, lab, timeout)?;
    capture.wait_for_connected_station(timeout)?;
    report_stack(capture, timeout, "ap-station-reconnected")
}

fn with_cleanup_errors(
    primary: impl std::fmt::Display,
    client: Option<Box<dyn std::error::Error>>,
    stop: Option<&dyn std::error::Error>,
    stop_stack: Option<Box<dyn std::error::Error>>,
    restart: Option<Box<dyn std::error::Error>>,
) -> Box<dyn std::error::Error> {
    let mut message = primary.to_string();
    if let Some(error) = client {
        message.push_str(&format!("; client restore failed: {error}"));
    }
    if let Some(error) = stop {
        message.push_str(&format!("; AP stop failed: {error}"));
    }
    if let Some(error) = stop_stack {
        message.push_str(&format!("; stopped-AP stack query failed: {error}"));
    }
    if let Some(error) = restart {
        message.push_str(&format!("; station restore failed: {error}"));
    }
    message.into()
}

fn stop_access_point(
    capture: &SerialCapture,
    timeout: Duration,
    generation: u32,
    lab: &LabConfig,
) -> Result<open_esp_radio_hil_protocol::WifiAccessPointEvidence> {
    let evidence = capture.wait_access_point_stop(capture.request_access_point_stop()?, timeout)?;
    if evidence.generation != generation || evidence.channel != lab.access_point.channel() {
        return Err(format!("AP returned inconsistent stop evidence: {evidence:?}").into());
    }
    Ok(evidence)
}

const fn protocol_direction(direction: Direction) -> ProtocolDirection {
    match direction {
        Direction::Rx => ProtocolDirection::Rx,
        Direction::Tx => ProtocolDirection::Tx,
        Direction::Bidirectional => ProtocolDirection::Bidirectional,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_hil_protocol::{
        Finished, ResultSummary, StackUsage, StackWatermark, TransportEvidence,
    };

    fn evidence(rx_bytes: u64, tx_bytes: u64, rx_units: u64, tx_units: u64) -> SessionEvidence {
        SessionEvidence {
            finished: Finished {
                summary: ResultSummary {
                    passed: true,
                    evidence_records: 0,
                },
                evidence_crc32c: 0,
            },
            transport: TransportEvidence {
                rx_bytes,
                tx_bytes,
                rx_units,
                tx_units,
                elapsed_micros: 1,
                transport_errors: 0,
            },
            radio: None,
            tx_timing: None,
            rx_delivery: None,
            network_scheduler: None,
            stack: StackUsage {
                cpu0: StackWatermark {
                    capacity_bytes: 1,
                    free_bytes: 1,
                    used_bytes: 0,
                    minimum_free_bytes: 1,
                },
                cpu1: StackWatermark {
                    capacity_bytes: 1,
                    free_bytes: 1,
                    used_bytes: 0,
                    minimum_free_bytes: 1,
                },
            },
        }
    }

    #[test]
    fn udp_exact_delivery_checks_both_directions() {
        let sent = UdpTransmission {
            bytes: 1_200,
            datagrams: 1,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };
        let received = Burst {
            bytes: 1_200,
            datagrams: 1,
            started_at_zero: true,
            ..Burst::default()
        };
        assert!(validate_udp(Some(sent), Some(&[received]), evidence(1_200, 1_200, 1, 1)).is_ok());
    }

    #[test]
    fn udp_ordering_defect_fails_closed() {
        let received = Burst {
            bytes: 1_200,
            datagrams: 1,
            missing: 1,
            started_at_zero: true,
            ..Burst::default()
        };
        assert!(validate_udp(None, Some(&[received]), evidence(0, 1_200, 0, 1)).is_err());
    }

    #[test]
    fn tcp_exact_delivery_checks_both_directions() {
        let sent = TcpTransmission {
            bytes: 8_192,
            writes: 1,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_writes: 1,
            deadline_resets: 0,
        };
        let received = TcpReception {
            bytes: 8_192,
            reads: 1,
            elapsed: Duration::from_secs(1),
            pattern_errors: 0,
            eof: true,
        };
        assert!(validate_tcp(Some(sent), Some(received), evidence(8_192, 8_192, 1, 1)).is_ok());
    }

    #[test]
    fn parses_normal_and_sub_millisecond_ping_samples() {
        assert_eq!(
            ping_sample_micros("64 bytes from 1.2.3.4: time=1.25 ms")
                .unwrap()
                .unwrap(),
            1_250
        );
        assert_eq!(
            ping_sample_micros("64 bytes from 1.2.3.4: time<1 ms")
                .unwrap()
                .unwrap(),
            1_000
        );
    }
}
