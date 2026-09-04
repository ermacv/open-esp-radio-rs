//! Host receiver and report writer for the production UDP TX qualification.

use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, SessionConfig, SessionFlowConfig,
    SessionLinkRequirements, Transport,
};

use crate::{
    Result,
    evidence::traffic_capture::{SerialCapture, await_udp_tx_ready},
    qualification::scenario::PhyExpectation,
    traffic::bidirectional::{
        AmpduEvidence, MIN_QUALIFIED_AGGREGATES, TaskPollSet, TxQualification,
        post_block_ack_delivery_loss_lower_bound, task_poll_markdown, task_polls_from_log,
    },
    traffic::host_network::BenchmarkIpv4Route,
    transport::lab_config::{LabConfig, StationFixtureConfig},
    transport::local_air_monitor::{
        AirIntervalSummary, LocalAirMonitorCapture, LocalAirMonitorEvidence,
    },
    transport::local_linux_fixture::{LocalLinuxTxCapture, LocalLinuxTxEvidence},
    transport::openwrt_fixture::{
        ChannelUtilization, OpenWrtStationLinkEvidence, require_idle_channel_utilization,
        station_link,
    },
    transport::station_fixture::require_ht40_mcs7,
    transport::udp_socket::{configure_qualification_receive_buffer, open_reverse_flow},
};

const DEFAULT_PORT: u16 = 9_002;
const DEVICE_SOURCE_PORT: u16 = 4_324;
const DEFAULT_DURATION: Duration = Duration::from_secs(16);
const DEFAULT_PAYLOAD: usize = 1_472;
const MIN_BURST_DATAGRAMS: u64 = 1_000;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Eq, PartialEq)]
struct Options {
    device: Ipv4Addr,
    port: u16,
    duration: Duration,
    payload: usize,
    offered_rate_bps: Option<u64>,
    throughput_floor_bps: Option<u64>,
    serial: PathBuf,
    bandwidth_mhz: u16,
    minimum_rate_kbps: u64,
    maximum_idle_channel_utilization_255: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Burst {
    pub(crate) bytes: u64,
    pub(crate) datagrams: u64,
    pub(crate) missing: u64,
    pub(crate) reordered: u64,
    pub(crate) first_reordered_after: Option<u32>,
    pub(crate) first_reordered_sequence: Option<u32>,
    pub(crate) maximum_reorder_distance: u32,
    pub(crate) duplicates: u64,
    pub(crate) elapsed_us: u64,
    pub(crate) started_at_zero: bool,
    pub(crate) lowest_sequence: u32,
    pub(crate) highest_sequence: u32,
    pub(crate) maximum_interarrival_us: u64,
    pub(crate) sequence_after_maximum_interarrival: Option<u32>,
    pub(crate) missing_runs: u64,
    pub(crate) maximum_missing_run: u64,
    pub(crate) maximum_missing_run_start: Option<u32>,
    pub(crate) maximum_missing_run_end: Option<u32>,
}

impl Burst {
    pub(crate) fn throughput_kbps(self) -> u64 {
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(self.elapsed_us.max(1))
            .unwrap_or(0)
    }
}

struct ActiveBurst {
    evidence: Burst,
    started: Instant,
    last: Instant,
    lowest_sequence: u32,
    highest_sequence: u32,
    seen_sequences: HashSet<u32>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EvidencePolicy {
    pub(crate) require_exact_delivery: bool,
    pub(crate) require_no_beacon_loss: bool,
    pub(crate) require_driver_observation: bool,
    pub(crate) capture_independent_laptop_air_monitor: bool,
}

impl ActiveBurst {
    fn new(sequence: u32, length: usize, now: Instant) -> Self {
        let mut seen_sequences = HashSet::new();
        seen_sequences.insert(sequence);
        Self {
            evidence: Burst {
                bytes: length as u64,
                datagrams: 1,
                started_at_zero: sequence == 0,
                ..Burst::default()
            },
            started: now,
            last: now,
            lowest_sequence: sequence,
            highest_sequence: sequence,
            seen_sequences,
        }
    }

    fn push(&mut self, sequence: u32, length: usize, now: Instant) {
        let interarrival_us = now
            .duration_since(self.last)
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        if interarrival_us > self.evidence.maximum_interarrival_us {
            self.evidence.maximum_interarrival_us = interarrival_us;
            self.evidence.sequence_after_maximum_interarrival = Some(sequence);
        }
        if !self.seen_sequences.insert(sequence) {
            self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
        } else if sequence < self.highest_sequence {
            self.evidence.reordered = self.evidence.reordered.saturating_add(1);
            if self.evidence.first_reordered_sequence.is_none() {
                self.evidence.first_reordered_after = Some(self.highest_sequence);
                self.evidence.first_reordered_sequence = Some(sequence);
            }
            self.evidence.maximum_reorder_distance = self
                .evidence
                .maximum_reorder_distance
                .max(self.highest_sequence - sequence);
        }
        self.lowest_sequence = self.lowest_sequence.min(sequence);
        self.highest_sequence = self.highest_sequence.max(sequence);
        self.evidence.bytes = self.evidence.bytes.saturating_add(length as u64);
        self.evidence.datagrams = self.evidence.datagrams.saturating_add(1);
        self.last = now;
    }

    fn finish(mut self) -> Burst {
        let sequence_span = u64::from(self.highest_sequence - self.lowest_sequence) + 1;
        self.evidence.missing = sequence_span.saturating_sub(self.seen_sequences.len() as u64);
        let mut active_missing_run_start = None;
        for sequence in self.lowest_sequence..=self.highest_sequence {
            if self.seen_sequences.contains(&sequence) {
                if let Some(start) = active_missing_run_start.take() {
                    self.record_missing_run(start, sequence - 1);
                }
            } else if active_missing_run_start.is_none() {
                active_missing_run_start = Some(sequence);
            }
        }
        if let Some(start) = active_missing_run_start {
            self.record_missing_run(start, self.highest_sequence);
        }
        self.evidence.elapsed_us = self
            .last
            .duration_since(self.started)
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
            .max(1);
        self.evidence.lowest_sequence = self.lowest_sequence;
        self.evidence.highest_sequence = self.highest_sequence;
        self.evidence
    }

    fn record_missing_run(&mut self, start: u32, end: u32) {
        let length = u64::from(end - start) + 1;
        self.evidence.missing_runs = self.evidence.missing_runs.saturating_add(1);
        if length > self.evidence.maximum_missing_run {
            self.evidence.maximum_missing_run = length;
            self.evidence.maximum_missing_run_start = Some(start);
            self.evidence.maximum_missing_run_end = Some(end);
        }
    }
}

pub(crate) fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    evidence_policy: EvidencePolicy,
) -> Result<()> {
    let mut options = parse_options(&arguments, lab)?;
    fs::create_dir_all(output)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.port))?;
    let host_receive_buffer_bytes = configure_qualification_receive_buffer(&socket)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let discovered_address =
        match await_udp_tx_ready(&capture, lab, options.device, DEVICE_READY_TIMEOUT) {
            Ok(address) => address,
            Err(error) => {
                capture.finish_to(output)?;
                return Err(error);
            }
        };
    options.device = discovered_address.address;
    let host_route = match BenchmarkIpv4Route::discover(options.device, &lab.station_fixture) {
        Ok(route) => route,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    socket.connect(SocketAddrV4::new(options.device, DEVICE_SOURCE_PORT))?;
    let host_address = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("TX qualification requires IPv4".into()),
    };
    host_route.verify_socket_source(host_address)?;
    host_route.record(output, options.device, host_address)?;
    // Admit the reverse benchmark flow through stateful host firewalls
    // without changing firewall policy. The TX-only firmware owns a bounded
    // one-packet RX queue on this port, so the probe cannot grow unbounded or
    // enter the measured radio TX accounting.
    if let Err(error) = open_reverse_flow(&socket) {
        capture.finish_to(output)?;
        return Err(error.into());
    }
    let pre_workload_channel_utilization = match (
        options.maximum_idle_channel_utilization_255,
        &lab.station_fixture,
    ) {
        (Some(maximum), StationFixtureConfig::OpenWrt(config)) => {
            match require_idle_channel_utilization(config, maximum) {
                Ok(utilization) => Some(utilization),
                Err(error) => {
                    capture.finish_to(output)?;
                    return Err(error);
                }
            }
        }
        (Some(_), StationFixtureConfig::LocalLinux(_) | StationFixtureConfig::External(_)) => {
            capture.finish_to(output)?;
            return Err("TX idle-channel evidence requires a managed OpenWrt fixture".into());
        }
        (None, _) => None,
    };
    let local_ingress_capture = match &lab.station_fixture {
        StationFixtureConfig::LocalLinux(config) => Some(LocalLinuxTxCapture::start(
            config,
            options.device,
            DEVICE_SOURCE_PORT,
            options.port,
            options.duration,
            if options.bandwidth_mhz == 40 {
                PhyExpectation::Ht40
            } else {
                PhyExpectation::He20
            },
        )?),
        StationFixtureConfig::OpenWrt(_) | StationFixtureConfig::External(_) => None,
    };
    let independent_air_capture = if evidence_policy.capture_independent_laptop_air_monitor {
        let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
            return Err("independent laptop evidence requires an OpenWrt station fixture".into());
        };
        Some(LocalAirMonitorCapture::start(
            config,
            options.device,
            options.duration,
            output,
        )?)
    } else {
        None
    };

    let session = match capture.start_session(SessionConfig {
        network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Tx,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                peer: Some(Ipv4Endpoint {
                    address: host_address.octets(),
                    port: options.port,
                }),
                target_rx: None,
                target_tx: Some(FlowConfig {
                    payload_bytes: u16::try_from(options.payload)?,
                    offered_rate_bps: options.offered_rate_bps,
                    pacing_group_datagrams: None,
                }),
            }),
            None,
        ],
        link_requirements: SessionLinkRequirements::tx_block_ack(0),
    }) {
        Ok(session) => session,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    let receive_duration = options.duration.saturating_add(Duration::from_secs(2));
    let bursts = receive_bursts(&socket, options.device, receive_duration)?;
    let structured = match capture.wait_for_session(session, Duration::from_secs(5)) {
        Ok(evidence) => evidence,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    if let Err(error) = capture.acknowledge_session(session) {
        capture.finish_to(output)?;
        return Err(error);
    }
    let local_ingress = local_ingress_capture
        .map(LocalLinuxTxCapture::finish)
        .transpose()?;
    let independent_air = independent_air_capture
        .map(LocalAirMonitorCapture::finish)
        .transpose()?;
    if let Some(evidence) = independent_air.as_ref() {
        fs::write(
            output.join("independent-air.json"),
            serde_json::to_vec_pretty(evidence)?,
        )?;
    }
    let openwrt_link = match &lab.station_fixture {
        StationFixtureConfig::OpenWrt(config) => Some(station_link(config, options.device)?),
        StationFixtureConfig::LocalLinux(_) | StationFixtureConfig::External(_) => None,
    };
    if let Some(evidence) = local_ingress.as_ref() {
        write_local_ingress_evidence(output, evidence)?;
    }
    let beacon_loss = evidence_policy
        .require_no_beacon_loss
        .then(|| capture.require_no_beacon_loss());
    let log = capture.finish_to(output)?;
    if let Some(result) = beacon_loss {
        result?;
    }

    let qualified: Vec<_> = bursts
        .iter()
        .copied()
        .filter(|burst| burst.started_at_zero && burst.datagrams >= MIN_BURST_DATAGRAMS)
        .collect();
    let minimum_bursts = 1;
    if qualified.len() < minimum_bursts {
        return Err(format!(
            "received only {} complete TX bursts; required {minimum_bursts}; {}",
            qualified.len(),
            describe_bursts(&bursts),
        )
        .into());
    }
    let missing: u64 = qualified.iter().map(|burst| burst.missing).sum();
    let reordered: u64 = qualified.iter().map(|burst| burst.reordered).sum();
    let duplicates: u64 = qualified.iter().map(|burst| burst.duplicates).sum();
    let device_floor_kbps = structured
        .transport
        .tx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    let host_floor = qualified
        .iter()
        .map(|burst| burst.throughput_kbps())
        .min()
        .expect("at least one qualified burst");
    let link_report = require_performance_link(
        &lab.station_fixture,
        options.bandwidth_mhz,
        local_ingress.as_ref(),
        openwrt_link.as_ref(),
    )?;
    if !evidence_policy.require_driver_observation {
        if structured.radio.is_some()
            || structured.tx_timing.is_some()
            || structured.rx_delivery.is_some()
            || structured.network_scheduler.is_some()
        {
            return Err("performance image published driver-internal evidence".into());
        }
        if !structured.finished.summary.passed {
            return Err("target did not complete the typed TX session normally".into());
        }
        if structured.transport.rx_bytes != 0 || structured.transport.rx_units != 0 {
            return Err("TX-only session reported unexpected received traffic".into());
        }
        if structured.transport.transport_errors != 0 {
            return Err(format!(
                "typed TX session reported {} transport errors",
                structured.transport.transport_errors
            )
            .into());
        }
        let throughput_failure = if let Some(required) = options.throughput_floor_bps {
            let measured_host = host_floor.saturating_mul(1_000);
            let measured_target = device_floor_kbps.saturating_mul(1_000);
            if measured_host < required || measured_target < required {
                Some(format!(
                    "TX throughput is below the configured floor: required={required} host={measured_host} target={measured_target} bit/s"
                ))
            } else {
                None
            }
        } else {
            None
        };
        write_performance_report(
            output,
            TxPerformanceReport {
                options: &options,
                host_address,
                bursts: &qualified,
                host_floor_kbps: host_floor,
                device_floor_kbps,
                structured,
                host_receive_buffer_bytes,
                link_report: &link_report,
                pre_workload_channel_utilization,
                task_polls: task_polls_from_log(&log),
                core0_coarse: Core0CoarseEvidence::from_log(&log),
                tx_phases: TxPhaseEvidence::from_log(&log),
                egress_grant_timeline: EgressGrantTimelineEvidence::from_log(&log),
                ap_egress_identity: ApEgressIdentityEvidence::from_log(&log),
                ap_modeled_airtime: ApModeledAirtimeEvidence::from_log(&log),
                independent_air: independent_air.as_ref(),
                failure: throughput_failure.as_deref(),
            },
        )?;
        if let Some(failure) = throughput_failure {
            return Err(failure.into());
        }
        eprintln!(
            "OPENRADIOHOST result=PASS mode=tx-performance host_floor_kbps={host_floor} device_floor_kbps={device_floor_kbps} bursts={} host_receive_buffer_bytes={host_receive_buffer_bytes} report={}",
            qualified.len(),
            output.join("report.md").display(),
        );
        return Ok(());
    }
    let (typed_tx, typed_timing) = structured.require_tx_radio(
        options.bandwidth_mhz,
        options.minimum_rate_kbps,
        u32::try_from(MIN_QUALIFIED_AGGREGATES).unwrap_or(u32::MAX),
    )?;
    let tx = TxQualification {
        throughput_floor_kbps: device_floor_kbps,
        sample_count: 1,
        ampdu: AmpduEvidence::from_typed(typed_tx, typed_timing),
    };
    if evidence_policy.require_exact_delivery && (missing != 0 || reordered != 0 || duplicates != 0)
    {
        let received_datagrams = qualified.iter().map(|burst| burst.datagrams).sum::<u64>();
        let lowest_sequence = qualified
            .iter()
            .map(|burst| burst.lowest_sequence)
            .min()
            .unwrap_or(0);
        let highest_sequence = qualified
            .iter()
            .map(|burst| burst.highest_sequence)
            .max()
            .unwrap_or(0);
        let maximum_interarrival = qualified
            .iter()
            .max_by_key(|burst| burst.maximum_interarrival_us)
            .copied()
            .unwrap_or_default();
        let target_tx_units = Some(structured.transport.tx_units);
        let host_delivery = Burst {
            datagrams: received_datagrams,
            duplicates,
            ..Burst::default()
        };
        let post_block_ack_loss = target_tx_units.and_then(|units| {
            post_block_ack_delivery_loss_lower_bound(tx.ampdu, host_delivery, units)
        });
        return Err(format!(
            "host observed TX sequence defects: missing={missing} reordered={reordered} \
             duplicates={duplicates} received={received_datagrams} \
             range={lowest_sequence}..={highest_sequence} max_interarrival_us={} \
             sequence_after_max_interarrival={:?} missing_runs={} \
             maximum_missing_run={} maximum_missing_range={:?}..={:?} \
             target_tx_units={target_tx_units:?} ampdu_subframes={} block_acknowledged={} \
             post_block_ack_delivery_loss_lower_bound={post_block_ack_loss:?} \
             local_wireless_ingress={:?}",
            maximum_interarrival.maximum_interarrival_us,
            maximum_interarrival.sequence_after_maximum_interarrival,
            qualified
                .iter()
                .map(|burst| burst.missing_runs)
                .sum::<u64>(),
            qualified
                .iter()
                .map(|burst| burst.maximum_missing_run)
                .max()
                .unwrap_or(0),
            qualified
                .iter()
                .max_by_key(|burst| burst.maximum_missing_run)
                .and_then(|burst| burst.maximum_missing_run_start),
            qualified
                .iter()
                .max_by_key(|burst| burst.maximum_missing_run)
                .and_then(|burst| burst.maximum_missing_run_end),
            tx.ampdu.subframes,
            tx.ampdu.acknowledged,
            local_ingress.as_ref().map(|evidence| evidence.udp_packets),
        )
        .into());
    }
    if let Some(required) = options.throughput_floor_bps {
        let measured_host = host_floor.saturating_mul(1_000);
        let measured_target = tx.throughput_floor_kbps.saturating_mul(1_000);
        if measured_host < required || measured_target < required {
            return Err(format!(
                "TX throughput is below the configured floor: required={required} host={measured_host} target={measured_target} bit/s"
            )
            .into());
        }
    }
    {
        let evidence = structured;
        let received_bytes = qualified.iter().map(|burst| burst.bytes).sum::<u64>();
        let received_datagrams = qualified.iter().map(|burst| burst.datagrams).sum::<u64>();
        if !evidence.finished.summary.passed {
            return Err("target did not complete the typed TX session normally".into());
        }
        if evidence.transport.rx_bytes != 0 || evidence.transport.rx_units != 0 {
            return Err("TX-only session reported unexpected received traffic".into());
        }
        if evidence.transport.transport_errors != 0 {
            return Err(format!(
                "typed TX session reported {} transport errors",
                evidence.transport.transport_errors
            )
            .into());
        }
        if evidence_policy.require_exact_delivery
            && let Some(local) = local_ingress.as_ref()
            && local.udp_packets != evidence.transport.tx_units
        {
            return Err(format!(
                "target/local wireless TX delivery mismatch: target={} local_ingress={}",
                evidence.transport.tx_units, local.udp_packets
            )
            .into());
        }
        if evidence_policy.require_exact_delivery
            && (evidence.transport.tx_bytes != received_bytes
                || evidence.transport.tx_units != received_datagrams)
        {
            return Err(format!(
                "typed/host TX delivery mismatch: target={}/{} host={received_bytes}/{received_datagrams} \
                 range={}..={} max_interarrival_us={} sequence_after_max_interarrival={:?}",
                evidence.transport.tx_bytes,
                evidence.transport.tx_units,
                qualified[0].lowest_sequence,
                qualified[0].highest_sequence,
                qualified[0].maximum_interarrival_us,
                qualified[0].sequence_after_maximum_interarrival,
            )
            .into());
        }
    }
    write_report(
        output,
        TxReport {
            options: &options,
            host_address,
            bursts: &qualified,
            host_floor_kbps: host_floor,
            device_floor_kbps: tx.throughput_floor_kbps,
            device_samples: tx.sample_count,
            ampdu: tx.ampdu,
            structured,
            host_receive_buffer_bytes,
            require_exact_delivery: evidence_policy.require_exact_delivery,
            link_report: &link_report,
            pre_workload_channel_utilization,
            task_polls: task_polls_from_log(&log),
            independent_air: independent_air.as_ref(),
        },
    )?;
    eprintln!(
        "OPENRADIOHOST result=PASS mode=tx host_floor_kbps={host_floor} \
         device_floor_kbps={} bursts={} missing={missing} reordered={reordered} duplicates={duplicates} \
         host_receive_buffer_bytes={} ampdu_avg_subframes={:.2} ampdu_31={} ampdu_32={} report={}",
        tx.throughput_floor_kbps,
        qualified.len(),
        host_receive_buffer_bytes,
        tx.ampdu.subframes as f64 / tx.ampdu.aggregates.max(1) as f64,
        tx.ampdu.thirtyone,
        tx.ampdu.full32,
        output.join("report.md").display(),
    );
    Ok(())
}

fn write_local_ingress_evidence(output: &Path, evidence: &LocalLinuxTxEvidence) -> Result<()> {
    fs::write(
        output.join("local-wireless-ingress.txt"),
        format!(
            "udp_packets={}\nchannel_width_mhz={}\ntx_bitrate={}\nrx_bitrate={}\n",
            evidence.udp_packets,
            evidence.channel_width_mhz,
            evidence.tx_bitrate,
            evidence.rx_bitrate,
        ),
    )?;
    Ok(())
}

fn require_performance_link(
    fixture: &StationFixtureConfig,
    bandwidth_mhz: u16,
    local: Option<&LocalLinuxTxEvidence>,
    openwrt: Option<&OpenWrtStationLinkEvidence>,
) -> Result<String> {
    match fixture {
        StationFixtureConfig::LocalLinux(_) => {
            let evidence = local.ok_or("local AP link snapshot is unavailable")?;
            if bandwidth_mhz == 40 {
                require_ht40_mcs7(
                    "STA TX/AP RX",
                    evidence.channel_width_mhz,
                    &evidence.rx_bitrate,
                )?;
            }
            Ok(format!(
                "local AP interface width={} MHz; AP TX/RX bitrate=`{}` / `{}`",
                evidence.channel_width_mhz, evidence.tx_bitrate, evidence.rx_bitrate
            ))
        }
        StationFixtureConfig::OpenWrt(_) => {
            let evidence = openwrt.ok_or("OpenWrt AP link snapshot is unavailable")?;
            if bandwidth_mhz == 40 {
                require_ht40_mcs7(
                    "STA TX/AP RX",
                    evidence.channel_width_mhz,
                    &evidence.rx_bitrate,
                )?;
            }
            Ok(format!(
                "OpenWrt AP interface width={} MHz; AP TX/RX bitrate=`{}` / `{}`",
                evidence.channel_width_mhz, evidence.tx_bitrate, evidence.rx_bitrate
            ))
        }
        StationFixtureConfig::External(_) if bandwidth_mhz == 40 => {
            Err("HT40 performance requires a managed fixture link snapshot".into())
        }
        StationFixtureConfig::External(_) => {
            Ok(String::from("external AP; link vector not observed"))
        }
    }
}

fn parse_options(arguments: &[String], lab: &LabConfig) -> Result<Options> {
    let mut options = Options {
        device: Ipv4Addr::UNSPECIFIED,
        port: DEFAULT_PORT,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        offered_rate_bps: None,
        throughput_floor_bps: None,
        serial: lab.device.serial.clone(),
        bandwidth_mhz: 20,
        minimum_rate_kbps: 114_700,
        maximum_idle_channel_utilization_255: None,
    };
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("TX option requires a value")?;
        match arguments[index].as_str() {
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(8..=300).contains(&seconds) {
                    return Err("--seconds must be in 8..=300".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--port" => options.port = value.parse::<u16>()?,
            "--payload" => {
                options.payload = value.parse::<usize>()?;
                if !(64..=1_472).contains(&options.payload) {
                    return Err("--payload must be in 64..=1472".into());
                }
            }
            "--rate" => options.offered_rate_bps = Some(parse_rate(value)?),
            "--floor" => options.throughput_floor_bps = Some(parse_rate(value)?),
            "--max-idle-channel-utilization-255" => {
                let maximum = value.parse::<u8>()?;
                if maximum == 0 {
                    return Err("--max-idle-channel-utilization-255 must be nonzero".into());
                }
                options.maximum_idle_channel_utilization_255 = Some(maximum);
            }
            "--phy" => match value.as_str() {
                "he20" => {
                    options.bandwidth_mhz = 20;
                    options.minimum_rate_kbps = 114_700;
                }
                "ht40" => {
                    options.bandwidth_mhz = 40;
                    options.minimum_rate_kbps = 135_000;
                }
                _ => return Err("--phy must be he20 or ht40".into()),
            },
            other => return Err(format!("unknown TX option `{other}`").into()),
        }
        index += 2;
    }
    if options.port == 0 {
        return Err("--port must be nonzero".into());
    }
    Ok(options)
}

fn parse_rate(value: &str) -> Result<u64> {
    let (digits, multiplier) = match value.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1_000_u64),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1_000_000_u64),
        Some(b'g' | b'G') => (&value[..value.len() - 1], 1_000_000_000_u64),
        _ => (value, 1),
    };
    let rate = digits
        .parse::<u64>()?
        .checked_mul(multiplier)
        .ok_or("rate overflow")?;
    if !(100_000..=1_000_000_000).contains(&rate) {
        return Err("--rate must be in 100K..=1G".into());
    }
    Ok(rate)
}

pub(crate) fn receive_bursts(
    socket: &UdpSocket,
    expected_device: Ipv4Addr,
    duration: Duration,
) -> std::io::Result<Vec<Burst>> {
    let deadline = Instant::now() + duration;
    let mut packet = [0_u8; 2_048];
    let mut active: Option<ActiveBurst> = None;
    let mut bursts = Vec::new();
    let mut ignored_reverse_probe_error = false;
    while Instant::now() < deadline {
        match socket.recv_from(&mut packet) {
            Ok((length, source)) => {
                if !matches!(source, SocketAddr::V4(source) if *source.ip() == expected_device) {
                    continue;
                }
                let Some(encoded) = packet.get(..4).and_then(|bytes| bytes.try_into().ok()) else {
                    continue;
                };
                let sequence = u32::from_be_bytes(encoded);
                let now = Instant::now();
                match &mut active {
                    Some(active) => active.push(sequence, length, now),
                    None => active = Some(ActiveBurst::new(sequence, length, now)),
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            // The only host-to-target datagram on this connected socket is a
            // one-byte conntrack probe. A reset-separated run can associate
            // its delayed ICMP Port Unreachable with the reused four-tuple.
            // Ignore at most that single asynchronous error; burst count and
            // sequence validation still reject absent or defective TX data.
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionRefused
                    && !ignored_reverse_probe_error =>
            {
                ignored_reverse_probe_error = true;
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(active) = active {
        bursts.push(active.finish());
    }
    Ok(bursts)
}

pub(crate) fn describe_bursts(bursts: &[Burst]) -> String {
    let datagrams = bursts.iter().map(|burst| burst.datagrams).sum::<u64>();
    let zero_started = bursts.iter().filter(|burst| burst.started_at_zero).count();
    let lowest = bursts.iter().map(|burst| burst.lowest_sequence).min();
    let highest = bursts.iter().map(|burst| burst.highest_sequence).max();
    format!(
        "observed_bursts={} observed_datagrams={datagrams} zero_started={zero_started} sequence_range={lowest:?}..={highest:?}",
        bursts.len(),
    )
}

struct TxPerformanceReport<'a> {
    options: &'a Options,
    host_address: Ipv4Addr,
    bursts: &'a [Burst],
    host_floor_kbps: u64,
    device_floor_kbps: u64,
    structured: crate::evidence::traffic_capture::SessionEvidence,
    host_receive_buffer_bytes: usize,
    link_report: &'a str,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    task_polls: TaskPollSet,
    core0_coarse: Option<Core0CoarseEvidence>,
    tx_phases: Option<TxPhaseEvidence>,
    egress_grant_timeline: Option<EgressGrantTimelineEvidence>,
    ap_egress_identity: Option<ApEgressIdentityEvidence>,
    ap_modeled_airtime: Option<ApModeledAirtimeEvidence>,
    independent_air: Option<&'a LocalAirMonitorEvidence>,
    failure: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Core0CoarseEvidence {
    radio_polls: u64,
    radio_cycles: u64,
    radio_instructions: u64,
}

impl Core0CoarseEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let line = log
            .lines()
            .find(|line| line.starts_with("ORC0C ") || line.contains(" ORC0C "))?;
        Some(Self {
            radio_polls: numeric_field(line, "radio_polls")?,
            radio_cycles: numeric_field(line, "radio_cycles")?,
            radio_instructions: numeric_field(line, "radio_instret")?,
        })
    }

    fn markdown(self, elapsed_micros: u64, datagrams: u64) -> String {
        const CPU_MHZ: f64 = 320.0;
        let available_cycles = elapsed_micros as f64 * CPU_MHZ;
        let occupancy = self.radio_cycles as f64 * 100.0 / available_cycles.max(1.0);
        let ipc = self.radio_instructions as f64 / self.radio_cycles.max(1) as f64;
        let cycles_per_datagram = self.radio_cycles as f64 / datagrams.max(1) as f64;
        let instructions_per_datagram = self.radio_instructions as f64 / datagrams.max(1) as f64;
        format!(
            "## Core0 hardware counters\n\n\
             The 320 MHz cycle and retired-instruction counters cover radio-task polls, including interrupt preemption, but exclude time while the task is pending.\n\n\
             - Radio polls: `{}`\n\
             - Cycles / retired instructions: `{}` / `{}`\n\
             - Core0 cycle occupancy: `{occupancy:.2}%`\n\
             - IPC: `{ipc:.3}`\n\
             - Cycles / instructions per TX datagram: `{cycles_per_datagram:.2}` / `{instructions_per_datagram:.2}`\n\n",
            self.radio_polls, self.radio_cycles, self.radio_instructions,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ApEgressIdentityEvidence {
    exact: u64,
    unclassified: u64,
    non_associated: u64,
    role_unbound: u64,
    interface_mismatch: u64,
    peer_slot_mismatch: u64,
    peer_generation_mismatch: u64,
    traffic_class_mismatch: u64,
}

impl ApEgressIdentityEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let line = log
            .lines()
            .find(|line| line.starts_with("ORC0TXI ") || line.contains(" ORC0TXI "))?;
        Some(Self {
            exact: numeric_field(line, "exact")?,
            unclassified: numeric_field(line, "unclassified")?,
            non_associated: numeric_field(line, "non_associated")?,
            role_unbound: numeric_field(line, "role_unbound")?,
            interface_mismatch: numeric_field(line, "interface_mismatch")?,
            peer_slot_mismatch: numeric_field(line, "peer_slot_mismatch")?,
            peer_generation_mismatch: numeric_field(line, "peer_generation_mismatch")?,
            traffic_class_mismatch: numeric_field(line, "traffic_class_mismatch")?,
        })
    }

    fn markdown(self) -> String {
        let mismatches = self
            .role_unbound
            .saturating_add(self.interface_mismatch)
            .saturating_add(self.peer_slot_mismatch)
            .saturating_add(self.peer_generation_mismatch)
            .saturating_add(self.traffic_class_mismatch);
        format!(
            "## AP egress identity correspondence\n\n\
             This is observational. AP role admission remains authoritative.\n\n\
             - Exact associated-peer identities: `{}`\n\
             - Unclassified / non-associated frames: `{}` / `{}`\n\
             - Role-unbound / interface / slot / generation / traffic-class mismatches: `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - Total role-identity mismatches: `{mismatches}`\n\n",
            self.exact,
            self.unclassified,
            self.non_associated,
            self.role_unbound,
            self.interface_mismatch,
            self.peer_slot_mismatch,
            self.peer_generation_mismatch,
            self.traffic_class_mismatch,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ApModeledAirtimeEvidence {
    modeled_aggregates: u64,
    identity_bound: u64,
    terminal_mismatch: u64,
    publications: u64,
    modeled_hundred_ns: u64,
}

impl ApModeledAirtimeEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let line = log
            .lines()
            .find(|line| line.starts_with("ORC0TXA ") || line.contains(" ORC0TXA "))?;
        if text_field(line, "hardware_measurement")? != "unavailable" {
            return None;
        }
        Some(Self {
            modeled_aggregates: numeric_field(line, "modeled_aggregates")?,
            identity_bound: numeric_field(line, "identity_bound")?,
            terminal_mismatch: numeric_field(line, "terminal_mismatch")?,
            publications: numeric_field(line, "publications")?,
            modeled_hundred_ns: numeric_field(line, "modeled_hundred_ns")?,
        })
    }

    fn markdown(self) -> String {
        let modeled_millis = self.modeled_hundred_ns as f64 / 10_000.0;
        let modeled_micros_per_aggregate =
            self.modeled_hundred_ns as f64 / self.modeled_aggregates.max(1) as f64 / 10.0;
        let publications_per_aggregate =
            self.publications as f64 / self.modeled_aggregates.max(1) as f64;
        format!(
            "## AP submitted-PPDU duration model\n\n\
             This sums the HT data-PPDU durations modeled from the exact initial and retry A-MPDU publication vectors observed by Core0. It excludes contention, protection, SIFS and BlockAck time and is not a hardware measurement of on-air airtime.\n\n\
             - Terminal aggregates / identity-bound / terminal mismatch: `{}` / `{}` / `{}`\n\
             - A-MPDU publications: `{}` ({publications_per_aggregate:.3} per terminal aggregate)\n\
             - Modeled submitted data-PPDU duration: `{}` × 100 ns ({modeled_millis:.3} ms total; {modeled_micros_per_aggregate:.3} us per terminal aggregate)\n\
             - Hardware airtime measurement: `unavailable`\n\n",
            self.modeled_aggregates,
            self.identity_bound,
            self.terminal_mismatch,
            self.publications,
            self.modeled_hundred_ns,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxPhaseEvidence {
    core0_start_calls: u64,
    core0_start_cycles: u64,
    core0_start_instructions: u64,
    core0_prepare_calls: u64,
    core0_prepare_cycles: u64,
    core0_prepare_instructions: u64,
    core0_publish_calls: u64,
    core0_publish_cycles: u64,
    core0_publish_instructions: u64,
    core0_service_calls: u64,
    core0_service_cycles: u64,
    core0_service_instructions: u64,
    core0_encode_calls: u64,
    core0_encode_cycles: u64,
    core0_encode_instructions: u64,
    core0_commit_calls: u64,
    core0_commit_cycles: u64,
    core0_commit_instructions: u64,
    core1_admission_attempts: u64,
    core1_admission_successes: u64,
    core1_admission_cycles: u64,
    core1_admission_instructions: u64,
    core1_consume_calls: u64,
    core1_consume_bytes: u64,
    core1_consume_cycles: u64,
    core1_consume_instructions: u64,
    core1_emit_cycles: u64,
    core1_emit_instructions: u64,
    core1_publication_cycles: u64,
    core1_publication_instructions: u64,
}

impl TxPhaseEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let core0 = log
            .lines()
            .find(|line| line.starts_with("ORC0TX ") || line.contains(" ORC0TX "))?;
        let core1 = log
            .lines()
            .find(|line| line.starts_with("ONTX ") || line.contains(" ONTX "))?;
        let core0_nested = log
            .lines()
            .find(|line| line.starts_with("ORC0TXN ") || line.contains(" ORC0TXN "))?;
        Some(Self {
            core0_start_calls: numeric_field(core0, "start_calls")?,
            core0_start_cycles: numeric_field(core0, "start_cycles")?,
            core0_start_instructions: numeric_field(core0, "start_instret")?,
            core0_prepare_calls: numeric_field(core0, "prepare_calls")?,
            core0_prepare_cycles: numeric_field(core0, "prepare_cycles")?,
            core0_prepare_instructions: numeric_field(core0, "prepare_instret")?,
            core0_publish_calls: numeric_field(core0, "publish_calls")?,
            core0_publish_cycles: numeric_field(core0, "publish_cycles")?,
            core0_publish_instructions: numeric_field(core0, "publish_instret")?,
            core0_service_calls: numeric_field(core0, "service_calls")?,
            core0_service_cycles: numeric_field(core0, "service_cycles")?,
            core0_service_instructions: numeric_field(core0, "service_instret")?,
            core0_encode_calls: numeric_field(core0_nested, "encode_calls")?,
            core0_encode_cycles: numeric_field(core0_nested, "encode_cycles")?,
            core0_encode_instructions: numeric_field(core0_nested, "encode_instret")?,
            core0_commit_calls: numeric_field(core0_nested, "commit_calls")?,
            core0_commit_cycles: numeric_field(core0_nested, "commit_cycles")?,
            core0_commit_instructions: numeric_field(core0_nested, "commit_instret")?,
            core1_admission_attempts: numeric_field(core1, "admission_attempts")?,
            core1_admission_successes: numeric_field(core1, "admission_successes")?,
            core1_admission_cycles: numeric_field(core1, "admission_cycles")?,
            core1_admission_instructions: numeric_field(core1, "admission_instret")?,
            core1_consume_calls: numeric_field(core1, "consume_calls")?,
            core1_consume_bytes: numeric_field(core1, "consume_bytes")?,
            core1_consume_cycles: numeric_field(core1, "consume_cycles")?,
            core1_consume_instructions: numeric_field(core1, "consume_instret")?,
            core1_emit_cycles: numeric_field(core1, "emit_cycles")?,
            core1_emit_instructions: numeric_field(core1, "emit_instret")?,
            core1_publication_cycles: numeric_field(core1, "publication_cycles")?,
            core1_publication_instructions: numeric_field(core1, "publication_instret")?,
        })
    }

    fn markdown(self, datagrams: u64, core0: Option<Core0CoarseEvidence>) -> String {
        let datagrams = datagrams.max(1) as f64;
        let core0_phase_cycles = self
            .core0_start_cycles
            .saturating_add(self.core0_prepare_cycles)
            .saturating_add(self.core0_publish_cycles)
            .saturating_add(self.core0_service_cycles);
        let core0_residual =
            core0.map(|total| total.radio_cycles.saturating_sub(core0_phase_cycles));
        let admission_failures = self
            .core1_admission_attempts
            .saturating_sub(self.core1_admission_successes);
        format!(
            "## TX phase hardware counters\n\n\
             These diagnostic per-transaction samples are intrusive. The first four Core0 bins are non-overlapping driver calls; encode and commit are nested per-frame samples and are not added to their sum. The Core0 residual is all radio-task work outside the first four calls. Core1 emission is the network stack callback, while publication is the surrounding pinned-slot/channel/wake work.\n\n\
             | Owner / phase | Calls | Cycles / datagram | Instructions / datagram |\n\
             |---|---:|---:|---:|\n\
             | Core0 fresh start | {} | {:.2} | {:.2} |\n\
             | Core0 standby prepare | {} | {:.2} | {:.2} |\n\
             | Core0 prepared publish | {} | {:.2} | {:.2} |\n\
             | Core0 terminal service | {} | {:.2} | {:.2} |\n\
             | Core0 measured phase sum | - | {:.2} | - |\n\
             | Core0 residual | - | {} | - |\n\
             | Core0 802.11/CCMP encode (nested) | {} | {:.2} | {:.2} |\n\
             | Core0 A-MPDU descriptor commit (nested) | {} | {:.2} | {:.2} |\n\
             | Core1 TX admission | {} attempts / {} successes / {} failures | {:.2} | {:.2} |\n\
             | Core1 packet emission | {} | {:.2} | {:.2} |\n\
             | Core1 driver publication | {} | {:.2} | {:.2} |\n\n\
             - Core1 emitted bytes: `{}`\n\n",
            self.core0_start_calls,
            self.core0_start_cycles as f64 / datagrams,
            self.core0_start_instructions as f64 / datagrams,
            self.core0_prepare_calls,
            self.core0_prepare_cycles as f64 / datagrams,
            self.core0_prepare_instructions as f64 / datagrams,
            self.core0_publish_calls,
            self.core0_publish_cycles as f64 / datagrams,
            self.core0_publish_instructions as f64 / datagrams,
            self.core0_service_calls,
            self.core0_service_cycles as f64 / datagrams,
            self.core0_service_instructions as f64 / datagrams,
            core0_phase_cycles as f64 / datagrams,
            core0_residual
                .map(|cycles| format!("{:.2}", cycles as f64 / datagrams))
                .unwrap_or_else(|| String::from("not available")),
            self.core0_encode_calls,
            self.core0_encode_cycles as f64 / datagrams,
            self.core0_encode_instructions as f64 / datagrams,
            self.core0_commit_calls,
            self.core0_commit_cycles as f64 / datagrams,
            self.core0_commit_instructions as f64 / datagrams,
            self.core1_admission_attempts,
            self.core1_admission_successes,
            admission_failures,
            self.core1_admission_cycles as f64 / datagrams,
            self.core1_admission_instructions as f64 / datagrams,
            self.core1_consume_calls,
            self.core1_emit_cycles as f64 / datagrams,
            self.core1_emit_instructions as f64 / datagrams,
            self.core1_consume_calls,
            self.core1_publication_cycles as f64 / datagrams,
            self.core1_publication_instructions as f64 / datagrams,
            self.core1_consume_bytes,
        )
    }
}

const EGRESS_GRANT_TIMELINE_WALL_PHASES: [&str; 6] = [
    "issue_receive",
    "receive_network_finish",
    "network_finish_progress_publish",
    "progress_publish_radio_receive",
    "issue_radio_receive",
    "radio_receive_successor_issue",
];

/// Diagnostic-only, serial-correlated timing parsed from one measured target
/// interval. It is archived beside the raw UART log rather than enlarging the
/// target's ordinary session-result value or qualification wire ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
struct EgressGrantTimelineEvidence {
    grants_issued: u64,
    grants_completed: u64,
    incomplete_completions: u64,
    slot_collisions: u64,
    unmatched_events: u64,
    wall_phases: [EgressGrantTimelinePhaseEvidence; EGRESS_GRANT_TIMELINE_WALL_PHASES.len()],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
struct EgressGrantTimelinePhaseEvidence {
    phase: &'static str,
    unit: &'static str,
    samples: u64,
    total: u64,
    lifetime_max: u64,
}

impl EgressGrantTimelineEvidence {
    fn from_log(log: &str) -> Option<Self> {
        let summary = log
            .lines()
            .find(|line| line.starts_with("ONTXTL ") || line.contains(" ONTXTL "))?;
        let wall_phases = parse_egress_timeline_phases(
            log,
            "ONTXTLP ",
            "micros",
            "total_us",
            "lifetime_max_us",
            EGRESS_GRANT_TIMELINE_WALL_PHASES,
        )?;
        Some(Self {
            grants_issued: numeric_field(summary, "issued")?,
            grants_completed: numeric_field(summary, "completed")?,
            incomplete_completions: numeric_field(summary, "incomplete")?,
            slot_collisions: numeric_field(summary, "collisions")?,
            unmatched_events: numeric_field(summary, "unmatched")?,
            wall_phases,
        })
    }

    fn markdown(self) -> String {
        let phase_rows = self
            .wall_phases
            .iter()
            .map(|phase| {
                let average = phase.total as f64 / phase.samples.max(1) as f64;
                format!(
                    "| `{}` | {} | {} | {:.3} | {} |\n",
                    phase.phase, phase.unit, phase.samples, average, phase.lifetime_max,
                )
            })
            .collect::<String>();
        format!(
            "## Serial-keyed egress grant timeline\n\n\
             This diagnostic observer joins Core0 and Core1 events by the existing affine grant serial. It does not carry ownership or participate in scheduling. `radio_receive` currently means Core0 accepted the software progress receipt; physical aggregate publication and terminal BlockAck are not yet serial-bound.\n\n\
             - Grants issued / complete / incomplete: `{}` / `{}` / `{}`\n\
             - Slot collisions / unmatched events: `{}` / `{}`\n\n\
             | Phase | Unit | Samples | Mean | Lifetime max |\n\
             |---|---|---:|---:|---:|\n\
             {phase_rows}\n",
            self.grants_issued,
            self.grants_completed,
            self.incomplete_completions,
            self.slot_collisions,
            self.unmatched_events,
        )
    }
}

fn parse_egress_timeline_phases<const N: usize>(
    log: &str,
    prefix: &str,
    unit: &'static str,
    total_key: &str,
    maximum_key: &str,
    names: [&'static str; N],
) -> Option<[EgressGrantTimelinePhaseEvidence; N]> {
    let needle = format!(" {prefix}");
    let phases: [Option<EgressGrantTimelinePhaseEvidence>; N] = core::array::from_fn(|index| {
        let phase = names[index];
        let line = log.lines().find(|line| {
            (line.starts_with(prefix) || line.contains(&needle))
                && text_field(line, "phase") == Some(phase)
        });
        line.and_then(|line| {
            Some(EgressGrantTimelinePhaseEvidence {
                phase,
                unit,
                samples: numeric_field(line, "samples")?,
                total: numeric_field(line, total_key)?,
                lifetime_max: numeric_field(line, maximum_key)?,
            })
        })
    });
    phases
        .into_iter()
        .collect::<Option<Vec<_>>>()?
        .try_into()
        .ok()
}

fn numeric_field(line: &str, key: &str) -> Option<u64> {
    line.split_ascii_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.parse().ok()).flatten()
    })
}

fn text_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_ascii_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn write_performance_report(output: &Path, report: TxPerformanceReport<'_>) -> Result<()> {
    let TxPerformanceReport {
        options,
        host_address,
        bursts,
        host_floor_kbps,
        device_floor_kbps,
        structured,
        host_receive_buffer_bytes,
        link_report,
        pre_workload_channel_utilization,
        task_polls,
        core0_coarse,
        tx_phases,
        egress_grant_timeline,
        ap_egress_identity,
        ap_modeled_airtime,
        independent_air,
        failure,
    } = report;
    let datagrams = bursts.iter().map(|burst| burst.datagrams).sum::<u64>();
    let bytes = bursts.iter().map(|burst| burst.bytes).sum::<u64>();
    let missing = bursts.iter().map(|burst| burst.missing).sum::<u64>();
    let reordered = bursts.iter().map(|burst| burst.reordered).sum::<u64>();
    let duplicates = bursts.iter().map(|burst| burst.duplicates).sum::<u64>();
    let offered_rate = options
        .offered_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let pre_workload_channel_utilization =
        format_channel_utilization(pre_workload_channel_utilization);
    let failure_report = failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    let task_poll_report = task_poll_markdown(task_polls);
    let core0_report = core0_coarse
        .map(|evidence| {
            evidence.markdown(
                structured.transport.elapsed_micros,
                structured.transport.tx_units,
            )
        })
        .unwrap_or_default();
    let tx_phase_report = tx_phases
        .map(|evidence| evidence.markdown(structured.transport.tx_units, core0_coarse))
        .unwrap_or_default();
    let egress_grant_timeline_report = egress_grant_timeline
        .map(EgressGrantTimelineEvidence::markdown)
        .unwrap_or_default();
    if let Some(evidence) = egress_grant_timeline {
        fs::write(
            output.join("egress-grant-timeline.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    }
    let ap_egress_identity_report = ap_egress_identity
        .map(ApEgressIdentityEvidence::markdown)
        .unwrap_or_default();
    let ap_modeled_airtime_report = ap_modeled_airtime
        .map(ApModeledAirtimeEvidence::markdown)
        .unwrap_or_default();
    let independent_air_report = tx_air_timing_markdown(independent_air);
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TX performance HIL\n\n\
             - Result: `{result}`\n\
             {failure_report}\
             - Evidence boundary: `transport, external host sink, stack watermark; driver observation not collected`\n\
             - AP-side link vector: {link_report}\n\
             - Pre-workload channel utilization: `{pre_workload_channel_utilization}`\n\
             - Device/host: `{}` / `{host_address}`\n\
             - Complete host bursts: `{}`; datagrams: `{datagrams}`; bytes: `{bytes}`\n\
             - Payload / target offered-rate bound: `{}` bytes / `{offered_rate}`\n\
             - Host/device throughput floor: `{:.3}` / `{:.3} Mbit/s`\n\
             - Host missing/reordered/duplicate datagrams (informational): `{missing}` / `{reordered}` / `{duplicates}`\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             - Target transport: `{}` bytes / `{}` datagrams / `{}` us\n\
             - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
             - Evidence CRC32C: `0x{:08x}`\n\n\
             {task_poll_report}\
             {core0_report}\
             {tx_phase_report}\
             {egress_grant_timeline_report}\
             {ap_egress_identity_report}\
             {ap_modeled_airtime_report}\
             {independent_air_report}\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.device,
            bursts.len(),
            options.payload,
            host_floor_kbps as f64 / 1_000.0,
            device_floor_kbps as f64 / 1_000.0,
            structured.transport.tx_bytes,
            structured.transport.tx_units,
            structured.transport.elapsed_micros,
            structured.stack.cpu0.free_bytes,
            structured.stack.cpu0.capacity_bytes,
            structured.stack.cpu0.minimum_free_bytes,
            structured.stack.cpu1.free_bytes,
            structured.stack.cpu1.capacity_bytes,
            structured.stack.cpu1.minimum_free_bytes,
            structured.finished.evidence_crc32c,
        ),
    )?;
    Ok(())
}

struct TxReport<'a> {
    options: &'a Options,
    host_address: Ipv4Addr,
    bursts: &'a [Burst],
    host_floor_kbps: u64,
    device_floor_kbps: u64,
    device_samples: usize,
    ampdu: AmpduEvidence,
    structured: crate::evidence::traffic_capture::SessionEvidence,
    host_receive_buffer_bytes: usize,
    require_exact_delivery: bool,
    link_report: &'a str,
    pre_workload_channel_utilization: Option<ChannelUtilization>,
    task_polls: TaskPollSet,
    independent_air: Option<&'a LocalAirMonitorEvidence>,
}

fn air_interval_markdown(label: &str, interval: Option<AirIntervalSummary>) -> String {
    interval.map_or_else(
        || format!("- {label}: `unavailable`\n"),
        |interval| {
            let average = interval.total_micros as f64 / f64::from(interval.samples.max(1));
            format!(
                "- {label}: samples `{}`; average/min/p50/p95/p99/max `{average:.2}` / `{}` / `{}` / `{}` / `{}` / `{}` us\n",
                interval.samples,
                interval.minimum_micros,
                interval.p50_micros,
                interval.p95_micros,
                interval.p99_micros,
                interval.maximum_micros,
            )
        },
    )
}

fn tx_air_timing_markdown(independent: Option<&LocalAirMonitorEvidence>) -> String {
    let Some(independent) = independent else {
        return String::from("## Independent TX air timing\n\nNot collected.\n\n");
    };
    let timing = &independent.target_egress;
    format!(
        "## Independent TX air timing\n\n\
         - Capture frames/kernel drops: `{}` / `{}`\n\
         - Target data records / peer BlockAck records: `{}` / `{}`\n\
         - Peer BlockAck full/tail/hole/unique MPDUs/backward starts: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Exact target-data/BlockAck pairing available: `{}`\n\
         {}{}{}\n",
        independent.captured_frames,
        independent.kernel_dropped,
        timing.target_data_frames,
        timing.peer_block_ack_frames,
        timing.peer_full_block_ack_frames,
        timing.peer_tail_block_ack_frames,
        timing.peer_hole_block_ack_frames,
        timing.peer_unique_block_acked_mpdus,
        timing.peer_backward_block_ack_starts,
        timing.target_data_pairing_available,
        air_interval_markdown(
            "Peer BlockAck interarrival",
            timing.peer_block_ack_interarrival,
        ),
        air_interval_markdown("Final target data to BlockAck", timing.data_to_block_ack),
        air_interval_markdown(
            "BlockAck to next target data",
            timing.block_ack_to_next_data,
        ),
    )
}

fn format_channel_utilization(utilization: Option<ChannelUtilization>) -> String {
    utilization
        .map(|utilization| {
            format!(
                "{}/255 (busy/active: {}/{} ms)",
                utilization.scaled_255, utilization.busy_millis, utilization.active_millis,
            )
        })
        .unwrap_or_else(|| String::from("not required"))
}

fn write_report(output: &Path, report: TxReport<'_>) -> Result<()> {
    let datagrams: u64 = report.bursts.iter().map(|burst| burst.datagrams).sum();
    let bytes: u64 = report.bursts.iter().map(|burst| burst.bytes).sum();
    let missing: u64 = report.bursts.iter().map(|burst| burst.missing).sum();
    let reordered: u64 = report.bursts.iter().map(|burst| burst.reordered).sum();
    let duplicates: u64 = report.bursts.iter().map(|burst| burst.duplicates).sum();
    let terminal_exchanges = report
        .ampdu
        .completed
        .saturating_add(report.ampdu.timeout)
        .saturating_add(report.ampdu.collision);
    let evidence = report.structured;
    let structured_report = format!(
        "- Typed session evidence: `{}` bytes / `{}` datagrams / `{}` us; CRC32C `0x{:08x}`\n\
                 - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n",
        evidence.transport.tx_bytes,
        evidence.transport.tx_units,
        evidence.transport.elapsed_micros,
        evidence.finished.evidence_crc32c,
        evidence.stack.cpu0.free_bytes,
        evidence.stack.cpu0.capacity_bytes,
        evidence.stack.cpu0.minimum_free_bytes,
        evidence.stack.cpu1.free_bytes,
        evidence.stack.cpu1.capacity_bytes,
        evidence.stack.cpu1.minimum_free_bytes,
    );
    let offered_rate = report
        .options
        .offered_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    let maximum_interarrival = report
        .bursts
        .iter()
        .max_by_key(|burst| burst.maximum_interarrival_us)
        .copied()
        .unwrap_or_default();
    let maximum_interarrival_us = maximum_interarrival.maximum_interarrival_us;
    let sequence_after_maximum_interarrival =
        maximum_interarrival.sequence_after_maximum_interarrival;
    let pre_workload_channel_utilization =
        format_channel_utilization(report.pre_workload_channel_utilization);
    let task_poll_report = task_poll_markdown(report.task_polls);
    let independent_air_report = tx_air_timing_markdown(report.independent_air);
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TX-only HIL\n\n\
             - Result: `PASS`\n\
             - Delivery contract: `{}`\n\
             - AP-side link vector: {}\n\
             - Pre-workload channel utilization: `{pre_workload_channel_utilization}`\n\
             - Device/host: `{}` / `{}`\n\
             - Complete host bursts: `{}`; datagrams: `{datagrams}`; bytes: `{bytes}`\n\
             - Payload / target offered-rate bound: `{}` bytes / `{offered_rate}`\n\
             {structured_report}\
             - Host receive floor: `{:.3} Mbit/s`\n\
             - Host UDP `SO_RCVBUF` read-back: `{}` bytes\n\
             - Host maximum packet interarrival: `{maximum_interarrival_us}` us before sequence `{sequence_after_maximum_interarrival:?}`\n\
             - Device socket floor: `{:.3} Mbit/s` across `{}` samples\n\
             - Host missing/reordered/duplicate datagrams: `{missing}` / `{reordered}` / `{duplicates}`\n\n\
             ## A-MPDU evidence\n\n\
             - Prepared/completed/publications: `{}` / `{}` / `{}`\n\
             - Subframes: `{}` total, `{:.2}` average, min `{}`, max `{}`\n\
             - Exact 31 / full 32: `{}` / `{}`\n\
             - Build stop at frame / capacity / empty queue: `{}` / `{}` / `{}`\n\
             - Acknowledged/individual fallback: `{}` / `{}`\n\
             - Hardware timeouts/collisions: `{}` / `{}`\n\
             - BlockAck samples/received/full/partial/empty: `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - BlockAck success-without-valid/control-TID/start-outside/max-start-lag: `{}` / `{}` / `{}` / `{}`\n\
             - Preparation average/max: `{:.2}` / `{}` us\n\
             - Publication average/max: `{:.2}` / `{}` us\n\
             - Exchange average/max: `{:.2}` / `{}` us\n\
             - First-publication exchange average/max: `{:.2}` / `{}` us across `{}` exchanges\n\
             - Retried exchange average/max: `{:.2}` / `{}` us across `{}` exchanges and `{}` publications\n\
             - TX IRQ wake epochs/samples/clock-skew rejects: `{}` / `{}` / `{}`; IRQ-to-service average/max: `{:.2}` / `{}` us\n\
             - Sampled publication-to-IRQ flight average/max: `{:.2}` / `{}` us across `{}` samples\n\n\
             - Standby prepared/published/cancelled: `{}` / `{}` / `{}`\n\n\
             {task_poll_report}\
             {independent_air_report}\
             UART evidence is in [`uart.log`](uart.log).\n",
            if report.require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
            report.link_report,
            report.options.device,
            report.host_address,
            report.bursts.len(),
            report.options.payload,
            report.host_floor_kbps as f64 / 1_000.0,
            report.host_receive_buffer_bytes,
            report.device_floor_kbps as f64 / 1_000.0,
            report.device_samples,
            report.ampdu.aggregates,
            report.ampdu.completed,
            report.ampdu.publications,
            report.ampdu.subframes,
            report.ampdu.subframes as f64 / report.ampdu.aggregates.max(1) as f64,
            report.ampdu.minimum,
            report.ampdu.maximum,
            report.ampdu.thirtyone,
            report.ampdu.full32,
            report.ampdu.stop_frame,
            report.ampdu.stop_capacity,
            report.ampdu.stop_empty,
            report.ampdu.acknowledged,
            report.ampdu.individual_retry,
            report.ampdu.timeout,
            report.ampdu.collision,
            report.ampdu.block_ack_samples,
            report.ampdu.block_ack_received,
            report.ampdu.full_block_ack,
            report.ampdu.partial_block_ack,
            report.ampdu.empty_block_ack,
            report.ampdu.block_ack_success_without,
            report.ampdu.block_ack_nonzero_control,
            report.ampdu.block_ack_start_outside,
            report.ampdu.block_ack_start_lag_max,
            report.ampdu.preparation_us as f64 / report.ampdu.aggregates.max(1) as f64,
            report.ampdu.preparation_max_us,
            report.ampdu.publication_us as f64 / report.ampdu.publications.max(1) as f64,
            report.ampdu.publication_max_us,
            report.ampdu.exchange_us as f64 / terminal_exchanges.max(1) as f64,
            report.ampdu.exchange_max_us,
            report.ampdu.first_exchange_us as f64 / report.ampdu.first_exchanges.max(1) as f64,
            report.ampdu.first_exchange_max_us,
            report.ampdu.first_exchanges,
            report.ampdu.retry_exchange_us as f64 / report.ampdu.retried_exchanges.max(1) as f64,
            report.ampdu.retry_exchange_max_us,
            report.ampdu.retried_exchanges,
            report.ampdu.retry_publications,
            report.ampdu.tx_irq_epochs,
            report.ampdu.tx_irq_samples,
            report.ampdu.tx_irq_skew,
            report.ampdu.tx_irq_service_us as f64 / report.ampdu.tx_irq_samples.max(1) as f64,
            report.ampdu.tx_irq_service_max_us,
            report.ampdu.tx_flight_us as f64 / report.ampdu.tx_flight_samples.max(1) as f64,
            report.ampdu.tx_flight_max_us,
            report.ampdu.tx_flight_samples,
            report.ampdu.standby_prepared,
            report.ampdu.standby_published,
            report.ampdu.standby_cancelled,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tx_options() {
        let options = parse_options(
            &[
                "--seconds".into(),
                "8".into(),
                "--payload".into(),
                "1200".into(),
                "--rate".into(),
                "80M".into(),
                "--phy".into(),
                "ht40".into(),
                "--max-idle-channel-utilization-255".into(),
                "64".into(),
            ],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.duration, Duration::from_secs(8));
        assert_eq!(options.payload, 1_200);
        assert_eq!(options.offered_rate_bps, Some(80_000_000));
        assert_eq!(options.bandwidth_mhz, 40);
        assert_eq!(options.minimum_rate_kbps, 135_000);
        assert_eq!(options.maximum_idle_channel_utilization_255, Some(64));
    }

    #[test]
    fn channel_utilization_report_preserves_the_measured_interval() {
        assert_eq!(
            format_channel_utilization(Some(ChannelUtilization {
                scaled_255: 17,
                active_millis: 12_003,
                busy_millis: 783,
            })),
            "17/255 (busy/active: 783/12003 ms)"
        );
    }

    #[test]
    fn parses_core0_tx_cycles_and_instructions() {
        let evidence = Core0CoarseEvidence::from_log(
            "ORC0C rx_irq_posts=154 radio_polls=63030 radio_cycles=2950392831 radio_instret=593043026 poll_to_runner_cycles=21183800",
        )
        .unwrap();
        assert_eq!(evidence.radio_polls, 63_030);
        assert_eq!(evidence.radio_cycles, 2_950_392_831);
        assert_eq!(evidence.radio_instructions, 593_043_026);
        let markdown = evidence.markdown(12_001_617, 119_716);
        assert!(markdown.contains("Core0 cycle occupancy: `76.82%`"));
        assert!(markdown.contains("IPC: `0.201`"));
    }

    #[test]
    fn parses_non_overlapping_tx_phase_counters() {
        let evidence = TxPhaseEvidence::from_log(
            "ORC0TX start_calls=2 start_cycles=20 start_instret=10 prepare_calls=3 prepare_cycles=30 prepare_instret=15 publish_calls=4 publish_cycles=40 publish_instret=20 service_calls=5 service_cycles=50 service_instret=25\n\
             ORC0TXN encode_calls=10 encode_cycles=70 encode_instret=35 commit_calls=10 commit_cycles=80 commit_instret=40\n\
             ONTX admission_attempts=12 admission_successes=10 admission_cycles=120 admission_instret=60 consume_calls=10 consume_bytes=14720 consume_cycles=500 consume_instret=250 emit_cycles=300 emit_instret=150 publication_cycles=200 publication_instret=100",
        )
        .unwrap();
        assert_eq!(evidence.core0_prepare_cycles, 30);
        assert_eq!(evidence.core0_encode_cycles, 70);
        assert_eq!(evidence.core1_admission_attempts, 12);
        assert_eq!(evidence.core1_admission_successes, 10);
        assert_eq!(evidence.core1_publication_cycles, 200);
        let markdown = evidence.markdown(
            10,
            Some(Core0CoarseEvidence {
                radio_polls: 7,
                radio_cycles: 200,
                radio_instructions: 100,
            }),
        );
        assert!(markdown.contains("Core0 measured phase sum | - | 14.00"));
        assert!(markdown.contains("Core0 residual | - | 6.00"));
        assert!(markdown.contains("12 attempts / 10 successes / 2 failures"));
        assert!(markdown.contains("Core1 driver publication | 10 | 20.00"));
    }

    #[test]
    fn parses_complete_serial_keyed_egress_timeline() {
        let evidence = EgressGrantTimelineEvidence::from_log(
            "ONTXTL issued=4 completed=3 incomplete=1 collisions=0 unmatched=0\n\
             ONTXTLP phase=issue_receive samples=4 total_us=40 lifetime_max_us=12\n\
             ONTXTLP phase=receive_network_finish samples=3 total_us=60 lifetime_max_us=25\n\
             ONTXTLP phase=network_finish_progress_publish samples=4 total_us=8 lifetime_max_us=3\n\
             ONTXTLP phase=progress_publish_radio_receive samples=4 total_us=16 lifetime_max_us=5\n\
             ONTXTLP phase=issue_radio_receive samples=4 total_us=160 lifetime_max_us=45\n\
             ONTXTLP phase=radio_receive_successor_issue samples=3 total_us=9 lifetime_max_us=4",
        )
        .unwrap();
        assert_eq!(evidence.grants_issued, 4);
        assert_eq!(evidence.grants_completed, 3);
        assert_eq!(evidence.incomplete_completions, 1);
        assert_eq!(evidence.wall_phases[1].phase, "receive_network_finish");
        assert_eq!(evidence.wall_phases[1].total, 60);
        assert!(
            evidence
                .markdown()
                .contains("| `issue_receive` | micros | 4 | 10.000 | 12 |")
        );
        assert!(evidence.markdown().contains(
            "physical aggregate publication and terminal BlockAck are not yet serial-bound"
        ));
    }

    #[test]
    fn parses_ap_egress_identity_correspondence_without_authorizing_it() {
        let evidence = ApEgressIdentityEvidence::from_log(
            "ORC0TXI exact=100 unclassified=2 non_associated=3 role_unbound=4 interface_mismatch=5 peer_slot_mismatch=6 peer_generation_mismatch=7 traffic_class_mismatch=8",
        )
        .unwrap();
        assert_eq!(evidence.exact, 100);
        assert_eq!(evidence.peer_generation_mismatch, 7);
        let markdown = evidence.markdown();
        assert!(markdown.contains("AP role admission remains authoritative"));
        assert!(markdown.contains("Total role-identity mismatches: `30`"));
    }

    #[test]
    fn parses_ap_modeled_airtime_with_explicit_hardware_provenance() {
        let evidence = ApModeledAirtimeEvidence::from_log(
            "ORC0TXA modeled_aggregates=10 identity_bound=10 terminal_mismatch=0 publications=12 modeled_hundred_ns=350000 hardware_measurement=unavailable",
        )
        .unwrap();
        assert_eq!(evidence.modeled_aggregates, 10);
        assert_eq!(evidence.publications, 12);
        let markdown = evidence.markdown();
        assert!(markdown.contains("1.200 per terminal aggregate"));
        assert!(markdown.contains("35.000 ms total"));
        assert!(markdown.contains("Hardware airtime measurement: `unavailable`"));
    }

    #[test]
    fn rejects_ap_modeled_airtime_that_claims_unavailable_hardware_data() {
        assert!(
            ApModeledAirtimeEvidence::from_log(
                "ORC0TXA modeled_aggregates=1 identity_bound=1 terminal_mismatch=0 publications=1 modeled_hundred_ns=29160 hardware_measurement=measured",
            )
            .is_none()
        );
        assert!(ApModeledAirtimeEvidence::from_log("unrelated").is_none());
    }

    #[test]
    fn burst_distinguishes_reordering_from_unrecovered_loss() {
        let now = Instant::now();
        let mut burst = ActiveBurst::new(0, 100, now);
        burst.push(1, 100, now);
        burst.push(3, 100, now);
        burst.push(2, 100, now);
        let evidence = burst.finish();
        assert_eq!(evidence.missing, 0);
        assert_eq!(evidence.reordered, 1);
        assert_eq!(evidence.duplicates, 0);
    }

    #[test]
    fn burst_reports_unrecovered_loss_and_duplicates_separately() {
        let now = Instant::now();
        let mut burst = ActiveBurst::new(0, 100, now);
        burst.push(2, 100, now);
        burst.push(2, 100, now);
        let evidence = burst.finish();
        assert_eq!(evidence.missing, 1);
        assert_eq!(evidence.reordered, 0);
        assert_eq!(evidence.duplicates, 1);
        assert_eq!(evidence.missing_runs, 1);
        assert_eq!(evidence.maximum_missing_run, 1);
        assert_eq!(evidence.maximum_missing_run_start, Some(1));
        assert_eq!(evidence.maximum_missing_run_end, Some(1));
    }

    #[test]
    fn burst_reports_contiguous_missing_sequence_runs() {
        let now = Instant::now();
        let mut burst = ActiveBurst::new(0, 100, now);
        burst.push(3, 100, now);
        burst.push(4, 100, now);
        burst.push(8, 100, now);
        let evidence = burst.finish();
        assert_eq!(evidence.missing, 5);
        assert_eq!(evidence.missing_runs, 2);
        assert_eq!(evidence.maximum_missing_run, 3);
        assert_eq!(evidence.maximum_missing_run_start, Some(5));
        assert_eq!(evidence.maximum_missing_run_end, Some(7));
    }

    #[test]
    fn burst_records_sequence_range_and_largest_interarrival() {
        let now = Instant::now();
        let mut burst = ActiveBurst::new(10, 100, now);
        burst.push(11, 100, now + Duration::from_micros(25));
        burst.push(12, 100, now + Duration::from_micros(125));
        let evidence = burst.finish();
        assert_eq!(evidence.lowest_sequence, 10);
        assert_eq!(evidence.highest_sequence, 12);
        assert_eq!(evidence.maximum_interarrival_us, 100);
        assert_eq!(evidence.sequence_after_maximum_interarrival, Some(12));
    }

    #[test]
    fn incomplete_burst_summary_distinguishes_missing_sequence_zero_from_no_traffic() {
        let bursts = [Burst {
            datagrams: 17,
            lowest_sequence: 41,
            highest_sequence: 57,
            ..Burst::default()
        }];
        assert_eq!(
            describe_bursts(&bursts),
            "observed_bursts=1 observed_datagrams=17 zero_started=0 sequence_range=Some(41)..=Some(57)"
        );
        assert_eq!(
            describe_bursts(&[]),
            "observed_bursts=0 observed_datagrams=0 zero_started=0 sequence_range=None..=None"
        );
    }
}
