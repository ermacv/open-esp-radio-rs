//! ICMP latency and loss qualification for an already connected target.

use crate::context::Context;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{
        AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType, connect, ipproto, recv, send,
        socket_with,
    },
};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4},
    os::fd::OwnedFd,
    path::Path,
    time::{Duration, Instant},
};

use crate::Result;
use crate::{
    evidence::run::{Comparison, Measurement, MeasurementUnit},
    session::await_network_ready,
};

const DEFAULT_COUNT: u16 = 100;
const DEFAULT_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_PAYLOAD_BYTES: usize = 56;
const MAX_PAYLOAD_BYTES: usize = 1_400;
const NETWORK_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Config {
    pub(crate) device: Ipv4Addr,
    pub(crate) count: u16,
    pub(crate) interval: Duration,
    pub(crate) timeout: Duration,
    pub(crate) payload_bytes: usize,
    pub(crate) maximum_lost: u16,
    pub(crate) maximum_p95: Option<Duration>,
}

#[derive(Clone, Debug)]
struct LatencySummary {
    transmitted: u16,
    received: u16,
    readiness_attempts: u8,
    lost_sequences: Vec<u16>,
    minimum_us: u64,
    average_us: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    maximum_us: u64,
}

impl LatencySummary {
    fn loss_percent(&self) -> f64 {
        f64::from(self.transmitted - self.received) * 100.0 / f64::from(self.transmitted)
    }
}

pub(crate) fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let mut options = options.validate()?;
    let capture = context.capture(output)?;
    options.device = match await_network_ready(&capture, context.target(), NETWORK_READY_TIMEOUT) {
        Ok(address) => address,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    let socket = IcmpSocket::connect(options.device)?;
    let summary = match measure(&socket, options) {
        Ok(summary) => summary,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    context.measurements.record(measurements(options, &summary));
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish()?;
    if let Some(result) = beacon_loss {
        result?;
    }
    let acceptance_failure = if options.count - summary.received > options.maximum_lost {
        Some(format!(
            "ICMP lost {} replies at sequences {:?}, above the configured maximum {}",
            options.count - summary.received,
            summary.lost_sequences,
            options.maximum_lost,
        ))
    } else if options.maximum_p95.is_some_and(|maximum| {
        summary.p95_us > u64::try_from(maximum.as_micros()).unwrap_or(u64::MAX)
    }) {
        Some(format!(
            "ICMP p95 {} us exceeds the configured {} us",
            summary.p95_us,
            options.maximum_p95.expect("checked above").as_micros(),
        ))
    } else {
        None
    };
    fs::create_dir_all(output)?;
    let report_path = output.join("report.md");
    fs::write(
        &report_path,
        report(options, &summary, acceptance_failure.as_deref()),
    )?;
    if acceptance_failure.is_none() {
        eprintln!(
            "OPENRADIOHOST result=PASS mode=icmp transmitted={} received={} loss_percent={:.3} \
             readiness_attempts={} min_us={} avg_us={} p50_us={} p95_us={} p99_us={} max_us={} report={}",
            summary.transmitted,
            summary.received,
            summary.loss_percent(),
            summary.readiness_attempts,
            summary.minimum_us,
            summary.average_us,
            summary.p50_us,
            summary.p95_us,
            summary.p99_us,
            summary.maximum_us,
            report_path.display(),
        );
    }
    match acceptance_failure {
        Some(failure) => Err(format!("{failure}; report={}", report_path.display()).into()),
        None => Ok(()),
    }
}

fn measurements(options: Config, summary: &LatencySummary) -> Vec<Measurement> {
    let lost = u64::from(summary.transmitted - summary.received);
    let minimum_received = u64::from(options.count - options.maximum_lost);
    let loss_basis_points = lost
        .saturating_mul(10_000)
        .checked_div(u64::from(summary.transmitted))
        .unwrap_or(0);
    let mut measurements = vec![
        Measurement::observed(
            "icmp.requests.transmitted",
            u64::from(summary.transmitted),
            MeasurementUnit::Count,
        ),
        Measurement::observed(
            "icmp.replies.received",
            u64::from(summary.received),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::AtLeast, minimum_received),
        Measurement::observed("icmp.replies.lost", lost, MeasurementUnit::Count)
            .evaluated(Comparison::AtMost, u64::from(options.maximum_lost)),
        Measurement::observed("icmp.loss", loss_basis_points, MeasurementUnit::BasisPoints),
        Measurement::observed(
            "icmp.readiness.attempts",
            u64::from(summary.readiness_attempts),
            MeasurementUnit::Count,
        ),
        Measurement::observed(
            "icmp.rtt.minimum",
            summary.minimum_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.average",
            summary.average_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p50",
            summary.p50_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p95",
            summary.p95_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p99",
            summary.p99_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.maximum",
            summary.maximum_us,
            MeasurementUnit::Microseconds,
        ),
    ];
    if let Some(maximum) = options.maximum_p95 {
        let maximum = u64::try_from(maximum.as_micros()).unwrap_or(u64::MAX);
        let p95 = measurements
            .iter_mut()
            .find(|measurement| measurement.name == "icmp.rtt.p95")
            .expect("p95 measurement is present");
        *p95 = p95.clone().evaluated(Comparison::AtMost, maximum);
    }
    measurements
}

/// Called only after successful maintenance and completion of the original RX
/// session, while its capture and station epoch remain live. A fresh socket
/// prevents a previously queued reply from satisfying this new exchange.
pub(crate) fn post_maintenance_echo(device: Ipv4Addr, output: &Path) -> Result<()> {
    let result = fresh_echo(device);
    fs::write(
        output.join("post-maintenance-echo.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema":1, "target":device, "requested_replies":3,
            "after":"completed-maintenance-and-original-rx-session",
            "passed":result.is_ok(), "failure":result.as_ref().err().map(ToString::to_string),
            "scope":"fresh ICMP exchange in the retained station epoch; no UDP rate or RF timing claim"
        }))?,
    )?;
    result
}

/// Three new requests on a newly opened socket; no queued replies are reused.
pub(crate) fn fresh_echo(device: Ipv4Addr) -> Result<()> {
    let socket = IcmpSocket::connect(device)?;
    require_fresh_echoes(|sequence| {
        socket.send_echo(sequence, 32)?;
        socket.wait_for_echo(sequence, Duration::from_secs(2))
    })
}

fn require_fresh_echoes(mut exchange: impl FnMut(u16) -> Result<bool>) -> Result<()> {
    for sequence in 0..3 {
        if !exchange(sequence)? {
            return Err(format!("fresh ICMP echo {sequence} did not return").into());
        }
    }
    Ok(())
}

fn measure(socket: &IcmpSocket, options: Config) -> Result<LatencySummary> {
    let readiness_attempts = wait_until_reachable(socket, options.payload_bytes, options.timeout)?;
    let mut samples = Vec::with_capacity(usize::from(options.count));
    let mut lost_sequences = Vec::new();
    let mut next_send = Instant::now();
    for sequence in 0..options.count {
        let now = Instant::now();
        if now < next_send {
            oer_process::sleep(next_send - now)?;
        }
        let started = Instant::now();
        socket.send_echo(sequence, options.payload_bytes)?;
        if socket.wait_for_echo(sequence, options.timeout)? {
            samples.push(started.elapsed());
        } else {
            lost_sequences.push(sequence);
        }
        next_send = started + options.interval;
    }
    if samples.is_empty() {
        return Err("ICMP qualification received no echo replies".into());
    }
    samples.sort_unstable();
    let total_us = samples.iter().map(Duration::as_micros).sum::<u128>();
    let micros = |duration: Duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
    let percentile = |percent: usize| {
        let index = (samples.len() * percent).div_ceil(100).saturating_sub(1);
        micros(samples[index])
    };
    Ok(LatencySummary {
        transmitted: options.count,
        received: u16::try_from(samples.len()).expect("sample count is bounded by u16"),
        readiness_attempts,
        lost_sequences,
        minimum_us: micros(samples[0]),
        average_us: u64::try_from(total_us / samples.len() as u128).unwrap_or(u64::MAX),
        p50_us: percentile(50),
        p95_us: percentile(95),
        p99_us: percentile(99),
        maximum_us: micros(*samples.last().expect("nonempty samples")),
    })
}

fn wait_until_reachable(
    socket: &IcmpSocket,
    payload_bytes: usize,
    timeout: Duration,
) -> Result<u8> {
    const MAX_ATTEMPTS: u8 = 3;
    for attempt in 1..=MAX_ATTEMPTS {
        let sequence = u16::MAX - u16::from(attempt);
        socket.send_echo(sequence, payload_bytes)?;
        if socket.wait_for_echo(sequence, timeout)? {
            return Ok(attempt);
        }
    }
    Err(
        format!("ICMP target did not become reachable after {MAX_ATTEMPTS} readiness attempts")
            .into(),
    )
}

fn report(options: Config, summary: &LatencySummary, acceptance_failure: Option<&str>) -> String {
    let result = if acceptance_failure.is_some() {
        "FAIL"
    } else {
        "PASS"
    };
    let failure = acceptance_failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    format!(
        "# Open-radio ICMP latency HIL\n\n\
         - Result: `{result}`\n\
         {failure}\
         - Device: `{}`\n\
         - Payload: `{}` bytes\n\
         - Interval/timeout: `{}` / `{}` us\n\
         - Readiness attempts before measurement: `{}`\n\
         - Transmitted/received: `{}` / `{}`\n\
         - Lost measurement sequences: `{:?}`\n\
         - Loss: `{:.3}%`\n\
         - RTT min/avg/p50/p95/p99/max: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` us\n",
        options.device,
        options.payload_bytes,
        options.interval.as_micros(),
        options.timeout.as_micros(),
        summary.readiness_attempts,
        summary.transmitted,
        summary.received,
        summary.lost_sequences,
        summary.loss_percent(),
        summary.minimum_us,
        summary.average_us,
        summary.p50_us,
        summary.p95_us,
        summary.p99_us,
        summary.maximum_us,
    )
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: Ipv4Addr::UNSPECIFIED,
            count: DEFAULT_COUNT,
            interval: DEFAULT_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
            payload_bytes: DEFAULT_PAYLOAD_BYTES,
            maximum_lost: 0,
            maximum_p95: None,
        }
    }
}
impl Config {
    fn validate(self) -> Result<Self> {
        if self.count == 0 {
            return Err("ICMP count must be nonzero".into());
        }
        if self.maximum_lost >= self.count {
            return Err("ICMP maximum lost replies must be below the request count".into());
        }
        if self.interval.is_zero() || self.timeout.is_zero() {
            return Err("ICMP interval and timeout must be nonzero".into());
        }
        if self.payload_bytes > MAX_PAYLOAD_BYTES {
            return Err(format!("ICMP payload must be 0..={MAX_PAYLOAD_BYTES} bytes").into());
        }

        Ok(self)
    }
}

struct IcmpSocket {
    descriptor: OwnedFd,
}

impl IcmpSocket {
    fn connect(device: Ipv4Addr) -> Result<Self> {
        // Linux ping sockets are datagram ICMP endpoints available to groups
        // allowed by `net.ipv4.ping_group_range`; no raw-socket capability is
        // needed for the ordinary `cargo hil` path.
        let descriptor = socket_with(
            AddressFamily::INET,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC,
            Some(ipproto::ICMP),
        )?;
        connect(&descriptor, &SocketAddrV4::new(device, 0))?;
        Ok(Self { descriptor })
    }

    fn send_echo(&self, sequence: u16, payload_bytes: usize) -> Result<()> {
        let mut packet = vec![0_u8; 8 + payload_bytes];
        packet[0] = 8;
        packet[6..8].copy_from_slice(&sequence.to_be_bytes());
        for (index, byte) in packet[8..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_add(sequence as u8);
        }
        let checksum = checksum(&packet);
        packet[2..4].copy_from_slice(&checksum.to_be_bytes());
        let sent = send(&self.descriptor, &packet, SendFlags::empty())?;
        if sent != packet.len() {
            return Err(format!("short ICMP send: {sent}/{}", packet.len()).into());
        }
        Ok(())
    }

    fn wait_for_echo(&self, sequence: u16, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            oer_process::check_cancelled()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let timeout = Timespec::try_from((deadline - now).min(Duration::from_millis(20)))?;
            match poll(
                &mut [PollFd::new(&self.descriptor, PollFlags::IN)],
                Some(&timeout),
            ) {
                Ok(0) | Err(Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
            let mut packet = [0_u8; 1_500];
            let received = match recv(&self.descriptor, &mut packet[..], RecvFlags::empty()) {
                Ok((received, _)) => received,
                Err(Errno::INTR | Errno::WOULDBLOCK) => continue,
                Err(error) => return Err(error.into()),
            };
            let packet = &packet[..received];
            if packet.len() >= 8
                && packet[0] == 0
                && packet[1] == 0
                && packet[6..8] == sequence.to_be_bytes()
            {
                return Ok(true);
            }
        }
    }
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    for chunk in bytes.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn post_maintenance_exchange_requires_every_fresh_reply() {
        for missing in 0..3 {
            let mut calls = Vec::new();
            assert!(
                require_fresh_echoes(|sequence| {
                    calls.push(sequence);
                    Ok(sequence != missing)
                })
                .is_err()
            );
            assert_eq!(calls.last(), Some(&missing));
        }
        assert!(require_fresh_echoes(|_| Err("disconnected".into())).is_err());
        let mut calls = Vec::new();
        require_fresh_echoes(|sequence| {
            calls.push(sequence);
            Ok(true)
        })
        .unwrap();
        assert_eq!(calls, [0, 1, 2]);
    }
}
