//! WPA2 AP lifecycle, exact data-plane and concurrent-client qualification.

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
    controlled_openwrt_client::{
        ControlledOpenWrtClient, OpenWrtUdpTransmission, SecondaryClientProbeEvidence,
    },
    lab_config::{LabConfig, StationFixtureConfig},
    paced_tcp::{
        Config as TcpConfig, HostReception as TcpReception, HostTransmission as TcpTransmission,
        exchange as exchange_tcp, receive as receive_tcp, send as send_tcp,
    },
    paced_udp::{Config as UdpConfig, HostTransmission as UdpTransmission, send as send_udp},
    rx_delivery,
    scenario::{
        AccessPointClient, AccessPointTraffic, Criteria, Direction, LinkExpectation, PhyExpectation,
    },
    traffic_capture::{SerialCapture, SessionEvidence, probe_udp_rx_ready},
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
    pub(crate) client: AccessPointClient,
    pub(crate) traffic: AccessPointTraffic,
    pub(crate) criteria: Criteria,
    pub(crate) expected_link: Option<LinkExpectation>,
    pub(crate) require_rx_delivery_evidence: bool,
}

enum ConnectedClients {
    Laptop {
        primary: ControlledClient,
        secondary: Option<ControlledOpenWrtClient>,
    },
    OpenWrt {
        primary: ControlledOpenWrtClient,
    },
}

impl ConnectedClients {
    fn openwrt_primary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::OpenWrt { primary } => Some(primary),
            Self::Laptop { .. } => None,
        }
    }

    fn secondary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::Laptop { secondary, .. } => secondary.as_ref(),
            Self::OpenWrt { .. } => None,
        }
    }
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
    secondary_client: Option<SecondaryClientProbeEvidence>,
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
    if let Some(expected_phy) = config.expected_link.map(|link| link.phy) {
        let expected_bandwidth_mhz = match expected_phy {
            PhyExpectation::Ht40 => 40,
            PhyExpectation::Ht20 => 20,
            PhyExpectation::He20 => {
                return Err("AP qualification does not claim an HE20 PHY".into());
            }
        };
        if lab.access_point.bandwidth_mhz() != expected_bandwidth_mhz {
            return Err(format!(
                "AP scenario requires {} ({} MHz), but the lab AP request is {} MHz",
                expected_phy.id(),
                expected_bandwidth_mhz,
                lab.access_point.bandwidth_mhz(),
            )
            .into());
        }
    }
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
    report_stack(capture, config.timeout, "ap-initial-station-connected")?;
    let minimum_clients = config.criteria.minimum_concurrent_ap_clients.unwrap_or(1);
    if lab.access_point.client_limit() < minimum_clients {
        return Err(format!(
            "AP client_limit={} is below scenario minimum_concurrent_ap_clients={minimum_clients}",
            lab.access_point.client_limit(),
        )
        .into());
    }

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

        let clients = match connect_clients(config.client, minimum_clients, lab) {
            Ok(clients) => clients,
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
        let secondary_probe = clients.secondary().map(|client| {
            client.spawn_probe(
                lab.access_point.target_address(),
                traffic_duration(&config.traffic),
            )
        });
        let data_result = qualify_data_plane(capture, config, lab, &clients);
        let secondary_probe_result = secondary_probe.map(|probe| {
            probe
                .join()
                .map_err(|_| "secondary AP client probe thread panicked".to_owned())?
        });
        // Keep every admitted peer alive until the target has completed its
        // AP stop transaction. Otherwise the clients deauthenticate first and
        // the qualification never exercises the driver's ordered
        // disassociation/deauthentication teardown.
        let stop_result = stop_access_point(capture, config.timeout, started.generation, lab);
        let client_restore = restore_clients(clients);
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
            let mut data_error = match stop_result.as_ref() {
                Ok(stopped) => format!(
                    "{error}; AP TX evidence: data={} attempts={} retried_frames={} \
                     maximum_attempts={} minimum_final_rate_kbps={} ack_snr={}/{}/{} \
                     tx_ht40_mcs7={}/{} \
                     ack_timeout_retries={} \
                     cts_timeout_retries={} collision_retries={} hardware_failures={} \
                     hardware_timeouts={} collision_limits={} last_hardware_status={} beacons={}; AP RX evidence: \
                     units={} descriptors={} recycled_descriptors={} retained_descriptors={} discarded_units={} \
                     rx_ht40_mcs={:?} total_ht={} ht_ampdu={} rssi={}/{}/{}/{} protected={} mic_failures={} quarantined={} duplicates={} \
                     radio_rejected={} protocol_rejected={} ethernet_staged={} tcp_staged={}",
                    stopped.data_frames_transmitted,
                    stopped.data_tx_attempts,
                    stopped.data_tx_retried_frames,
                    stopped.data_tx_maximum_attempts,
                    stopped.data_tx_minimum_final_rate_kbps,
                    stopped.data_tx_ack_snr_samples,
                    stopped.data_tx_minimum_ack_snr_db,
                    stopped.data_tx_maximum_ack_snr_db,
                    stopped.tx_ht40_mcs7_aggregates,
                    stopped.tx_ht_aggregates,
                    stopped.tx_ack_timeout_retries,
                    stopped.tx_cts_timeout_retries,
                    stopped.tx_collision_retries,
                    stopped.tx_hardware_failures,
                    stopped.tx_hardware_timeouts,
                    stopped.tx_collision_limits,
                    stopped.tx_last_hardware_status,
                    stopped.beacons_transmitted,
                    stopped.completed_rx_units,
                    stopped.completed_rx_descriptors,
                    stopped.recycled_rx_descriptors,
                    stopped.retained_rx_descriptors,
                    stopped.discarded_rx_units,
                    stopped.rx_ht40_mcs_frames,
                    stopped.rx_ht_data_frames,
                    stopped.rx_ht_ampdu_data_frames,
                    stopped.rx_rssi_samples,
                    stopped.rx_rssi_sum_dbm,
                    stopped.rx_rssi_min_dbm,
                    stopped.rx_rssi_max_dbm,
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
            if let Some(Err(probe_error)) = secondary_probe_result.as_ref() {
                data_error.push_str("; secondary AP client: ");
                data_error.push_str(probe_error);
            }
            return Err(with_cleanup_errors(
                data_error,
                client_restore.err(),
                stop_result.as_ref().err().map(|error| error.as_ref()),
                stop_stack_result.err(),
                restart_result.err(),
            ));
        }
        if let Some(Err(error)) = secondary_probe_result.as_ref() {
            return Err(with_cleanup_errors(
                error,
                client_restore.err(),
                stop_result.as_ref().err().map(|error| error.as_ref()),
                stop_stack_result.err(),
                restart_result.err(),
            ));
        }
        let traffic = data_result?;
        let secondary_client = secondary_probe_result.transpose()?;
        client_restore?;
        let stopped = stop_result?;
        stop_stack_result?;
        restart_result?;
        validate_mcs_evidence(&config.traffic, config.expected_link, &stopped)?;
        let unacknowledged_disconnects = stopped
            .disassociations_published
            .saturating_sub(stopped.disassociations_acknowledged)
            .saturating_add(
                stopped
                    .deauthentications_published
                    .saturating_sub(stopped.deauthentications_acknowledged),
            );
        if stopped.beacons_transmitted == 0
            || stopped.missed_beacon_intervals != 0
            || stopped.maximum_beacon_lateness_micros >= 102_400
            || stopped.authentication_responses == 0
            || stopped.association_responses == 0
            || stopped.wpa2_response_windows < 2
            || stopped.wpa2_pending_on_stop != 0
            || stopped.wpa2_handshake_failures != 0
            || stopped.wpa2_handshake_timeouts != 0
            || stopped.authorized_peers == 0
            || stopped.maximum_associated_peers == 0
            || stopped.maximum_authorized_peers == 0
            || stopped.maximum_associated_peers < minimum_clients
            || stopped.maximum_authorized_peers < minimum_clients
            || stopped.peer_removals < u32::from(minimum_clients)
            || stopped.disassociations_prepared < u32::from(minimum_clients)
            || stopped.disassociations_published < u32::from(minimum_clients)
            || stopped.disassociations_prepared != stopped.disassociations_published
            || stopped.deauthentications_prepared < u32::from(minimum_clients)
            || stopped.deauthentications_published < u32::from(minimum_clients)
            || stopped.deauthentications_prepared != stopped.deauthentications_published
            || stopped.completed_rx_units == 0
            || stopped.completed_rx_descriptors
                != stopped
                    .recycled_rx_descriptors
                    .saturating_add(stopped.retained_rx_descriptors)
            // Complete vendor `wifi_softap_stop` submits disassociation and
            // deauthentication back-to-back and does not make peer removal
            // conditional on their ACKs. Our owner waits for each terminal
            // DMA outcome, but an unacknowledged disconnect is expected once
            // the client has already left. No other terminal TX failure is
            // accepted by this gate.
            || u32::from(stopped.tx_hardware_failures) != unacknowledged_disconnects
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
            secondary_client,
            access_point: stopped,
        });
    }
    Ok(cycles)
}

fn validate_mcs_evidence(
    traffic: &AccessPointTraffic,
    expected_link: Option<LinkExpectation>,
    evidence: &open_esp_radio_hil_protocol::WifiAccessPointEvidence,
) -> Result<()> {
    let Some(LinkExpectation {
        phy,
        minimum_mcs: Some(minimum_mcs),
    }) = expected_link
    else {
        return Ok(());
    };
    if phy != PhyExpectation::Ht40 || minimum_mcs != 7 {
        return Err(format!(
            "AP target MCS evidence currently supports only HT40 MCS7, requested {} MCS{minimum_mcs}",
            phy.id(),
        )
        .into());
    }
    let (require_rx, require_tx) = match traffic {
        AccessPointTraffic::Udp { direction, .. } | AccessPointTraffic::Tcp { direction, .. } => {
            match direction {
                Direction::Rx => (true, false),
                Direction::Tx => (false, true),
                Direction::Bidirectional => (true, true),
            }
        }
        AccessPointTraffic::Icmp { .. } => (true, true),
        AccessPointTraffic::None => {
            return Err("AP minimum_mcs requires a data-plane workload".into());
        }
    };
    if require_rx && evidence.rx_ht40_mcs_frames[7] == 0 {
        return Err(format!(
            "AP client-to-target path did not observe HT40 MCS7 protected data \
             (HT40 MCS histogram={:?}, total HT frames={}, HT A-MPDU frames={})",
            evidence.rx_ht40_mcs_frames,
            evidence.rx_ht_data_frames,
            evidence.rx_ht_ampdu_data_frames,
        )
        .into());
    }
    if require_tx && evidence.tx_ht40_mcs7_aggregates == 0 {
        return Err(format!(
            "AP target-to-client path did not publish an HT40 MCS7 aggregate (HT aggregates={})",
            evidence.tx_ht_aggregates,
        )
        .into());
    }
    Ok(())
}

fn traffic_duration(traffic: &AccessPointTraffic) -> Duration {
    match traffic {
        AccessPointTraffic::None => Duration::from_secs(2),
        AccessPointTraffic::Icmp {
            count, interval_ms, ..
        } => Duration::from_millis(u64::from(*count) * u64::from(*interval_ms))
            .max(Duration::from_secs(2)),
        AccessPointTraffic::Udp {
            duration_seconds, ..
        }
        | AccessPointTraffic::Tcp {
            duration_seconds, ..
        } => Duration::from_secs(u64::from(*duration_seconds)),
    }
}

fn connect_clients(
    client: AccessPointClient,
    minimum_clients: u8,
    lab: &LabConfig,
) -> Result<ConnectedClients> {
    let openwrt_fixture = || -> Result<&crate::lab_config::OpenWrtConfig> {
        match &lab.station_fixture {
            StationFixtureConfig::OpenWrt(fixture) => Ok(fixture),
            _ => Err("AP OpenWrt client requires the OpenWrt station fixture".into()),
        }
    };
    match client {
        AccessPointClient::OpenWrt => Ok(ConnectedClients::OpenWrt {
            primary: ControlledOpenWrtClient::connect_primary(
                &lab.access_point,
                openwrt_fixture()?,
            )?,
        }),
        AccessPointClient::Laptop => {
            // Associate the observable OpenWrt peer first in two-client runs.
            // This gives debugfs evidence for the first BA bank and exercises
            // the laptop on the next independently allocated peer slot.
            let secondary = if minimum_clients >= 2 {
                Some(ControlledOpenWrtClient::connect(
                    &lab.access_point,
                    openwrt_fixture()?,
                )?)
            } else {
                None
            };
            let primary = match ControlledClient::connect(&lab.access_point) {
                Ok(primary) => primary,
                Err(error) => {
                    let restore = secondary
                        .map(ControlledOpenWrtClient::restore)
                        .transpose()
                        .err();
                    return Err(with_cleanup_errors(error, restore, None, None, None));
                }
            };
            Ok(ConnectedClients::Laptop { primary, secondary })
        }
    }
}

fn restore_clients(clients: ConnectedClients) -> Result<()> {
    match clients {
        ConnectedClients::OpenWrt { primary } => primary.restore(),
        ConnectedClients::Laptop { primary, secondary } => {
            let secondary = secondary.map(ControlledOpenWrtClient::restore).transpose();
            let primary = primary.restore();
            match (primary, secondary) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(primary), Ok(_)) => Err(primary),
                (Ok(()), Err(secondary)) => Err(secondary),
                (Err(primary), Err(secondary)) => Err(format!(
                    "primary client restore failed: {primary}; secondary client restore failed: {secondary}",
                )
                .into()),
            }
        }
    }
}

fn qualify_data_plane(
    capture: &SerialCapture,
    config: &Config,
    lab: &LabConfig,
    clients: &ConnectedClients,
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
            clients,
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
    clients: &ConnectedClients,
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
    if rx_rate_bps.is_some() {
        let openwrt_probe = clients
            .openwrt_primary()
            .map(|client| client.spawn_udp_rx_probe(target, UDP_RX_PORT));
        let readiness = probe_udp_rx_ready(capture, target, UDP_RX_PORT, config.timeout);
        if let Some(probe) = openwrt_probe {
            let result = probe
                .join()
                .map_err(|_| "OpenWrt UDP readiness probe thread panicked")?;
            result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
        }
        readiness?;
    }
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
        // The current AP capability contract is legacy unicast TX. Block Ack
        // must be requested only after AP HT negotiation is implemented.
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
    let data_plane = if let Some(openwrt) = clients.openwrt_primary() {
        match (rx_rate_bps, socket) {
            (Some(rate), None) => openwrt
                .send_udp(target, UDP_RX_PORT, rate, duration, payload_bytes)
                .map(|sent| {
                    (
                        Some(openwrt_udp_transmission(sent, duration)),
                        None,
                        Some(sent),
                    )
                }),
            _ => Err("OpenWrt AP client accepts only an RX-only UDP workload".into()),
        }
    } else {
        match (send_config, socket) {
            (Some(send_config), Some(socket)) => {
                let sender =
                    thread::spawn(move || send_udp(send_config).map_err(|e| e.to_string()));
                let received = receive_bursts(&socket, target, receive_duration);
                let sent = sender
                    .join()
                    .map_err(|_| "AP UDP sender thread panicked")??;
                Ok((Some(sent), Some(received?), None))
            }
            (Some(send_config), None) => send_udp(send_config).map(|sent| (Some(sent), None, None)),
            (None, Some(socket)) => receive_bursts(&socket, target, receive_duration)
                .map(|received| (None, Some(received), None))
                .map_err(Into::into),
            (None, None) => Err("AP UDP workload has no data direction".into()),
        }
    };

    let structured = capture.wait_for_session(session, config.timeout);
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let (host_tx, host_rx, openwrt_tx) =
        data_plane.map_err(|error| format!("AP UDP host path failed: {error}"))?;
    let structured = structured.map_err(|error| format!("AP UDP target failed: {error}"))?;
    acknowledgement?;
    let mut report = session_report(direction, &structured);
    let host_received = validate_udp(
        host_tx,
        host_rx.as_deref(),
        structured,
        config.criteria.exact_delivery,
        config.require_rx_delivery_evidence,
    )?;
    if let Some(host_received) = host_received {
        // Target TX evidence counts frames admitted to the radio path. Under
        // an intentionally saturated characterization workload, only the
        // receiver can report delivered throughput.
        report.tx_bytes = host_received.bytes;
        report.tx_units = host_received.datagrams;
    }
    validate_rate_criteria(&report, &config.criteria).map_err(|error| {
        let source = host_tx.map(|host| {
            format!(
                "host UDP source={}bps datagrams={} maximum_lateness_us={} maximum_catch_up_datagrams={} deadline_resets={}",
                host.throughput_bps(),
                host.datagrams,
                host.maximum_lateness_us(),
                host.maximum_catch_up_datagrams,
                host.deadline_resets,
            )
        });
        let openwrt = openwrt_tx.map(|source| {
            format!(
                "OpenWrt station tx_packets={} tx_retries={} tx_failed={} radio_rx_fcs_errors={}",
                source.station_tx_packets,
                source.station_tx_retries,
                source.station_tx_failed,
                source.radio_rx_fcs_errors,
            )
        });
        match (source, openwrt) {
            (Some(source), Some(openwrt)) => format!("{error}; {source}; {openwrt}"),
            (Some(source), None) => format!("{error}; {source}"),
            (None, Some(openwrt)) => format!("{error}; {openwrt}"),
            (None, None) => error.to_string(),
        }
    })?;
    Ok(TrafficReport::Udp(report))
}

fn validate_udp(
    host_tx: Option<UdpTransmission>,
    host_rx: Option<&[Burst]>,
    evidence: SessionEvidence,
    require_exact_delivery: bool,
    require_rx_delivery_evidence: bool,
) -> Result<Option<Burst>> {
    if !evidence.finished.summary.passed || evidence.transport.transport_errors != 0 {
        return Err(format!(
            "AP UDP target failed: passed={} errors={}",
            evidence.finished.summary.passed, evidence.transport.transport_errors,
        )
        .into());
    }
    match host_tx {
        Some(host) => {
            if (require_exact_delivery
                && (host.bytes != evidence.transport.rx_bytes
                    || host.datagrams != evidence.transport.rx_units))
                || (!require_exact_delivery
                    && (evidence.transport.rx_bytes > host.bytes
                        || evidence.transport.rx_units > host.datagrams))
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
            let rx = evidence
                .radio
                .and_then(|radio| radio.rx)
                .ok_or("AP UDP RX session did not publish typed RX radio evidence")?;
            let expected_highest = require_exact_delivery
                .then(|| {
                    u32::try_from(host.datagrams)
                        .ok()
                        .and_then(|datagrams| datagrams.checked_sub(1))
                })
                .flatten();
            if (require_exact_delivery
                && (rx.sequence_first != Some(0)
                    || rx.sequence_highest != expected_highest
                    || rx.sequence_gap_events != 0
                    || rx.sequence_forward_missing != 0))
                || rx.sequence_backward != 0
                || rx.sequence_duplicates != 0
                || rx.sequence_unsequenced != 0
            {
                return Err(format!("AP UDP RX ordering defect: {rx:?}").into());
            }
            if require_rx_delivery_evidence {
                let delivery = evidence
                    .rx_delivery
                    .ok_or("AP diagnostic did not publish typed RX delivery evidence")?;
                let assessment = rx_delivery::assess(host.datagrams, delivery);
                if !assessment.exact() {
                    return Err(format!(
                        "typed AP RX delivery frontier is {}",
                        assessment.frontier()
                    )
                    .into());
                }
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
            if (require_exact_delivery && host.missing != 0)
                || host.reordered != 0
                || host.duplicates != 0
            {
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
            if (require_exact_delivery
                && (host.bytes != evidence.transport.tx_bytes
                    || host.datagrams != evidence.transport.tx_units))
                || (!require_exact_delivery
                    && (host.bytes > evidence.transport.tx_bytes
                        || host.datagrams > evidence.transport.tx_units))
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
            return Ok(Some(host));
        }
        None if evidence.transport.tx_bytes != 0 || evidence.transport.tx_units != 0 => {
            return Err("AP UDP RX-only session reported transmitted traffic".into());
        }
        None => {}
    }
    Ok(None)
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

fn openwrt_udp_transmission(
    source: OpenWrtUdpTransmission,
    requested_duration: Duration,
) -> UdpTransmission {
    UdpTransmission {
        bytes: source.bytes,
        datagrams: source.datagrams,
        // The remote process setup belongs outside the offered traffic
        // interval reported by iperf. Use the scenario-owned duration rather
        // than the encompassing SSH wall clock.
        elapsed: requested_duration,
        maximum_lateness: Duration::ZERO,
        maximum_catch_up_datagrams: 1,
        deadline_resets: 0,
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
    if let Some(minimum) = criteria.minimum_combined_bps {
        let combined = bitrate(report.rx_bytes).saturating_add(bitrate(report.tx_bytes));
        if combined < u128::from(minimum) {
            return Err(
                format!("AP combined bitrate {combined} is below required {minimum}").into(),
            );
        }
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
    let lifecycle_cursor = capture.station_lifecycle_cursor();
    start_station(capture, lab, timeout)?;
    capture.wait_for_connected_station_after(lifecycle_cursor, timeout)?;
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
    if evidence.generation != generation
        || evidence.channel != lab.access_point.channel()
        || evidence.bandwidth_mhz != lab.access_point.bandwidth_mhz()
    {
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
        Finished, RadioEvidence, ResultSummary, RxConsumerLedgerEvidence, RxDeliveryEvidence,
        RxRadioEvidence, RxSequenceStageEvidence, StackUsage, StackWatermark, TransportEvidence,
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
            radio: (rx_units != 0).then_some(RadioEvidence {
                rx: Some(RxRadioEvidence {
                    sequence_first: Some(0),
                    sequence_highest: u32::try_from(rx_units)
                        .ok()
                        .and_then(|units| units.checked_sub(1)),
                    ..RxRadioEvidence::default()
                }),
                tx: None,
            }),
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
        assert!(
            validate_udp(
                Some(sent),
                Some(&[received]),
                evidence(1_200, 1_200, 1, 1),
                true,
                false,
            )
            .is_ok()
        );
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
        assert!(
            validate_udp(
                None,
                Some(&[received]),
                evidence(0, 1_200, 0, 1),
                true,
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn udp_target_rx_reordering_fails_closed_even_when_counts_match() {
        let sent = UdpTransmission {
            bytes: 2_400,
            datagrams: 2,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };
        let mut reordered = evidence(2_400, 0, 2, 0);
        reordered
            .radio
            .as_mut()
            .and_then(|radio| radio.rx.as_mut())
            .expect("RX evidence")
            .sequence_backward = 1;

        assert!(validate_udp(Some(sent), None, reordered, true, false).is_err());
    }

    #[test]
    fn udp_characterization_reports_receiver_delivery_and_allows_loss() {
        let sent = UdpTransmission {
            bytes: 2_400,
            datagrams: 2,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };
        let received = Burst {
            bytes: 1_200,
            datagrams: 1,
            missing: 1,
            started_at_zero: true,
            ..Burst::default()
        };
        let delivered = validate_udp(
            Some(sent),
            Some(&[received]),
            evidence(1_200, 2_400, 1, 2),
            false,
            false,
        )
        .unwrap();

        assert_eq!(delivered, Some(received));
    }

    #[test]
    fn rx_delivery_diagnostic_fails_closed_without_publication_evidence() {
        let sent = UdpTransmission {
            bytes: 1_200,
            datagrams: 1,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };

        let error = validate_udp(Some(sent), None, evidence(1_200, 0, 1, 0), true, true)
            .expect_err("diagnostic evidence must be mandatory");

        assert!(error.to_string().contains("typed RX delivery evidence"));
    }

    #[test]
    fn rx_delivery_diagnostic_rejects_evidence_from_the_wrong_publication_edge() {
        let sent = UdpTransmission {
            bytes: 1_200,
            datagrams: 1,
            elapsed: Duration::from_secs(1),
            maximum_lateness: Duration::ZERO,
            maximum_catch_up_datagrams: 1,
            deadline_resets: 0,
        };
        let mut observed = evidence(1_200, 0, 1, 0);
        observed.rx_delivery = Some(RxDeliveryEvidence {
            udp_consumer: RxSequenceStageEvidence {
                data_units: 1,
                first: Some(0),
                highest: Some(0),
                ..Default::default()
            },
            consumer_ledger: RxConsumerLedgerEvidence {
                unexpected_consumer: 1,
                first_observed: Some(0),
                ..Default::default()
            },
            ..Default::default()
        });

        let error = validate_udp(Some(sent), None, observed, true, true)
            .expect_err("misplaced diagnostic evidence must fail closed");

        assert!(error.to_string().contains("typed AP RX delivery frontier"));
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

    #[test]
    fn ap_rate_gate_checks_combined_bidirectional_throughput() {
        let report = SessionReport {
            direction: Direction::Bidirectional,
            rx_bytes: 2_000_000,
            tx_bytes: 2_000_000,
            rx_units: 0,
            tx_units: 0,
            elapsed_micros: 1_000_000,
        };
        let mut criteria = Criteria {
            minimum_combined_bps: Some(32_000_000),
            ..Criteria::default()
        };
        assert!(validate_rate_criteria(&report, &criteria).is_ok());
        criteria.minimum_combined_bps = Some(32_000_001);
        assert!(validate_rate_criteria(&report, &criteria).is_err());
    }

    #[test]
    fn ap_ht40_mcs7_gate_is_directional_and_fails_closed() {
        let link = Some(LinkExpectation {
            phy: PhyExpectation::Ht40,
            minimum_mcs: Some(7),
        });
        let rx = AccessPointTraffic::Udp {
            direction: Direction::Rx,
            duration_seconds: 1,
            rx_rate_bps: Some(1),
            tx_rate_bps: None,
            payload_bytes: 1,
        };
        let tx = AccessPointTraffic::Udp {
            direction: Direction::Tx,
            duration_seconds: 1,
            rx_rate_bps: None,
            tx_rate_bps: Some(1),
            payload_bytes: 1,
        };
        let mut observed = open_esp_radio_hil_protocol::WifiAccessPointEvidence::default();
        assert!(validate_mcs_evidence(&rx, link, &observed).is_err());
        observed.rx_ht_data_frames = 1;
        observed.rx_ht40_mcs_frames[7] = 1;
        assert!(validate_mcs_evidence(&rx, link, &observed).is_ok());
        assert!(validate_mcs_evidence(&tx, link, &observed).is_err());
        observed.tx_ht_aggregates = 1;
        observed.tx_ht40_mcs7_aggregates = 1;
        assert!(validate_mcs_evidence(&tx, link, &observed).is_ok());
    }
}
