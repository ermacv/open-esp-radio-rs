//! Host receiver and report writer for the production UDP TX qualification.

use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, SessionConfig, Transport,
};

use crate::{
    Result,
    bidirectional::{AmpduEvidence, qualify_tx_log},
    traffic_capture::{SerialCapture, await_device_marker},
};

const DEFAULT_PORT: u16 = 9_002;
const DEVICE_SOURCE_PORT: u16 = 4_324;
const DEFAULT_DURATION: Duration = Duration::from_secs(16);
const DEFAULT_PAYLOAD: usize = 1_472;
const BURST_IDLE: Duration = Duration::from_millis(500);
const MIN_BURST_DATAGRAMS: u64 = 1_000;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);
const DEVICE_TX_READY_MARKER: &str = "result=PASS stage=udp-tx-ready ";

#[derive(Debug, Eq, PartialEq)]
struct Options {
    device: Ipv4Addr,
    port: u16,
    duration: Duration,
    payload: usize,
    offered_rate_bps: Option<u64>,
    serial: PathBuf,
    bandwidth_mhz: u16,
    rate_kbps: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Burst {
    pub(crate) bytes: u64,
    pub(crate) datagrams: u64,
    pub(crate) missing: u64,
    pub(crate) reordered: u64,
    pub(crate) duplicates: u64,
    pub(crate) elapsed_us: u64,
    pub(crate) started_at_zero: bool,
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
        if !self.seen_sequences.insert(sequence) {
            self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
        } else if sequence < self.highest_sequence {
            self.evidence.reordered = self.evidence.reordered.saturating_add(1);
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
        self.evidence.elapsed_us = self
            .last
            .duration_since(self.started)
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX)
            .max(1);
        self.evidence
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
    let mut options = parse_options(&arguments)?;
    let output = root.join("target/hil/esp32s31/qualification/open-radio-tx");
    fs::create_dir_all(&output)?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, options.port))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let discovered_address = match await_device_marker(
        &capture,
        DEVICE_TX_READY_MARKER,
        options.device,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
    };
    options.device = discovered_address.address;
    let route_probe = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    route_probe.connect(SocketAddrV4::new(options.device, DEVICE_SOURCE_PORT))?;
    let host_address = match route_probe.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("TX qualification requires IPv4".into()),
    };
    // Admit the reverse benchmark flow through stateful host firewalls
    // without changing firewall policy. The TX-only firmware owns a bounded
    // one-packet RX queue on this port, so the probe cannot grow unbounded or
    // enter the measured radio TX accounting.
    socket.send_to(&[0], SocketAddrV4::new(options.device, DEVICE_SOURCE_PORT))?;

    let session = if discovered_address.runtime_session {
        Some(capture.start_session(SessionConfig {
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
        })?)
    } else {
        None
    };
    let receive_duration = if session.is_some() {
        options.duration.saturating_add(Duration::from_secs(2))
    } else {
        options.duration
    };
    let bursts = receive_bursts(&socket, options.device, receive_duration)?;
    let structured = if let Some(session) = session {
        let evidence = match capture.wait_for_session(session, Duration::from_secs(5)) {
            Ok(evidence) => evidence,
            Err(error) => {
                let log = capture.finish();
                fs::write(output.join("uart.log"), &log)?;
                return Err(error);
            }
        };
        if let Err(error) = capture.acknowledge_session(session) {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
        Some(evidence)
    } else {
        None
    };
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;

    let qualified: Vec<_> = bursts
        .iter()
        .copied()
        .filter(|burst| burst.started_at_zero && burst.datagrams >= MIN_BURST_DATAGRAMS)
        .collect();
    let minimum_bursts = if structured.is_some() { 1 } else { 2 };
    if qualified.len() < minimum_bursts {
        return Err(format!(
            "received only {} complete TX bursts; required {minimum_bursts}",
            qualified.len()
        )
        .into());
    }
    let missing: u64 = qualified.iter().map(|burst| burst.missing).sum();
    let reordered: u64 = qualified.iter().map(|burst| burst.reordered).sum();
    let duplicates: u64 = qualified.iter().map(|burst| burst.duplicates).sum();
    if missing != 0 || reordered != 0 || duplicates != 0 {
        return Err(format!(
            "host observed TX sequence defects: missing={missing} reordered={reordered} \
             duplicates={duplicates}"
        )
        .into());
    }
    let host_floor = qualified
        .iter()
        .map(|burst| burst.throughput_kbps())
        .min()
        .expect("at least one qualified burst");
    let tx = qualify_tx_log(&log, options.bandwidth_mhz, options.rate_kbps)?;
    if let Some(evidence) = structured {
        let received_bytes = qualified.iter().map(|burst| burst.bytes).sum::<u64>();
        let received_datagrams = qualified.iter().map(|burst| burst.datagrams).sum::<u64>();
        let typed_throughput_kbps = evidence
            .transport
            .tx_bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(evidence.transport.elapsed_micros.max(1))
            .unwrap_or(0);
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
        if evidence.transport.tx_bytes != received_bytes
            || evidence.transport.tx_units != received_datagrams
        {
            return Err(format!(
                "typed/host TX delivery mismatch: target={}/{} host={received_bytes}/{received_datagrams}",
                evidence.transport.tx_bytes, evidence.transport.tx_units
            )
            .into());
        }
        if typed_throughput_kbps != tx.throughput_floor_kbps {
            return Err(format!(
                "typed/text TX throughput mismatch: {typed_throughput_kbps}/{} kbit/s",
                tx.throughput_floor_kbps
            )
            .into());
        }
    }
    write_report(
        &output,
        TxReport {
            options: &options,
            host_address,
            bursts: &qualified,
            host_floor_kbps: host_floor,
            device_floor_kbps: tx.throughput_floor_kbps,
            device_samples: tx.sample_count,
            ampdu: tx.ampdu,
            structured,
        },
    )?;
    println!(
        "OPENRADIOHOST result=PASS mode=tx host_floor_kbps={host_floor} \
         device_floor_kbps={} bursts={} missing=0 reordered=0 duplicates=0 \
         ampdu_avg_subframes={:.2} ampdu_31={} ampdu_32={} report={}",
        tx.throughput_floor_kbps,
        qualified.len(),
        tx.ampdu.subframes as f64 / tx.ampdu.aggregates.max(1) as f64,
        tx.ampdu.thirtyone,
        tx.ampdu.full32,
        output.join("report.md").display(),
    );
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic tx <device-ipv4> [options]\n\
         \n\
         --seconds <8..300> capture duration (default 16)\n\
         --payload <64..1472> UDP payload bytes (default 1472)\n\
         --rate <bps>       optional target offered-load bound\n\
         --port <port>      host UDP sink (default 9002)\n\
         --serial <path>    diagnostics device (default /dev/ttyACM0)\n\
         --phy <he20|ht40> expected TX vector (default he20)\n\n\
         Flash `cargo hil flash udp-tx`; host address and traffic parameters \
         are provisioned at runtime."
    );
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let device = arguments
        .first()
        .ok_or("missing ESP32-S31 IPv4 address")?
        .parse::<Ipv4Addr>()?;
    let mut options = Options {
        device,
        port: DEFAULT_PORT,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        offered_rate_bps: None,
        serial: PathBuf::from("/dev/ttyACM0"),
        bandwidth_mhz: 20,
        rate_kbps: 114_700,
    };
    let mut index = 1;
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
            "--serial" => options.serial = PathBuf::from(value),
            "--phy" => match value.as_str() {
                "he20" => {
                    options.bandwidth_mhz = 20;
                    options.rate_kbps = 114_700;
                }
                "ht40" => {
                    options.bandwidth_mhz = 40;
                    options.rate_kbps = 150_000;
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
                if sequence == 0 && active.is_some() {
                    bursts.push(active.take().expect("checked active burst").finish());
                }
                match &mut active {
                    Some(active) => active.push(sequence, length, now),
                    None => active = Some(ActiveBurst::new(sequence, length, now)),
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if active
                    .as_ref()
                    .is_some_and(|active| active.last.elapsed() >= BURST_IDLE)
                {
                    bursts.push(active.take().expect("checked active burst").finish());
                }
            }
            Err(error) => return Err(error),
        }
    }
    if let Some(active) = active {
        bursts.push(active.finish());
    }
    Ok(bursts)
}

struct TxReport<'a> {
    options: &'a Options,
    host_address: Ipv4Addr,
    bursts: &'a [Burst],
    host_floor_kbps: u64,
    device_floor_kbps: u64,
    device_samples: usize,
    ampdu: AmpduEvidence,
    structured: Option<crate::traffic_capture::SessionEvidence>,
}

fn write_report(output: &Path, report: TxReport<'_>) -> Result<()> {
    let datagrams: u64 = report.bursts.iter().map(|burst| burst.datagrams).sum();
    let bytes: u64 = report.bursts.iter().map(|burst| burst.bytes).sum();
    let terminal_exchanges = report
        .ampdu
        .completed
        .saturating_add(report.ampdu.timeout)
        .saturating_add(report.ampdu.collision);
    let structured_report = report
        .structured
        .map(|evidence| {
            format!(
                "- Typed session evidence: `{}` bytes / `{}` datagrams / `{}` us; CRC32C `0x{:08x}`\n",
                evidence.transport.tx_bytes,
                evidence.transport.tx_units,
                evidence.transport.elapsed_micros,
                evidence.finished.evidence_crc32c,
            )
        })
        .unwrap_or_else(|| String::from("- Typed session evidence: compatibility mode\n"));
    let offered_rate = report
        .options
        .offered_rate_bps
        .map(|rate| format!("{:.3} Mbit/s", rate as f64 / 1_000_000.0))
        .unwrap_or_else(|| String::from("saturated"));
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TX-only HIL\n\n\
             - Result: `PASS`\n\
             - Device/host: `{}` / `{}`\n\
             - Complete host bursts: `{}`; datagrams: `{datagrams}`; bytes: `{bytes}`\n\
             - Payload / target offered-rate bound: `{}` bytes / `{offered_rate}`\n\
             {structured_report}\
             - Host receive floor: `{:.3} Mbit/s`\n\
             - Device socket floor: `{:.3} Mbit/s` across `{}` samples\n\
             - Host missing/reordered/duplicate datagrams: `0` / `0` / `0`\n\n\
             ## A-MPDU evidence\n\n\
             - Prepared/completed/publications: `{}` / `{}` / `{}`\n\
             - Subframes: `{}` total, `{:.2}` average, min `{}`, max `{}`\n\
             - Exact 31 / full 32: `{}` / `{}`\n\
             - Build stop at frame / capacity / empty queue: `{}` / `{}` / `{}`\n\
             - Acknowledged/individual fallback: `{}` / `{}`\n\
             - Hardware timeouts/collisions: `{}` / `{}`\n\
             - Preparation average/max: `{:.2}` / `{}` us\n\
             - Publication average/max: `{:.2}` / `{}` us\n\
             - Exchange average/max: `{:.2}` / `{}` us\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            report.options.device,
            report.host_address,
            report.bursts.len(),
            report.options.payload,
            report.host_floor_kbps as f64 / 1_000.0,
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
            report.ampdu.preparation_us as f64 / report.ampdu.aggregates.max(1) as f64,
            report.ampdu.preparation_max_us,
            report.ampdu.publication_us as f64 / report.ampdu.publications.max(1) as f64,
            report.ampdu.publication_max_us,
            report.ampdu.exchange_us as f64 / terminal_exchanges.max(1) as f64,
            report.ampdu.exchange_max_us,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tx_options() {
        let options = parse_options(&[
            "192.168.178.141".into(),
            "--seconds".into(),
            "8".into(),
            "--payload".into(),
            "1200".into(),
            "--rate".into(),
            "80M".into(),
            "--phy".into(),
            "ht40".into(),
        ])
        .unwrap();
        assert_eq!(options.duration, Duration::from_secs(8));
        assert_eq!(options.payload, 1_200);
        assert_eq!(options.offered_rate_bps, Some(80_000_000));
        assert_eq!(options.bandwidth_mhz, 40);
        assert_eq!(options.rate_kbps, 150_000);
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
    }
}
