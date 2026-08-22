//! Simultaneous same-channel STA+AP data-plane and beacon qualification.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::Path,
    sync::{Arc, Barrier},
    thread,
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, SessionConfig, SessionLinkRequirements,
    Transport, WifiNetworkInterface, WifiRole, WifiStationAccessPointRequest,
};
use serde::Serialize;

use crate::{
    Result,
    controlled_client::ControlledClient,
    lab_config::LabConfig,
    paced_udp::{Config as UdpConfig, HostTransmission, send as send_udp},
    traffic_capture::{SerialCapture, SessionEvidence, probe_udp_rx_ready_via},
    tx_traffic::{Burst, receive_bursts},
    udp_socket::{configure_qualification_receive_buffer, open_reverse_flow},
    wifi_control::{report_stack, require_transition, stop_station},
};

const TARGET_RX_PORT: u16 = 4_323;
const TARGET_TX_SOURCE_PORT: u16 = 4_324;
const STATION_HOST_PORT: u16 = 9_101;
const ACCESS_POINT_HOST_PORT: u16 = 9_102;

#[derive(Clone, Copy)]
pub(crate) struct Config {
    pub(crate) timeout: Duration,
    pub(crate) duration: Duration,
    pub(crate) rate_bps_per_flow: u64,
    pub(crate) minimum_bps_per_flow: u64,
    pub(crate) maximum_fairness_skew_percent: u8,
    pub(crate) payload_bytes: usize,
}

#[derive(Serialize)]
struct Report {
    schema: u8,
    offered_bps_per_flow: u64,
    minimum_bps_per_flow: u64,
    maximum_fairness_skew_percent: u8,
    station: InterfaceReport,
    access_point: InterfaceReport,
    access_point_epoch: open_esp_radio_hil_protocol::WifiAccessPointEvidence,
}

#[derive(Serialize)]
struct InterfaceReport {
    target_rx_bps: u64,
    host_rx_bps: u64,
    host_tx_datagrams: u64,
    host_rx_datagrams: u64,
    host_rx_missing: u64,
    host_rx_reordered: u64,
    host_rx_duplicates: u64,
    host_rx_first_reordered_after: Option<u32>,
    host_rx_first_reordered_sequence: Option<u32>,
    host_rx_maximum_reorder_distance: u32,
}

struct HostFlow {
    target: Ipv4Addr,
    peer: Ipv4Addr,
    port: u16,
    socket: UdpSocket,
}

pub(crate) fn run(config: Config, output: &Path, lab: &LabConfig) -> Result<()> {
    fs::create_dir_all(output)?;
    let capture = SerialCapture::start_with_reset(&lab.device.serial);
    let result = qualify(&capture, config, lab).and_then(|report| {
        fs::write(
            output.join("station-access-point-report.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        Ok(())
    });
    let capture_result = capture.finish_to(output);
    result?;
    capture_result.map(|_| ())
}

fn qualify(capture: &SerialCapture, config: Config, lab: &LabConfig) -> Result<Report> {
    let capabilities = capture.prepare_station(lab, config.timeout)?;
    if !capabilities.features.simultaneous_station_access_point {
        return Err("firmware does not advertise simultaneous STA+AP".into());
    }
    let _ = stop_station(capture, config.timeout)?;

    let access_point_request = lab.access_point.protocol_request()?;
    let request = WifiStationAccessPointRequest {
        station_credentials: lab.station.protocol_credentials()?,
        access_point: access_point_request.clone(),
    };
    let started = capture.wait_wifi_role_transition(
        capture.request_station_access_point_start(request)?,
        config.timeout,
    )?;
    require_transition(started, WifiRole::Idle, WifiRole::StationAccessPoint)?;

    let (station_target, access_point_target) = wait_for_endpoints(capture, config.timeout)?;
    let client = ControlledClient::connect(&lab.access_point)?;
    let readiness = probe_udp_rx_ready_via(
        capture,
        WifiNetworkInterface::Station,
        station_target,
        None,
        TARGET_RX_PORT,
        config.timeout,
    )
    .and_then(|_| {
        probe_udp_rx_ready_via(
            capture,
            WifiNetworkInterface::AccessPoint,
            access_point_target,
            None,
            TARGET_RX_PORT,
            config.timeout,
        )
    });
    if let Err(error) = readiness {
        // A readiness failure is itself useful paired data-plane evidence.
        // Always close the production owner graph so terminal AP counters
        // distinguish hardware admission, protocol rejection and network
        // publication failures instead of leaving the board in an active
        // role with no report.
        let stopped = capture.wait_station_access_point_stop(
            capture.request_station_access_point_stop()?,
            config.timeout,
        )?;
        let restore = client.restore();
        restore?;
        return Err(format!(
            "{error}; paired terminal AP evidence: {:?}",
            stopped.access_point
        )
        .into());
    }

    let station_flow = reverse_flow(Ipv4Addr::UNSPECIFIED, STATION_HOST_PORT, station_target)?;
    let access_point_flow = reverse_flow(
        lab.access_point.client_address(),
        ACCESS_POINT_HOST_PORT,
        access_point_target,
    )?;
    let receiver_duration = config.duration.saturating_add(Duration::from_secs(5));
    let station_receiver = spawn_receiver(&station_flow, receiver_duration)?;
    let access_point_receiver = spawn_receiver(&access_point_flow, receiver_duration)?;

    // Give both target sessions two seconds beyond the simultaneous host
    // offer. This absorbs sequential command/readiness setup without making
    // either endpoint's measured transport depend on UART latency.
    let target_duration = config.duration.saturating_add(Duration::from_secs(2));
    let station_session = start_session(capture, &station_flow, config, target_duration)?;
    let access_point_session = start_session(capture, &access_point_flow, config, target_duration)?;

    let barrier = Arc::new(Barrier::new(3));
    let station_sender = spawn_sender(&station_flow, config, Arc::clone(&barrier));
    let access_point_sender = spawn_sender(&access_point_flow, config, Arc::clone(&barrier));
    barrier.wait();
    let station_host_tx = join_sender(station_sender, "station")?;
    let access_point_host_tx = join_sender(access_point_sender, "access-point")?;

    let station_evidence = capture.wait_for_session(station_session, config.timeout)?;
    let access_point_evidence = capture.wait_for_session(access_point_session, config.timeout)?;
    capture.acknowledge_session(station_session)?;
    capture.acknowledge_session(access_point_session)?;

    let station_host_rx = join_receiver(station_receiver, "station")?;
    let access_point_host_rx = join_receiver(access_point_receiver, "access-point")?;
    let station = interface_report(station_evidence, station_host_tx, station_host_rx)?;
    let access_point = interface_report(
        access_point_evidence,
        access_point_host_tx,
        access_point_host_rx,
    )?;

    // A qualification verdict must never bypass owner recovery. Preserve the
    // data-plane failure, but stop the pair and collect terminal AP evidence
    // before returning it to the caller.
    let data_plane_verdict = (|| {
        validate_interface("station", &station, config.minimum_bps_per_flow)?;
        validate_interface("access-point", &access_point, config.minimum_bps_per_flow)?;
        validate_fairness(
            "target RX",
            station.target_rx_bps,
            access_point.target_rx_bps,
            config.maximum_fairness_skew_percent,
        )?;
        validate_fairness(
            "host RX",
            station.host_rx_bps,
            access_point.host_rx_bps,
            config.maximum_fairness_skew_percent,
        )?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();

    let stopped = capture.wait_station_access_point_stop(
        capture.request_station_access_point_stop()?,
        config.timeout,
    )?;
    let terminal_verdict = (|| {
        require_transition(
            stopped.transition,
            WifiRole::StationAccessPoint,
            WifiRole::Idle,
        )?;
        if stopped.transition.generation != started.generation {
            return Err("paired start/stop generations differ".into());
        }
        validate_access_point_epoch(stopped.access_point)
    })();
    let restore = client.restore();
    let stack = report_stack(capture, config.timeout, "sta-ap-load-stopped");
    restore?;
    terminal_verdict?;
    stack?;
    data_plane_verdict?;

    Ok(Report {
        schema: 1,
        offered_bps_per_flow: config.rate_bps_per_flow,
        minimum_bps_per_flow: config.minimum_bps_per_flow,
        maximum_fairness_skew_percent: config.maximum_fairness_skew_percent,
        station,
        access_point,
        access_point_epoch: stopped.access_point,
    })
}

pub(crate) fn wait_for_endpoints(
    capture: &SerialCapture,
    timeout: Duration,
) -> Result<(Ipv4Addr, Ipv4Addr)> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let (Some(station), Some(access_point)) = (
            capture.observed_protocol_ipv4(WifiNetworkInterface::Station),
            capture.observed_protocol_ipv4(WifiNetworkInterface::AccessPoint),
        ) {
            return Ok((station, access_point));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err("paired role did not publish both network endpoints".into())
}

fn reverse_flow(bind: Ipv4Addr, port: u16, target: Ipv4Addr) -> Result<HostFlow> {
    let socket = UdpSocket::bind(SocketAddrV4::new(bind, port))?;
    configure_qualification_receive_buffer(&socket)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    socket.connect(SocketAddrV4::new(target, TARGET_TX_SOURCE_PORT))?;
    open_reverse_flow(&socket)?;
    let peer = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("UDP qualification selected an IPv6 source".into()),
    };
    Ok(HostFlow {
        target,
        peer,
        port,
        socket,
    })
}

fn start_session(
    capture: &SerialCapture,
    flow: &HostFlow,
    config: Config,
    duration: Duration,
) -> Result<crate::traffic_capture::SessionHandle> {
    let network_interface = if flow.port == STATION_HOST_PORT {
        WifiNetworkInterface::Station
    } else {
        WifiNetworkInterface::AccessPoint
    };
    capture.start_session(SessionConfig {
        network_interface,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(u32::try_from(duration.as_millis())?),
        peer: Some(Ipv4Endpoint {
            address: flow.peer.octets(),
            port: flow.port,
        }),
        target_rx: Some(FlowConfig {
            payload_bytes: u16::try_from(config.payload_bytes)?,
            offered_rate_bps: Some(config.rate_bps_per_flow),
        }),
        target_tx: Some(FlowConfig {
            payload_bytes: u16::try_from(config.payload_bytes)?,
            offered_rate_bps: Some(config.rate_bps_per_flow),
        }),
        link_requirements: SessionLinkRequirements::NONE,
    })
}

fn spawn_receiver(
    flow: &HostFlow,
    duration: Duration,
) -> Result<thread::JoinHandle<std::io::Result<Vec<Burst>>>> {
    let socket = flow.socket.try_clone()?;
    let target = flow.target;
    Ok(thread::spawn(move || {
        receive_bursts(&socket, target, duration)
    }))
}

fn spawn_sender(
    flow: &HostFlow,
    config: Config,
    barrier: Arc<Barrier>,
) -> thread::JoinHandle<std::result::Result<HostTransmission, String>> {
    let send = UdpConfig {
        address: flow.target,
        port: TARGET_RX_PORT,
        rate_bps: config.rate_bps_per_flow,
        duration: config.duration,
        payload: config.payload_bytes,
    };
    thread::spawn(move || {
        barrier.wait();
        send_udp(send).map_err(|error| error.to_string())
    })
}

fn join_sender(
    sender: thread::JoinHandle<std::result::Result<HostTransmission, String>>,
    name: &str,
) -> Result<HostTransmission> {
    sender
        .join()
        .map_err(|_| format!("{name} UDP sender panicked"))?
        .map_err(Into::into)
}

fn join_receiver(
    receiver: thread::JoinHandle<std::io::Result<Vec<Burst>>>,
    name: &str,
) -> Result<Vec<Burst>> {
    receiver
        .join()
        .map_err(|_| format!("{name} UDP receiver panicked"))?
        .map_err(Into::into)
}

fn interface_report(
    evidence: SessionEvidence,
    host_tx: HostTransmission,
    host_rx: Vec<Burst>,
) -> Result<InterfaceReport> {
    let burst = host_rx
        .into_iter()
        .filter(|burst| burst.started_at_zero)
        .max_by_key(|burst| burst.datagrams)
        .ok_or("target TX did not produce a zero-started UDP burst")?;
    let target_rx_bps = throughput_bps(
        evidence.transport.rx_bytes,
        evidence.transport.elapsed_micros,
    );
    Ok(InterfaceReport {
        target_rx_bps,
        host_rx_bps: burst.throughput_kbps().saturating_mul(1_000),
        host_tx_datagrams: host_tx.datagrams,
        host_rx_datagrams: burst.datagrams,
        host_rx_missing: burst.missing,
        host_rx_reordered: burst.reordered,
        host_rx_duplicates: burst.duplicates,
        host_rx_first_reordered_after: burst.first_reordered_after,
        host_rx_first_reordered_sequence: burst.first_reordered_sequence,
        host_rx_maximum_reorder_distance: burst.maximum_reorder_distance,
    })
}

fn throughput_bps(bytes: u64, elapsed_micros: u64) -> u64 {
    bytes
        .saturating_mul(8)
        .saturating_mul(1_000_000)
        .checked_div(elapsed_micros.max(1))
        .unwrap_or(0)
}

fn validate_interface(name: &str, report: &InterfaceReport, minimum: u64) -> Result<()> {
    if report.target_rx_bps < minimum || report.host_rx_bps < minimum {
        return Err(format!(
            "{name} fairness cell below {minimum} bps: target_rx={} host_rx={}",
            report.target_rx_bps, report.host_rx_bps,
        )
        .into());
    }
    if report.host_rx_reordered != 0 || report.host_rx_duplicates != 0 {
        return Err(format!(
            "{name} target TX ordering defect: reordered={} duplicates={} first={} after={} maximum_distance={}",
            report.host_rx_reordered,
            report.host_rx_duplicates,
            report
                .host_rx_first_reordered_sequence
                .map_or_else(|| "none".into(), |value| value.to_string()),
            report
                .host_rx_first_reordered_after
                .map_or_else(|| "none".into(), |value| value.to_string()),
            report.host_rx_maximum_reorder_distance,
        )
        .into());
    }
    Ok(())
}

fn validate_fairness(name: &str, first: u64, second: u64, maximum_skew: u8) -> Result<()> {
    let minimum = first.min(second);
    let maximum = first.max(second);
    if u128::from(maximum) * 100
        > u128::from(minimum) * u128::from(100_u8.saturating_add(maximum_skew))
    {
        return Err(format!(
            "{name} fairness skew exceeds {maximum_skew}%: station={first} access-point={second}",
        )
        .into());
    }
    Ok(())
}

fn validate_access_point_epoch(
    evidence: open_esp_radio_hil_protocol::WifiAccessPointEvidence,
) -> Result<()> {
    if evidence.beacons_transmitted == 0
        || evidence.missed_beacon_intervals != 0
        || evidence.maximum_beacon_lateness_micros >= 102_400
        || evidence.rx_hardware_buffer_full != 0
        || evidence.rx_hardware_fifo_overflow != 0
    {
        return Err(format!(
            "paired AP timing/RX health failed: beacons={} missed={} maximum_lateness_us={} buffer_full={} fifo_overflow={}",
            evidence.beacons_transmitted,
            evidence.missed_beacon_intervals,
            evidence.maximum_beacon_lateness_micros,
            evidence.rx_hardware_buffer_full,
            evidence.rx_hardware_fifo_overflow,
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fairness_is_symmetric_and_bounded() {
        assert!(validate_fairness("rx", 10_000, 12_000, 20).is_ok());
        assert!(validate_fairness("rx", 12_001, 10_000, 20).is_err());
        assert!(validate_fairness("rx", 10_000, 12_001, 20).is_err());
    }

    #[test]
    fn access_point_epoch_rejects_one_beacon_period_lateness() {
        let evidence = open_esp_radio_hil_protocol::WifiAccessPointEvidence {
            beacons_transmitted: 1,
            maximum_beacon_lateness_micros: 102_400,
            ..Default::default()
        };
        assert!(validate_access_point_epoch(evidence).is_err());
    }
}
