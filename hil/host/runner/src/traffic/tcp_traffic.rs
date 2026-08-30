//! Typed TCP RX, TX and full-duplex qualification over one runtime image.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, SessionConfig, SessionFlowConfig, SessionLinkRequirements,
    Transport,
};

use crate::{
    Result,
    evidence::traffic_capture::{SerialCapture, SessionEvidence, await_tcp_ready},
    traffic::host_network::reject_overlapping_ipv4_links,
    traffic::paced_tcp::{
        Config as PacedTcpConfig, HostReception, HostTransmission, exchange, receive, send,
    },
    transport::lab_config::LabConfig,
};

const DEFAULT_PORT: u16 = 4_325;
const DEFAULT_DURATION: Duration = Duration::from_secs(12);
const DEFAULT_CHUNK_BYTES: usize = 32_768;
const DEFAULT_RX_RATE_BPS: u64 = 20_000_000;
const DEFAULT_TX_RATE_BPS: u64 = 60_000_000;
const DEFAULT_BIDIRECTIONAL_RX_RATE_BPS: u64 = 10_000_000;
const DEFAULT_BIDIRECTIONAL_TX_RATE_BPS: u64 = 45_000_000;
const DEFAULT_TX_FLOOR_BPS: u64 = 45_000_000;
const DEFAULT_BIDIRECTIONAL_TX_FLOOR_BPS: u64 = 35_000_000;
const MAXIMUM_CHUNK_BYTES: usize = 32_768;
const DEVICE_READY_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug)]
struct Options {
    device: Ipv4Addr,
    direction: Direction,
    port: u16,
    duration: Duration,
    chunk_bytes: usize,
    rx_rate_bps: Option<u64>,
    rx_floor_bps: Option<u64>,
    tx_rate_bps: Option<u64>,
    tx_floor_bps: Option<u64>,
    serial: PathBuf,
}

pub(crate) fn run_rx(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    require_no_beacon_loss: bool,
) -> Result<()> {
    run(
        arguments,
        output,
        lab,
        Direction::Rx,
        require_no_beacon_loss,
    )
}

pub(crate) fn run_tx(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    require_no_beacon_loss: bool,
) -> Result<()> {
    run(
        arguments,
        output,
        lab,
        Direction::Tx,
        require_no_beacon_loss,
    )
}

pub(crate) fn run_bidirectional(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    require_no_beacon_loss: bool,
) -> Result<()> {
    run(
        arguments,
        output,
        lab,
        Direction::Bidirectional,
        require_no_beacon_loss,
    )
}

fn run(
    arguments: Vec<String>,
    output: &Path,
    lab: &LabConfig,
    direction: Direction,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let mut options = parse_options(&arguments, lab, direction)?;
    let mode = direction_name(direction);
    fs::create_dir_all(output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let ready = match await_tcp_ready(
        &capture,
        lab,
        options.device,
        options.port,
        direction,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(ready) => ready,
        Err(error) => {
            capture.finish_to(output)?;
            return Err(error);
        }
    };
    options.device = ready.address;
    reject_overlapping_ipv4_links(options.device)?;
    let session = capture.start_session(SessionConfig {
        network_interface: open_esp_radio_hil_protocol::WifiNetworkInterface::Station,
        transport: Transport::Tcp,
        direction,
        completion: Completion::DurationMillis(u32::try_from(options.duration.as_millis())?),
        flows: [
            Some(SessionFlowConfig {
                flow_id: 0,
                // The target publishes one TCP service and accepts in every
                // payload direction. `peer` remains a UDP datagram
                // destination, not a hidden request for the target to reverse
                // the TCP connection role.
                peer: None,
                target_rx: options.rx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(options.chunk_bytes).expect("validated TCP chunk"),
                    offered_rate_bps: Some(rate),
                }),
                target_tx: options.tx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(options.chunk_bytes).expect("validated TCP chunk"),
                    offered_rate_bps: Some(rate),
                }),
            }),
            None,
        ],
        link_requirements: if matches!(direction, Direction::Tx | Direction::Bidirectional) {
            SessionLinkRequirements::tx_block_ack(0)
        } else {
            SessionLinkRequirements::NONE
        },
    })?;

    let config = PacedTcpConfig {
        address: options.device,
        port: options.port,
        rate_bps: options.rx_rate_bps.unwrap_or(DEFAULT_RX_RATE_BPS),
        duration: options.duration,
        chunk_bytes: options.chunk_bytes,
    };
    let timeout = options.duration + Duration::from_secs(8);
    let data_plane = match direction {
        Direction::Rx => send(config).map(|sample| (Some(sample), None)),
        Direction::Tx => receive(config).map(|sample| (None, Some(sample))),
        Direction::Bidirectional => exchange(config).map(|(tx, rx)| (Some(tx), Some(rx))),
    };

    // Always collect the target's typed result after a data-plane failure.
    // Otherwise a reset/timeout hides precisely the evidence needed to
    // distinguish a host socket problem from a target transport failure.
    let structured = capture.wait_for_session(session, timeout);
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish_to(output)?;
    if let Some(result) = beacon_loss {
        result?;
    }
    let data_plane = data_plane.map_err(|error| format!("TCP data plane failed: {error}"));
    let structured = structured.map_err(|error| format!("TCP target evidence failed: {error}"));
    let acknowledgement =
        acknowledgement.map_err(|error| format!("TCP result acknowledgement failed: {error}"));
    let (host_tx, host_rx) = data_plane?;
    let structured = structured?;
    acknowledgement?;
    let failure = validate(&options, host_tx, host_rx, structured)
        .err()
        .map(|error| error.to_string());
    write_report(
        output,
        &options,
        host_tx,
        host_rx,
        structured,
        failure.as_deref(),
    )?;
    if let Some(failure) = failure {
        return Err(failure.into());
    }
    eprintln!(
        "OPENRADIOHOST result=PASS mode=tcp-{mode} rx_kbps={} tx_kbps={} rx_bytes={} tx_bytes={} pattern_errors=0 report={}",
        host_tx.map_or(0, |sample| sample.throughput_bps() / 1_000),
        host_rx.map_or(0, |sample| sample.throughput_bps() / 1_000),
        structured.transport.rx_bytes,
        structured.transport.tx_bytes,
        output.join("report.md").display(),
    );
    Ok(())
}

fn validate(
    options: &Options,
    host_tx: Option<HostTransmission>,
    host_rx: Option<HostReception>,
    structured: SessionEvidence,
) -> Result<()> {
    if !structured.finished.summary.passed || structured.transport.transport_errors != 0 {
        return Err(format!(
            "target TCP session failed: passed={} transport_errors={}",
            structured.finished.summary.passed, structured.transport.transport_errors,
        )
        .into());
    }
    if let Some(host) = host_tx {
        if host.bytes != structured.transport.rx_bytes || structured.transport.rx_units != 1 {
            return Err(format!(
                "host-to-target TCP mismatch: host={} target={} units={}",
                host.bytes, structured.transport.rx_bytes, structured.transport.rx_units,
            )
            .into());
        }
        let floor = options
            .rx_floor_bps
            .expect("RX direction has an acceptance floor");
        if host.throughput_bps() < floor {
            return Err(format!(
                "host-to-target TCP rate is below the configured {floor} bit/s RX floor"
            )
            .into());
        }
    } else if structured.transport.rx_bytes != 0 || structured.transport.rx_units != 0 {
        return Err("TCP TX-only session reported target receive traffic".into());
    }
    if let Some(host) = host_rx {
        if host.bytes != structured.transport.tx_bytes
            || structured.transport.tx_units != 1
            || !host.eof
            || host.pattern_errors != 0
        {
            return Err(format!(
                "target-to-host TCP mismatch: host={} target={} units={} eof={} pattern_errors={}",
                host.bytes,
                structured.transport.tx_bytes,
                structured.transport.tx_units,
                host.eof,
                host.pattern_errors,
            )
            .into());
        }
        let floor = options
            .tx_floor_bps
            .expect("TX direction has an acceptance floor");
        if host.throughput_bps() < floor {
            return Err(format!(
                "host TCP receive rate is below the configured {} bit/s TX floor",
                floor
            )
            .into());
        }
    } else if structured.transport.tx_bytes != 0 || structured.transport.tx_units != 0 {
        return Err("TCP RX-only session reported target transmit traffic".into());
    }
    Ok(())
}

fn write_report(
    output: &Path,
    options: &Options,
    host_tx: Option<HostTransmission>,
    host_rx: Option<HostReception>,
    structured: SessionEvidence,
    failure: Option<&str>,
) -> Result<()> {
    let result = if failure.is_some() { "FAIL" } else { "PASS" };
    let failure = failure
        .map(|failure| format!("- Acceptance failure: `{failure}`\n"))
        .unwrap_or_default();
    fs::write(
        output.join("report.md"),
        format!(
            "# Open-radio TCP {} HIL\n\n\
             - Result: `{result}`\n\
             {failure}\
             - Device: `{}`\n\
             - Host-to-target bytes/rate: `{}` / `{:.3} Mbit/s`\n\
             - Host-to-target offer: `{}` bit/s\n\
             - Host TX writes/max lateness/catch-up/deadline resets: `{}` / `{} us` / `{}` / `{}`\n\
             - Target-to-host bytes/rate: `{}` / `{:.3} Mbit/s`\n\
             - Target-to-host offer/floor: `{}` / `{}` bit/s\n\
             - Host RX reads: `{}`\n\
             - Typed RX/TX bytes: `{}` / `{}`\n\
             - Typed RX/TX streams: `{}` / `{}`\n\
             - Host RX EOF/pattern errors: `{}` / `{}`\n\
             - Target elapsed: `{}` us\n\
             - Stack minimum free: CPU0 `{}/{}` bytes (required `{}`); CPU1 `{}/{}` bytes (required `{}`)\n\
             - Typed evidence CRC32C: `0x{:08x}`\n\n\
             Byte equality and the deterministic absolute-offset pattern are required independently.\n\n\
             UART evidence is in [`uart.log`](uart.log).\n",
            direction_name(options.direction),
            options.device,
            host_tx.map_or(0, |value| value.bytes),
            host_tx.map_or(0.0, |value| value.throughput_bps() as f64 / 1_000_000.0),
            options.rx_rate_bps.unwrap_or(0),
            host_tx.map_or(0, |value| value.writes),
            host_tx.map_or(0, |value| value.maximum_lateness_us()),
            host_tx.map_or(0, |value| value.maximum_catch_up_writes),
            host_tx.map_or(0, |value| value.deadline_resets),
            host_rx.map_or(0, |value| value.bytes),
            host_rx.map_or(0.0, |value| value.throughput_bps() as f64 / 1_000_000.0),
            options.tx_rate_bps.unwrap_or(0),
            options.tx_floor_bps.unwrap_or(0),
            host_rx.map_or(0, |value| value.reads),
            structured.transport.rx_bytes,
            structured.transport.tx_bytes,
            structured.transport.rx_units,
            structured.transport.tx_units,
            host_rx.is_some_and(|value| value.eof),
            host_rx.map_or(0, |value| value.pattern_errors),
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

fn parse_options(arguments: &[String], lab: &LabConfig, direction: Direction) -> Result<Options> {
    let mut options = Options {
        device: Ipv4Addr::UNSPECIFIED,
        direction,
        port: DEFAULT_PORT,
        duration: DEFAULT_DURATION,
        chunk_bytes: DEFAULT_CHUNK_BYTES,
        rx_rate_bps: match direction {
            Direction::Rx => Some(DEFAULT_RX_RATE_BPS),
            Direction::Tx => None,
            Direction::Bidirectional => Some(DEFAULT_BIDIRECTIONAL_RX_RATE_BPS),
        },
        rx_floor_bps: match direction {
            Direction::Rx => Some(DEFAULT_RX_RATE_BPS * 9 / 10),
            Direction::Tx => None,
            Direction::Bidirectional => Some(DEFAULT_BIDIRECTIONAL_RX_RATE_BPS * 9 / 10),
        },
        tx_rate_bps: match direction {
            Direction::Rx => None,
            Direction::Tx => Some(DEFAULT_TX_RATE_BPS),
            Direction::Bidirectional => Some(DEFAULT_BIDIRECTIONAL_TX_RATE_BPS),
        },
        tx_floor_bps: match direction {
            Direction::Rx => None,
            Direction::Tx => Some(DEFAULT_TX_FLOOR_BPS),
            Direction::Bidirectional => Some(DEFAULT_BIDIRECTIONAL_TX_FLOOR_BPS),
        },
        serial: lab.device.serial.clone(),
    };
    let mut index = 0;
    while index < arguments.len() {
        let value = arguments
            .get(index + 1)
            .ok_or("TCP option requires a value")?;
        match arguments[index].as_str() {
            "--rx-rate" => options.rx_rate_bps = Some(parse_rate(value)?),
            "--rx-floor" => options.rx_floor_bps = Some(parse_rate(value)?),
            "--tx-rate" => options.tx_rate_bps = Some(parse_rate(value)?),
            "--tx-floor" => options.tx_floor_bps = Some(parse_rate(value)?),
            "--seconds" => {
                let seconds = value.parse::<u64>()?;
                if !(5..=300).contains(&seconds) {
                    return Err("TCP duration must be between 5 and 300 seconds".into());
                }
                options.duration = Duration::from_secs(seconds);
            }
            "--chunk" => {
                options.chunk_bytes = value.parse()?;
                if !(64..=MAXIMUM_CHUNK_BYTES).contains(&options.chunk_bytes) {
                    return Err("TCP chunk must be between 64 and 32768 bytes".into());
                }
            }
            "--port" => options.port = value.parse()?,
            option => return Err(format!("unsupported TCP option `{option}`").into()),
        }
        index += 2;
    }
    let shape_valid = match direction {
        Direction::Rx => {
            options.rx_rate_bps.is_some()
                && options.rx_floor_bps.is_some()
                && options.tx_rate_bps.is_none()
                && options.tx_floor_bps.is_none()
        }
        Direction::Tx => {
            options.rx_rate_bps.is_none()
                && options.rx_floor_bps.is_none()
                && options.tx_rate_bps.is_some()
                && options.tx_floor_bps.is_some()
        }
        Direction::Bidirectional => {
            options.rx_rate_bps.is_some()
                && options.rx_floor_bps.is_some()
                && options.tx_rate_bps.is_some()
                && options.tx_floor_bps.is_some()
        }
    };
    if !shape_valid {
        return Err("TCP rate options do not match the selected direction".into());
    }
    if options
        .rx_rate_bps
        .zip(options.rx_floor_bps)
        .is_some_and(|(rate, floor)| floor > rate)
    {
        return Err("TCP RX floor cannot exceed the offered rate".into());
    }
    Ok(options)
}

fn parse_rate(value: &str) -> Result<u64> {
    let rate = value.parse::<u64>()?;
    if !(100_000..=1_000_000_000).contains(&rate) {
        return Err("TCP rate must be between 100 kbit/s and 1 Gbit/s".into());
    }
    Ok(rate)
}

const fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Rx => "rx",
        Direction::Tx => "tx",
        Direction::Bidirectional => "bidirectional",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_tx_defaults_separate_offer_from_acceptance_floor() {
        let options = parse_options(&[], &LabConfig::for_test(), Direction::Tx).unwrap();

        assert_eq!(options.chunk_bytes, 32_768);
        assert_eq!(options.tx_rate_bps, Some(60_000_000));
        assert_eq!(options.tx_floor_bps, Some(45_000_000));
        assert_eq!(options.rx_rate_bps, None);
    }

    #[test]
    fn direction_rejects_an_inapplicable_flow_option() {
        assert!(
            parse_options(
                &["--tx-rate".into(), "1000000".into(),],
                &LabConfig::for_test(),
                Direction::Rx,
            )
            .is_err()
        );
    }
}
