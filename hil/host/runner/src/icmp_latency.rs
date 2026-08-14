//! ICMP latency and loss qualification for an already connected target.

use std::{
    fs, io,
    net::Ipv4Addr,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    time::{Duration, Instant},
};

use crate::Result;
use crate::{
    lab_config::LabConfig,
    traffic_capture::{SerialCapture, await_network_ready},
};

const DEFAULT_COUNT: u16 = 100;
const DEFAULT_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_PAYLOAD_BYTES: usize = 56;
const MAX_PAYLOAD_BYTES: usize = 1_400;
const NETWORK_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Options {
    device: Ipv4Addr,
    count: u16,
    interval: Duration,
    timeout: Duration,
    payload_bytes: usize,
    maximum_lost: u16,
    maximum_p95: Option<Duration>,
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
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let mut options = parse_options(&arguments)?;
    let capture = SerialCapture::start_with_reset(&lab.device.serial);
    options.device = match await_network_ready(&capture, lab, NETWORK_READY_TIMEOUT) {
        Ok(address) => address,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    let socket = IcmpSocket::connect(options.device)?;
    let summary = match measure(&socket, options) {
        Ok(summary) => summary,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish_to(output)?;
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
    if let Some(failure) = acceptance_failure {
        return Err(format!("{failure}; report={}", report_path.display()).into());
    }
    println!(
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
    Ok(())
}

fn measure(socket: &IcmpSocket, options: Options) -> Result<LatencySummary> {
    let readiness_attempts = wait_until_reachable(socket, options.payload_bytes, options.timeout)?;
    let mut samples = Vec::with_capacity(usize::from(options.count));
    let mut lost_sequences = Vec::new();
    let mut next_send = Instant::now();
    for sequence in 0..options.count {
        let now = Instant::now();
        if now < next_send {
            std::thread::sleep(next_send - now);
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

fn report(options: Options, summary: &LatencySummary, acceptance_failure: Option<&str>) -> String {
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

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut arguments = arguments.iter();
    let mut options = Options {
        device: Ipv4Addr::UNSPECIFIED,
        count: DEFAULT_COUNT,
        interval: DEFAULT_INTERVAL,
        timeout: DEFAULT_TIMEOUT,
        payload_bytes: DEFAULT_PAYLOAD_BYTES,
        maximum_lost: 0,
        maximum_p95: None,
    };
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value for `{argument}`"))?;
        match argument.as_str() {
            "--count" => options.count = value.parse()?,
            "--interval-ms" => options.interval = Duration::from_millis(value.parse()?),
            "--timeout-ms" => options.timeout = Duration::from_millis(value.parse()?),
            "--payload" => options.payload_bytes = value.parse()?,
            "--max-lost" => options.maximum_lost = value.parse()?,
            "--max-p95-ms" => options.maximum_p95 = Some(Duration::from_millis(value.parse()?)),
            _ => return Err(format!("unknown ICMP option `{argument}`").into()),
        }
    }
    if options.count == 0 {
        return Err("ICMP count must be nonzero".into());
    }
    if options.maximum_lost >= options.count {
        return Err("ICMP maximum lost replies must be below the request count".into());
    }
    if options.interval.is_zero() || options.timeout.is_zero() {
        return Err("ICMP interval and timeout must be nonzero".into());
    }
    if options.payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(format!("ICMP payload must be 0..={MAX_PAYLOAD_BYTES} bytes").into());
    }
    Ok(options)
}

struct IcmpSocket {
    descriptor: OwnedFd,
}

impl IcmpSocket {
    fn connect(device: Ipv4Addr) -> Result<Self> {
        // Linux ping sockets are datagram ICMP endpoints available to groups
        // allowed by `net.ipv4.ping_group_range`; no raw-socket capability is
        // needed for the ordinary `cargo hil` path.
        // SAFETY: `socket` has no pointer arguments. A nonnegative result is a
        // newly owned descriptor transferred exactly once into `OwnedFd`.
        let descriptor = unsafe {
            libc::socket(
                libc::AF_INET,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                libc::IPPROTO_ICMP,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: the successful `socket` call returned this fresh descriptor.
        let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
        let address = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(device.octets()),
            },
            sin_zero: [0; 8],
        };
        // SAFETY: `address` is initialized for AF_INET and both it and the
        // owned descriptor remain live for the complete call.
        let result = unsafe {
            libc::connect(
                descriptor.as_raw_fd(),
                (&raw const address).cast(),
                size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error().into());
        }
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
        // SAFETY: `packet` is a live byte slice and the connected descriptor
        // remains owned by `self` for the duration of `send`.
        let sent = unsafe {
            libc::send(
                self.descriptor.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                0,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if sent as usize != packet.len() {
            return Err(format!("short ICMP send: {sent}/{}", packet.len()).into());
        }
        Ok(())
    }

    fn wait_for_echo(&self, sequence: u16, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let remaining_ms =
                i32::try_from((deadline - now).as_millis().max(1)).unwrap_or(i32::MAX);
            let mut descriptor = libc::pollfd {
                fd: self.descriptor.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `descriptor` is one initialized pollfd and remains live
            // and writable for the complete call.
            let ready = unsafe { libc::poll(&raw mut descriptor, 1, remaining_ms) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            if ready == 0 {
                return Ok(false);
            }
            let mut packet = [0_u8; 1_500];
            // SAFETY: `packet` is a live writable byte array and the owned
            // descriptor remains open for the complete call.
            let received = unsafe {
                libc::recv(
                    self.descriptor.as_raw_fd(),
                    packet.as_mut_ptr().cast(),
                    packet.len(),
                    0,
                )
            };
            if received < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted
                    || error.kind() == io::ErrorKind::WouldBlock
                {
                    continue;
                }
                return Err(error.into());
            }
            let packet = &packet[..received as usize];
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
mod tests {
    use super::{checksum, parse_options};

    #[test]
    fn checksum_matches_an_even_and_odd_reference_packet() {
        assert_eq!(checksum(&[8, 0, 0, 0, 0, 0, 0, 1]), 0xf7fe);
        assert_eq!(checksum(&[1, 2, 3]), 0xfbfd);
    }

    #[test]
    fn parses_bounded_latency_options() {
        let arguments = [
            "--count",
            "20",
            "--interval-ms",
            "5",
            "--timeout-ms",
            "100",
            "--payload",
            "32",
        ]
        .map(String::from);
        let options = parse_options(&arguments).unwrap();
        assert_eq!(options.count, 20);
        assert_eq!(options.payload_bytes, 32);
    }
}
