//! Host receiver and report writer for the production UDP TX qualification.

use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, SessionConfig, SessionLinkRequirements,
    Transport,
};

use crate::{
    Result,
    evidence::traffic_capture::{SerialCapture, await_udp_tx_ready},
    invalidate_previous_report,
    qualification::scenario::PhyExpectation,
    traffic::bidirectional::{
        AmpduEvidence, MIN_QUALIFIED_AGGREGATES, TxQualification,
        post_block_ack_delivery_loss_lower_bound,
    },
    transport::lab_config::{LabConfig, StationFixtureConfig},
    transport::local_linux_fixture::{LocalLinuxTxCapture, LocalLinuxTxEvidence},
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
    require_exact_delivery: bool,
    require_no_beacon_loss: bool,
    require_driver_observation: bool,
) -> Result<()> {
    let mut options = parse_options(&arguments, lab)?;
    fs::create_dir_all(output)?;
    invalidate_previous_report(output)?;
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
    socket.connect(SocketAddrV4::new(options.device, DEVICE_SOURCE_PORT))?;
    let host_address = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("TX qualification requires IPv4".into()),
    };
    // Admit the reverse benchmark flow through stateful host firewalls
    // without changing firewall policy. The TX-only firmware owns a bounded
    // one-packet RX queue on this port, so the probe cannot grow unbounded or
    // enter the measured radio TX accounting.
    if let Err(error) = open_reverse_flow(&socket) {
        capture.finish_to(output)?;
        return Err(error.into());
    }
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

    let session = match capture.start_session(SessionConfig {
        network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
        transport: Transport::Udp,
        direction: Direction::Tx,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        peer: Some(Ipv4Endpoint {
            address: host_address.octets(),
            port: options.port,
        }),
        target_rx: None,
        target_tx: Some(FlowConfig {
            payload_bytes: u16::try_from(options.payload)?,
            offered_rate_bps: options.offered_rate_bps,
        }),
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
    if let Some(evidence) = local_ingress.as_ref() {
        write_local_ingress_evidence(output, evidence)?;
    }
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish_to(output)?;
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
    if !require_driver_observation {
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
        if let Some(required) = options.throughput_floor_bps {
            let measured_host = host_floor.saturating_mul(1_000);
            let measured_target = device_floor_kbps.saturating_mul(1_000);
            if measured_host < required || measured_target < required {
                return Err(format!(
                    "TX throughput is below the configured floor: required={required} host={measured_host} target={measured_target} bit/s"
                )
                .into());
            }
        }
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
            },
        )?;
        println!(
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
    if require_exact_delivery && (missing != 0 || reordered != 0 || duplicates != 0) {
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
        if require_exact_delivery
            && let Some(local) = local_ingress.as_ref()
            && local.udp_packets != evidence.transport.tx_units
        {
            return Err(format!(
                "target/local wireless TX delivery mismatch: target={} local_ingress={}",
                evidence.transport.tx_units, local.udp_packets
            )
            .into());
        }
        if require_exact_delivery
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
            require_exact_delivery,
        },
    )?;
    println!(
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
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TX performance HIL\n\n\
             - Result: `PASS`\n\
             - Evidence boundary: `transport, external host sink, stack watermark; driver observation not collected`\n\
             - Device/host: `{}` / `{host_address}`\n\
             - Complete host bursts: `{}`; datagrams: `{datagrams}`; bytes: `{bytes}`\n\
             - Payload / target offered-rate bound: `{}` bytes / `{offered_rate}`\n\
             - Host/device throughput floor: `{:.3}` / `{:.3} Mbit/s`\n\
             - Host missing/reordered/duplicate datagrams (informational): `{missing}` / `{reordered}` / `{duplicates}`\n\
             - Host UDP `SO_RCVBUF` read-back: `{host_receive_buffer_bytes}` bytes\n\
             - Target transport: `{}` bytes / `{}` datagrams / `{}` us\n\
             - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
             - Evidence CRC32C: `0x{:08x}`\n\n\
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
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TX-only HIL\n\n\
             - Result: `PASS`\n\
             - Delivery contract: `{}`\n\
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
             UART evidence is in [`uart.log`](uart.log).\n",
            if report.require_exact_delivery {
                "exact"
            } else {
                "performance-health"
            },
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
            ],
            &LabConfig::for_test(),
        )
        .unwrap();
        assert_eq!(options.duration, Duration::from_secs(8));
        assert_eq!(options.payload, 1_200);
        assert_eq!(options.offered_rate_bps, Some(80_000_000));
        assert_eq!(options.bandwidth_mhz, 40);
        assert_eq!(options.minimum_rate_kbps, 135_000);
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
