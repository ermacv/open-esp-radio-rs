//! Host sender and report writer for production RX-only qualification.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::{
    Result,
    bidirectional::qualify_rx_log,
    traffic_capture::{SerialCapture, await_udp_rx_ready},
};

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 20_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Eq, PartialEq)]
struct Options {
    address: Ipv4Addr,
    port: u16,
    rate_bps: u64,
    duration: Duration,
    payload: usize,
    serial: PathBuf,
    expected_rx_format: u8,
    phy: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct HostTransmission {
    bytes: u64,
    datagrams: u64,
    elapsed: Duration,
}

impl HostTransmission {
    fn throughput_bps(self) -> u64 {
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000_000)
            .checked_div(
                u64::try_from(self.elapsed.as_micros())
                    .unwrap_or(u64::MAX)
                    .max(1),
            )
            .unwrap_or(0)
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
    let output = root.join("target/hil/esp32s31/qualification/open-radio-rx");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start(&options.serial);
    let discovered_address = match await_udp_rx_ready(
        &capture,
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(address) => address,
        Err(error) => {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
    };
    options.address = discovered_address;
    let host = send_paced_udp(&options)?;
    thread::sleep(Duration::from_secs(5));
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    let rx = qualify_rx_log(&log, options.expected_rx_format)?;
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    if host.throughput_bps() < minimum_bps {
        return Err("host failed to offer at least 90% of the requested RX rate".into());
    }
    if rx.throughput_median_kbps < minimum_bps / 1_000 {
        return Err(format!(
            "device RX {} kbit/s is below the acceptance floor",
            rx.throughput_median_kbps,
        )
        .into());
    }
    let minimum_delivery = host.datagrams.saturating_mul(99) / 100;
    if rx.received_datagrams < minimum_delivery {
        return Err(format!(
            "device received only {}/{} host UDP datagrams; required at least {minimum_delivery}",
            rx.received_datagrams, host.datagrams,
        )
        .into());
    }
    let pipeline = rx.pipeline;
    let average_service_us = pipeline.service_us as f64 / pipeline.admitted_frames.max(1) as f64;
    let average_dispatch_us = pipeline.dispatch_us as f64 / pipeline.protocol_frames.max(1) as f64;
    let average_publish_us =
        pipeline.network_publish_us as f64 / pipeline.network_publications.max(1) as f64;
    let average_wait_us =
        pipeline.network_ready_wait_us as f64 / pipeline.network_ready_waits.max(1) as f64;
    let average_irq_service_us =
        pipeline.rx_irq_to_service_us as f64 / pipeline.rx_irq_service_samples.max(1) as f64;
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} RX-only HIL\n\n\
             - Result: `PASS`\n\
             - Device: `{}`\n\
             - Requested/actual host offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             - Device RX median: `{:.3} Mbit/s` across `{}` samples; received UDP datagrams: `{}`\n\
             - Enqueued/software-dropped frames: `{}` / `{}`\n\
             - HE-SU MCS0..11 frame histogram: `{:?}`; other PHY frames: `{}`\n\
             - Hardware BUFFER_FULL/FIFO_OVERFLOW: `0` / `0`\n\n\
             ## RX pipeline\n\n\
             - DMA service calls/frontier/admitted: `{}` / `{}` / `{}`; max frontier/admitted: `{}` / `{}`\n\
             - Frontier service buckets 0 / 1 / 2-3 / 4-7 / 8-15 / 16-31 / 32+: `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
             - RX IRQ posts/wake epochs/coalesced/sampled services/clock-skew rejects: `{}` / `{}` / `{}` / `{}` / `{}`; sampled IRQ-to-service: `{:.2} us` average, `{}` us boot maximum\n\
             - Staged bytes: `{}`; invalid empty/oversize units recycled: `{}` / `{}`; service: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Backpressured services: `{}`; pool/queue credit limited: `{}` / `{}`\n\
             - Protocol frames/data: `{}` / `{}`; dispatch: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network publications/bytes: `{}` / `{}`; copy+publish: `{:.2} us/frame` average, `{}` us boot maximum\n\
             - Network-ready waits: `{}`; `{:.2} us` average, `{}` us boot maximum\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.to_uppercase(),
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            host.bytes,
            host.datagrams,
            rx.throughput_median_kbps as f64 / 1_000.0,
            rx.sample_count,
            rx.received_datagrams,
            rx.enqueued,
            rx.dropped,
            rx.he_mcs_histogram,
            rx.other_phy_frames,
            pipeline.service_calls,
            pipeline.frontier_frames,
            pipeline.admitted_frames,
            pipeline.maximum_frontier,
            pipeline.maximum_admitted,
            pipeline.frontier_zero_services,
            pipeline.frontier_one_services,
            pipeline.frontier_two_three_services,
            pipeline.frontier_four_seven_services,
            pipeline.frontier_eight_fifteen_services,
            pipeline.frontier_sixteen_thirty_one_services,
            pipeline.frontier_thirty_two_plus_services,
            pipeline.rx_irq_posts,
            pipeline.rx_irq_epochs,
            pipeline.rx_irq_coalesced_posts,
            pipeline.rx_irq_service_samples,
            pipeline.rx_irq_clock_skew_samples,
            average_irq_service_us,
            pipeline.rx_irq_to_service_max_us,
            pipeline.staged_bytes,
            pipeline.stage_empty_discards,
            pipeline.stage_too_long_discards,
            average_service_us,
            pipeline.service_max_us,
            pipeline.backpressured_services,
            pipeline.pool_credit_limited_services,
            pipeline.queue_credit_limited_services,
            pipeline.protocol_frames,
            pipeline.protocol_data_frames,
            average_dispatch_us,
            pipeline.dispatch_max_us,
            pipeline.network_publications,
            pipeline.network_published_bytes,
            average_publish_us,
            pipeline.network_publish_max_us,
            pipeline.network_ready_waits,
            average_wait_us,
            pipeline.network_ready_wait_max_us,
        ),
    )?;
    println!(
        "OPENRADIOHOST result=PASS mode={}-rx offered_kbps={} host_kbps={} \
         rx_median_kbps={} enqueued={} dropped=0 report={}",
        options.phy,
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        rx.throughput_median_kbps,
        rx.enqueued,
        output.join("report.md").display(),
    );
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic rx <device-ipv4> [options]\n\
         \n\
         --rate <bps>       paced host-to-device rate (default 20M)\n\
         --seconds <5..300> traffic duration (default 12)\n\
         --payload <64..1472> UDP payload bytes (default 1200)\n\
         --port <port>      device UDP sink (default 4323)\n\
         --serial <path>    diagnostics device (default /dev/ttyACM0)\n\
         --phy <he20|ht40> expected RX vector (default he20)\n\n\
         Flash `cargo hil flash radio` and wait for DHCP first."
    );
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let address = arguments
        .first()
        .ok_or("missing ESP32-S31 IPv4 address")?
        .parse::<Ipv4Addr>()?;
    let mut options = Options {
        address,
        port: DEFAULT_PORT,
        rate_bps: DEFAULT_RATE_BPS,
        duration: DEFAULT_DURATION,
        payload: DEFAULT_PAYLOAD,
        serial: PathBuf::from("/dev/ttyACM0"),
        expected_rx_format: 4,
        phy: "he20",
    };
    let mut index = 1;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("RX option requires a value")?;
        match arguments[index].as_str() {
            "--rate" => options.rate_bps = parse_rate(value)?,
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(5..=300).contains(&seconds) {
                    return Err("--seconds must be in 5..=300".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--payload" => {
                options.payload = value.parse::<usize>()?;
                if !(64..=1_472).contains(&options.payload) {
                    return Err("--payload must be in 64..=1472".into());
                }
            }
            "--port" => options.port = value.parse::<u16>()?,
            "--serial" => options.serial = PathBuf::from(value),
            "--phy" => match value.as_str() {
                "he20" => {
                    options.expected_rx_format = 4;
                    options.phy = "he20";
                }
                "ht40" => {
                    options.expected_rx_format = 2;
                    options.phy = "ht40";
                }
                _ => return Err("--phy must be he20 or ht40".into()),
            },
            other => return Err(format!("unknown RX option `{other}`").into()),
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
    if !(100_000..=500_000_000).contains(&rate) {
        return Err("--rate must be in 100K..=500M".into());
    }
    Ok(rate)
}

fn send_paced_udp(options: &Options) -> Result<HostTransmission> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect(SocketAddrV4::new(options.address, options.port))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut packet = vec![0x5a; options.payload];
    let interval = Duration::from_nanos(
        u64::try_from((options.payload as u128 * 8 * 1_000_000_000) / options.rate_bps as u128)?
            .max(1),
    );
    let started = Instant::now();
    let deadline = started + options.duration;
    let mut next = started;
    let mut bytes = 0_u64;
    let mut datagrams = 0_u64;
    while Instant::now() < deadline {
        let now = Instant::now();
        if now < next {
            thread::sleep(next.duration_since(now));
        } else if now.duration_since(next) > Duration::from_millis(20) {
            next = now;
        }
        packet[..4].copy_from_slice(&i32::try_from(datagrams & i32::MAX as u64)?.to_be_bytes());
        let length = socket.send(&packet)?;
        if length != packet.len() {
            return Err(format!("short UDP send: {length}/{}", packet.len()).into());
        }
        bytes = bytes.saturating_add(length as u64);
        datagrams = datagrams.saturating_add(1);
        next += interval;
    }
    let elapsed = started.elapsed();
    packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
    let _ = socket.send(&packet);
    Ok(HostTransmission {
        bytes,
        datagrams,
        elapsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rx_options_and_rates() {
        assert_eq!(parse_rate("20M").unwrap(), 20_000_000);
        let options = parse_options(&[
            "192.168.178.141".into(),
            "--rate".into(),
            "40M".into(),
            "--phy".into(),
            "ht40".into(),
        ])
        .unwrap();
        assert_eq!(options.rate_bps, 40_000_000);
        assert_eq!(options.expected_rx_format, 2);
    }
}
