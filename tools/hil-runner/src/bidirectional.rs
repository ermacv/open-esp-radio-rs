//! Host side of the simultaneous RX/TX qualification cell.
//!
//! The firmware's `bidirectional` image owns a synthetic A-MPDU uplink while
//! this runner offers a paced UDP downlink.  Qualification is deliberately
//! based on device-side RX, TX-vector, placement and DMA-health evidence; a
//! successful host `send` alone is not evidence that the radio received it.

use std::{
    fs,
    io::Read,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::Result;

const DEFAULT_PORT: u16 = 4_323;
const DEFAULT_RATE_BPS: u64 = 10_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_PAYLOAD: usize = 1_200;
const MIN_QUALIFIED_SAMPLE: Duration = Duration::from_secs(4);
const PSRAM_CODE_START: u64 = 0x5000_0000;
const PSRAM_CODE_END: u64 = 0x5100_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phy {
    Ht40,
    He20,
}

impl Phy {
    const fn name(self) -> &'static str {
        match self {
            Self::Ht40 => "ht40",
            Self::He20 => "he20",
        }
    }

    const fn expected_tx(self) -> (u16, u64) {
        match self {
            Self::Ht40 => (40, 150_000),
            Self::He20 => (20, 114_700),
        }
    }

    const fn expected_rx_format(self) -> u8 {
        match self {
            Self::Ht40 => 2,
            Self::He20 => 4,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Options {
    address: Ipv4Addr,
    port: u16,
    rate_bps: u64,
    duration: Duration,
    payload: usize,
    serial: PathBuf,
    phy: Phy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ThroughputSample {
    datagrams: u64,
    elapsed_us: u64,
    throughput_kbps: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TxSample {
    throughput_kbps: u64,
    bandwidth_mhz: u16,
    rate_kbps: u64,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct DeviceReport {
    rx: Vec<ThroughputSample>,
    tx: Vec<TxSample>,
    rx_formats: Vec<u8>,
    dma_health: Vec<(u64, u64)>,
    code_addresses: Vec<u64>,
    failures: Vec<String>,
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
    let options = parse_options(&arguments)?;
    let capture = SerialCapture::start(&options.serial);
    thread::sleep(Duration::from_secs(1));
    let host = send_paced_udp(&options)?;
    // The direct RX sample closes on the terminal datagram. Leave time for
    // the ten-second DMA-health interval and the bounded USB logger backlog.
    thread::sleep(Duration::from_secs(5));
    let log = capture.finish();
    let report = parse_device_report(&log);
    let output = root.join("target/hil/esp32s31/qualification/open-radio-bidirectional");
    fs::create_dir_all(&output)?;
    fs::write(output.join("uart.log"), &log)?;
    qualify(&options, host, &report)?;

    let rx_median = median(
        qualified_rx_samples(&report)
            .iter()
            .map(|sample| sample.throughput_kbps)
            .collect(),
    )
    .ok_or("missing qualified direct-RX sample")?;
    let tx_floor = report
        .tx
        .iter()
        .map(|sample| sample.throughput_kbps)
        .min()
        .ok_or("missing concurrent TX sample")?;
    write_report(&output, &options, host, rx_median, tx_floor)?;
    println!(
        "OPENRADIOHOST result=PASS mode={}-bidirectional offered_kbps={} \
         host_kbps={} rx_median_kbps={rx_median} concurrent_tx_floor_kbps={tx_floor} \
         combined_floor_sum_kbps={} report={}",
        options.phy.name(),
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        rx_median.saturating_add(tx_floor),
        output.join("report.md").display(),
    );
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic bidirectional <ipv4> [options]\n\
         \n\
         --rate <bps>       paced host-to-device rate (default 10M)\n\
         --seconds <5..300> traffic duration (default 12)\n\
         --payload <64..1472> UDP payload bytes (default 1200)\n\
         --port <port>      device UDP sink (default 4323)\n\
         --serial <path>    diagnostics device (default /dev/ttyACM0)\n\
         --phy <ht40|he20> expected negotiated PHY (default he20)\n\n\
         Flash `cargo hil flash bidirectional` and wait for DHCP first."
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
        phy: Phy::He20,
    };
    let mut index = 1;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("bidirectional option requires a value")?;
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
            "--phy" => {
                options.phy = match value.as_str() {
                    "ht40" => Phy::Ht40,
                    "he20" => Phy::He20,
                    _ => return Err("--phy must be ht40 or he20".into()),
                };
            }
            other => return Err(format!("unknown bidirectional option `{other}`").into()),
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

fn parse_device_report(log: &str) -> DeviceReport {
    let mut report = DeviceReport::default();
    for line in log.lines() {
        if line.starts_with("ORX ") || line.contains(" ORX ") {
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) =
                (field(line, "d"), field(line, "u"), field(line, "k"))
            {
                report.rx.push(ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                });
            }
        } else if line.starts_with("OTX ") || line.contains(" OTX ") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) =
                (field(line, "k"), field(line, "w"), field(line, "r"))
            {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
                if let Some(address) = field(line, "a") {
                    report.code_addresses.push(address);
                }
            }
        } else if line.starts_with("ORXP ") || line.contains(" ORXP ") {
            if let Some(format) = field(line, "f") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("result=BENCH") && line.contains("stage=udp-rx-direct") {
            if let (Some(datagrams), Some(elapsed_us), Some(throughput_kbps)) = (
                field(line, "datagrams"),
                field(line, "elapsed_us"),
                field(line, "throughput_kbps"),
            ) {
                report.rx.push(ThroughputSample {
                    datagrams,
                    elapsed_us,
                    throughput_kbps,
                });
            }
        } else if line.contains("result=BENCH") && line.contains("stage=raw-mac-tx") {
            if let (Some(throughput_kbps), Some(bandwidth_mhz), Some(rate_kbps)) = (
                field(line, "throughput_kbps"),
                field(line, "bandwidth_mhz"),
                field(line, "rate_kbps"),
            ) {
                report.tx.push(TxSample {
                    throughput_kbps,
                    bandwidth_mhz: bandwidth_mhz as u16,
                    rate_kbps,
                });
            }
        } else if line.contains("stage=udp-rx-phy") {
            if let Some(format) = field(line, "rx_format") {
                report.rx_formats.push(format as u8);
            }
        } else if line.contains("stage=rx-runtime-delta") {
            if let (Some(buffer_full), Some(fifo_overflow)) =
                (field(line, "buffer_full"), field(line, "fifo_overflow"))
            {
                report.dma_health.push((buffer_full, fifo_overflow));
            }
        } else if line.contains("stage=tx-runtime") {
            if let Some(address) = field(line, "code_address") {
                report.code_addresses.push(address);
            }
        }
        if line.contains("result=FAIL")
            && (line.contains("raw-mac")
                || line.contains("embassy-net-radio")
                || line.contains("mic-failure"))
        {
            report.failures.push(line.to_owned());
        }
    }
    report
}

fn field(line: &str, key: &str) -> Option<u64> {
    line.split_whitespace().find_map(|token| {
        let (candidate, value) = token.split_once('=')?;
        (candidate == key).then(|| value.trim_end_matches(',').parse::<u64>().ok())?
    })
}

fn qualified_rx_samples(report: &DeviceReport) -> Vec<ThroughputSample> {
    report
        .rx
        .iter()
        .copied()
        .filter(|sample| {
            sample.elapsed_us >= MIN_QUALIFIED_SAMPLE.as_micros() as u64
                && sample.throughput_kbps > 0
                && sample.datagrams >= 16
        })
        .collect()
}

fn qualify(options: &Options, host: HostTransmission, report: &DeviceReport) -> Result<()> {
    if report.code_addresses.is_empty()
        || report
            .code_addresses
            .iter()
            .any(|address| !(PSRAM_CODE_START..PSRAM_CODE_END).contains(address))
    {
        return Err("missing psram-code runtime marker".into());
    }
    if report.dma_health.is_empty() {
        return Err("missing RX DMA-health interval".into());
    }
    if let Some((full, overflow)) = report
        .dma_health
        .iter()
        .find(|(full, overflow)| *full != 0 || *overflow != 0)
    {
        return Err(
            format!("RX DMA starvation: buffer_full={full} fifo_overflow={overflow}").into(),
        );
    }
    let rx = qualified_rx_samples(report);
    if rx.is_empty() {
        return Err("missing complete device-side direct-RX sample".into());
    }
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    if host.throughput_bps() < minimum_bps {
        return Err("host failed to offer at least 90% of the requested rate".into());
    }
    let rx_median = median(rx.iter().map(|sample| sample.throughput_kbps).collect())
        .expect("nonempty direct-RX samples");
    if rx_median < minimum_bps / 1_000 {
        return Err(format!("device RX {rx_median} kbit/s is below the acceptance floor").into());
    }
    let (expected_width, expected_rate) = options.phy.expected_tx();
    if report.tx.is_empty()
        || report.tx.iter().any(|sample| {
            sample.bandwidth_mhz != expected_width || sample.rate_kbps != expected_rate
        })
    {
        return Err(format!(
            "concurrent TX did not remain at {} / {expected_rate} kbit/s",
            options.phy.name()
        )
        .into());
    }
    if report.rx_formats.is_empty()
        || report
            .rx_formats
            .iter()
            .any(|format| *format != options.phy.expected_rx_format())
    {
        return Err(format!(
            "RX did not remain in the {} baseband format",
            options.phy.name()
        )
        .into());
    }
    if let Some(failure) = report.failures.first() {
        return Err(format!("device reported a data-path failure: {failure}").into());
    }
    Ok(())
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some((values.get(middle - 1)? + values.get(middle)?) / 2)
    } else {
        values.get(middle).copied()
    }
}

fn write_report(
    output: &Path,
    options: &Options,
    host: HostTransmission,
    rx_median: u64,
    tx_floor: u64,
) -> Result<()> {
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio {} bidirectional HIL\n\n\
             - Result: `PASS`\n\
             - Device: `{}`\n\
             - Requested downlink: `{:.3} Mbit/s`\n\
             - Actual host offer: `{:.3} Mbit/s`\n\
             - Host payload: `{}` bytes in `{}` datagrams\n\
             - Direct RX median: `{:.3} Mbit/s`\n\
             - Concurrent open-radio TX floor: `{:.3} Mbit/s`\n\
             - Combined conservative floor: `{:.3} Mbit/s`\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.phy.name().to_uppercase(),
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            host.bytes,
            host.datagrams,
            rx_median as f64 / 1_000.0,
            tx_floor as f64 / 1_000.0,
            rx_median.saturating_add(tx_floor) as f64 / 1_000.0,
        ),
    )?;
    Ok(())
}

struct SerialCapture {
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<String>,
}

impl SerialCapture {
    fn start(port: &Path) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let port = port.to_owned();
        let worker = thread::spawn(move || {
            let mut bytes = Vec::new();
            let mut serial = match serialport::new(port.to_string_lossy(), 115_200)
                .timeout(Duration::from_millis(100))
                .open()
            {
                Ok(serial) => serial,
                Err(error) => {
                    return format!("serial capture failed for {}: {error}\n", port.display())
                }
            };
            let mut buffer = [0_u8; 2_048];
            while !worker_stop.load(Ordering::Acquire) {
                match serial.read(&mut buffer) {
                    Ok(length) => bytes.extend_from_slice(&buffer[..length]),
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(error) => {
                        bytes.extend_from_slice(
                            format!("\nserial read failed: {error}\n").as_bytes(),
                        );
                        break;
                    }
                }
            }
            String::from_utf8_lossy(&bytes).into_owned()
        });
        Self { stop, worker }
    }

    fn finish(self) -> String {
        self.stop.store(true, Ordering::Release);
        self.worker
            .join()
            .unwrap_or_else(|_| "serial capture thread panicked\n".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rates_and_options() {
        assert_eq!(parse_rate("10M").unwrap(), 10_000_000);
        assert_eq!(parse_rate("2500K").unwrap(), 2_500_000);
        let options = parse_options(&[
            "192.168.178.141".into(),
            "--phy".into(),
            "he20".into(),
            "--seconds".into(),
            "5".into(),
        ])
        .unwrap();
        assert_eq!(options.phy, Phy::He20);
        assert_eq!(options.duration, Duration::from_secs(5));
    }

    #[test]
    fn qualifies_complete_he20_evidence() {
        let report = parse_device_report(
            "OTX b=50000000 d=1 u=5000000 k=80000 e=0 w=20 r=114700 g=1 x=0 l=1 a=1342257664\n\
             ORX b=6000000 d=5000 u=5000000 k=9600\n\
             ORXP f=4 r=11 m=11\n\
             OPEN_RADIO_PHY_HIL stage=rx-runtime-delta buffer_full=0 fifo_overflow=0\n",
        );
        let options = parse_options(&["192.168.178.141".into()]).unwrap();
        let host = HostTransmission {
            bytes: 6_250_000,
            datagrams: 5_208,
            elapsed: Duration::from_secs(5),
        };
        qualify(&options, host, &report).unwrap();
    }
}
