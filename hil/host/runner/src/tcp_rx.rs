//! Host-to-target TCP stream qualification over the typed HIL lifecycle.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use open_esp_radio_hil_protocol::{Completion, Direction, FlowConfig, SessionConfig, Transport};

use crate::{
    Result,
    paced_tcp::{Config as PacedTcpConfig, HostTransmission, send as send_paced_tcp},
    traffic_capture::{SerialCapture, SessionEvidence, await_tcp_rx_ready},
};

const DEFAULT_PORT: u16 = 4_325;
const DEFAULT_RATE_BPS: u64 = 20_000_000;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_CHUNK_BYTES: usize = 14_600;
const MAXIMUM_CHUNK_BYTES: usize = 32_768;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Eq, PartialEq)]
struct Options {
    address: Ipv4Addr,
    port: u16,
    rate_bps: u64,
    duration: Duration,
    chunk_bytes: usize,
    serial: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetSample {
    bytes: u64,
    streams: u64,
    elapsed_us: u64,
    throughput_kbps: u64,
    errors: u64,
    buffer_full: u64,
    fifo_overflow: u64,
    enqueued: u64,
    dropped: u64,
    eof: bool,
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
    let output = root.join("target/hil/esp32s31/qualification/open-radio-tcp-rx");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let ready = match await_tcp_rx_ready(
        &capture,
        options.address,
        options.port,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(ready) => ready,
        Err(error) => {
            let log = capture.finish();
            fs::write(output.join("uart.log"), &log)?;
            return Err(error);
        }
    };
    if !ready.runtime_session {
        return Err("TCP RX firmware did not advertise a runtime session".into());
    }
    options.address = ready.address;
    let session = capture.start_session(SessionConfig {
        transport: Transport::Tcp,
        direction: Direction::Rx,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        peer: None,
        target_rx: Some(FlowConfig {
            payload_bytes: u16::try_from(options.chunk_bytes)?,
            offered_rate_bps: Some(options.rate_bps),
        }),
        target_tx: None,
    })?;
    let host_result = send_paced_tcp(PacedTcpConfig {
        address: options.address,
        port: options.port,
        rate_bps: options.rate_bps,
        duration: options.duration,
        chunk_bytes: options.chunk_bytes,
    });
    let structured_result = capture.wait_for_session(
        session,
        options.duration.saturating_add(Duration::from_secs(10)),
    );
    let host = host_result?;
    let structured = structured_result?;
    capture.acknowledge_session(session)?;
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    let target = parse_target_sample(&log).ok_or("missing compact TCP RX target evidence")?;
    let failure = validate(&options, host, structured, target)
        .err()
        .map(|error| error.to_string());
    write_report(
        &output,
        &options,
        host,
        structured,
        target,
        failure.as_deref(),
    )?;
    if let Some(failure) = failure {
        return Err(failure.into());
    }
    println!(
        "OPENRADIOHOST result=PASS mode=tcp-rx offered_kbps={} host_kbps={} \
         target_kbps={} bytes={} streams=1 buffer_full=0 fifo_overflow=0 dropped=0 report={}",
        options.rate_bps / 1_000,
        host.throughput_bps() / 1_000,
        target.throughput_kbps,
        target.bytes,
        output.join("report.md").display(),
    );
    Ok(())
}

fn validate(
    options: &Options,
    host: HostTransmission,
    structured: SessionEvidence,
    target: TargetSample,
) -> Result<()> {
    let minimum_bps = options.rate_bps.saturating_mul(9) / 10;
    if host.throughput_bps() < minimum_bps {
        return Err("host failed to offer at least 90% of the requested TCP rate".into());
    }
    if target.throughput_kbps < minimum_bps / 1_000 {
        return Err(format!(
            "target TCP RX {} kbit/s is below the acceptance floor",
            target.throughput_kbps
        )
        .into());
    }
    if !structured.finished.summary.passed || structured.transport.transport_errors != 0 {
        return Err(format!(
            "target did not complete TCP RX cleanly: passed={} errors={}",
            structured.finished.summary.passed, structured.transport.transport_errors
        )
        .into());
    }
    if structured.transport.tx_bytes != 0 || structured.transport.tx_units != 0 {
        return Err("TCP RX-only session reported transmitted traffic".into());
    }
    if structured.transport.rx_units != 1 || target.streams != 1 || !target.eof {
        return Err(format!(
            "TCP stream did not end with one EOF-completed connection: typed={} text={} eof={}",
            structured.transport.rx_units, target.streams, target.eof
        )
        .into());
    }
    if host.bytes != structured.transport.rx_bytes || host.bytes != target.bytes {
        return Err(format!(
            "host/typed/text TCP byte mismatch: host={} typed={} text={}",
            host.bytes, structured.transport.rx_bytes, target.bytes
        )
        .into());
    }
    let typed_throughput_kbps = structured
        .transport
        .rx_bytes
        .saturating_mul(8)
        .saturating_mul(1_000)
        .checked_div(structured.transport.elapsed_micros.max(1))
        .unwrap_or(0);
    if typed_throughput_kbps != target.throughput_kbps
        || structured.transport.elapsed_micros != target.elapsed_us
    {
        return Err(format!(
            "typed/text TCP timing mismatch: typed={typed_throughput_kbps}/{} text={}/{}",
            structured.transport.elapsed_micros, target.throughput_kbps, target.elapsed_us
        )
        .into());
    }
    if target.errors != 0
        || target.buffer_full != 0
        || target.fifo_overflow != 0
        || target.dropped != 0
    {
        return Err(format!(
            "TCP RX health failure: errors={} buffer_full={} fifo_overflow={} dropped={}",
            target.errors, target.buffer_full, target.fifo_overflow, target.dropped
        )
        .into());
    }
    Ok(())
}

fn parse_target_sample(log: &str) -> Option<TargetSample> {
    log.lines().rev().find_map(|line| {
        if !line.contains("OTCPRX b=") {
            return None;
        }
        Some(TargetSample {
            bytes: field(line, "b")?,
            streams: field(line, "s")?,
            elapsed_us: field(line, "u")?,
            throughput_kbps: field(line, "k")?,
            errors: field(line, "e")?,
            buffer_full: field(line, "bf")?,
            fifo_overflow: field(line, "fo")?,
            enqueued: field(line, "enq")?,
            dropped: field(line, "drop")?,
            eof: field::<u8>(line, "eof")? != 0,
        })
    })
}

fn field<T: core::str::FromStr>(line: &str, name: &str) -> Option<T> {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(&format!("{name}=")))?
        .parse()
        .ok()
}

fn write_report(
    output: &Path,
    options: &Options,
    host: HostTransmission,
    structured: SessionEvidence,
    target: TargetSample,
    failure: Option<&str>,
) -> Result<()> {
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure_report = failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TCP RX-only HIL\n\n\
             - Result: `{result}`\n\
             {failure_report}\
             - Device: `{}`\n\
             - Requested/actual host offer: `{:.3}` / `{:.3} Mbit/s`\n\
             - Application chunk: `{}` bytes; host writes: `{}`\n\
             - Host/typed/text bytes: `{}` / `{}` / `{}`\n\
             - Completed streams / EOF: `{}` / `{}`\n\
             - Target throughput: `{:.3} Mbit/s` in `{}` us\n\
             - Host pacing maximum lateness/catch-up/deadline resets: `{} us` / `{}` writes / `{}`\n\
             - RX enqueued/software-dropped frames: `{}` / `{}`\n\
             - Hardware BUFFER_FULL/FIFO_OVERFLOW: `{}` / `{}`\n\
             - Typed evidence CRC32C: `0x{:08x}`\n\n\
             `rx_units` is one EOF-completed TCP stream; byte equality, not host write count, is the stream delivery proof.\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            options.address,
            options.rate_bps as f64 / 1_000_000.0,
            host.throughput_bps() as f64 / 1_000_000.0,
            options.chunk_bytes,
            host.writes,
            host.bytes,
            structured.transport.rx_bytes,
            target.bytes,
            target.streams,
            target.eof,
            target.throughput_kbps as f64 / 1_000.0,
            target.elapsed_us,
            host.maximum_lateness_us(),
            host.maximum_catch_up_writes,
            host.deadline_resets,
            target.enqueued,
            target.dropped,
            target.buffer_full,
            target.fifo_overflow,
            structured.finished.evidence_crc32c,
        ),
    )?;
    Ok(())
}

fn print_help() {
    println!(
        "cargo hil traffic tcp-rx <device-ipv4> [options]\n\
         \n\
         --rate <bps>       paced TCP application rate (default 20M)\n\
         --seconds <5..300> stream duration (default 12)\n\
         --chunk <64..32768> application write size (default 14600)\n\
         --port <port>      device TCP listener (default 4325)\n\
         --serial <path>    diagnostics device (default /dev/ttyACM0)\n\n\
         Flash `cargo hil flash tcp-rx` first."
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
        chunk_bytes: DEFAULT_CHUNK_BYTES,
        serial: PathBuf::from("/dev/ttyACM0"),
    };
    let mut index = 1;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("TCP RX option requires a value")?;
        match arguments[index].as_str() {
            "--rate" => options.rate_bps = parse_rate(value)?,
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(5..=300).contains(&seconds) {
                    return Err("--seconds must be in 5..=300".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--chunk" => {
                options.chunk_bytes = value.parse::<usize>()?;
                if !(64..=MAXIMUM_CHUNK_BYTES).contains(&options.chunk_bytes) {
                    return Err("--chunk must be in 64..=32768".into());
                }
            }
            "--port" => options.port = value.parse::<u16>()?,
            "--serial" => options.serial = PathBuf::from(value),
            other => return Err(format!("unknown TCP RX option `{other}`").into()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tcp_rx_options_and_target_evidence() {
        let options = parse_options(&[
            "192.168.178.141".into(),
            "--seconds".into(),
            "8".into(),
            "--chunk".into(),
            "1200".into(),
            "--rate".into(),
            "40M".into(),
        ])
        .unwrap();
        assert_eq!(options.duration, Duration::from_secs(8));
        assert_eq!(options.chunk_bytes, 1_200);
        assert_eq!(options.rate_bps, 40_000_000);

        let sample = parse_target_sample(
            "OTCPRX b=40000000 s=1 u=8000000 k=40000 e=0 bf=0 fo=0 enq=10 drop=0 eof=1\n",
        )
        .unwrap();
        assert_eq!(sample.bytes, 40_000_000);
        assert_eq!(sample.streams, 1);
        assert!(sample.eof);
    }
}
