//! Typed TCP RX, TX and full-duplex qualification over one runtime image.

use crate::execution::context::Context;
use std::{fs, net::Ipv4Addr, path::Path, time::Duration};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, SessionConfig, SessionFlowConfig, SessionLinkRequirements,
    Transport,
};

use crate::{
    Result,
    session::{SessionEvidence, await_tcp_ready},
    workload::traffic::{
        host_network::reject_overlapping_ipv4_links,
        paced_tcp::{
            Config as PacedTcpConfig, HostReception, HostTransmission, exchange, receive, send,
        },
    },
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
pub(crate) struct Config {
    pub(crate) device: Ipv4Addr,
    pub(crate) direction: Direction,
    pub(crate) port: u16,
    pub(crate) duration: Duration,
    pub(crate) chunk_bytes: usize,
    pub(crate) rx_rate_bps: Option<u64>,
    pub(crate) rx_floor_bps: Option<u64>,
    pub(crate) tx_rate_bps: Option<u64>,
    pub(crate) tx_floor_bps: Option<u64>,
}

pub(crate) fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_no_beacon_loss: bool,
) -> Result<()> {
    let mut options = options.validate()?;
    let direction = options.direction;
    let mode = direction_name(direction);
    fs::create_dir_all(output)?;
    let capture = context.capture(output)?;
    let ready = match await_tcp_ready(
        &capture,
        context,
        options.device,
        options.port,
        direction,
        DEVICE_READY_TIMEOUT,
    ) {
        Ok(ready) => ready,
        Err(error) => {
            return capture.finish_with(Err(error));
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
                    pacing_group_datagrams: None,
                }),
                target_tx: options.tx_rate_bps.map(|rate| FlowConfig {
                    payload_bytes: u16::try_from(options.chunk_bytes).expect("validated TCP chunk"),
                    offered_rate_bps: Some(rate),
                    pacing_group_datagrams: None,
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

    if let Ok((host_tx, host_rx)) = &data_plane {
        if let Some(host) = host_tx {
            context.measurements.rate(
                "tcp.rx.host-rate",
                host.throughput_bps(),
                options.rx_floor_bps,
            );
        }
        if let Some(host) = host_rx {
            context.measurements.rate(
                "tcp.tx.host-rate",
                host.throughput_bps(),
                options.tx_floor_bps,
            );
        }
    }

    // Always collect the target's typed result after a data-plane failure.
    // Otherwise a reset/timeout hides precisely the evidence needed to
    // distinguish a host socket problem from a target transport failure.
    let structured = capture.wait_for_session(session, timeout);
    let acknowledgement = structured
        .as_ref()
        .map(|_| capture.acknowledge_session(session))
        .unwrap_or(Ok(()));
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish()?;
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
    options: &Config,
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
    options: &Config,
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

impl Config {
    pub(crate) fn for_direction(direction: Direction) -> Self {
        Self {
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
        }
    }
}
impl Config {
    fn validate(self) -> Result<Self> {
        if !(Duration::from_secs(5)..=Duration::from_secs(300)).contains(&self.duration) {
            return Err("traffic duration must be in 5..=300 seconds".into());
        }
        if !(64..=MAXIMUM_CHUNK_BYTES).contains(&self.chunk_bytes) {
            return Err("TCP chunk must be in 64..=32768 bytes".into());
        }
        if [
            self.rx_rate_bps,
            self.tx_rate_bps,
            self.rx_floor_bps,
            self.tx_floor_bps,
        ]
        .into_iter()
        .flatten()
        .any(|rate| !(100_000..=1_000_000_000).contains(&rate))
        {
            return Err("traffic rate is outside the supported range".into());
        }

        let shape_valid = match self.direction {
            Direction::Rx => {
                self.rx_rate_bps.is_some()
                    && self.rx_floor_bps.is_some()
                    && self.tx_rate_bps.is_none()
                    && self.tx_floor_bps.is_none()
            }
            Direction::Tx => {
                self.rx_rate_bps.is_none()
                    && self.rx_floor_bps.is_none()
                    && self.tx_rate_bps.is_some()
                    && self.tx_floor_bps.is_some()
            }
            Direction::Bidirectional => {
                self.rx_rate_bps.is_some()
                    && self.rx_floor_bps.is_some()
                    && self.tx_rate_bps.is_some()
                    && self.tx_floor_bps.is_some()
            }
        };
        if !shape_valid {
            return Err("TCP rate self do not match the selected direction".into());
        }
        if self
            .rx_rate_bps
            .zip(self.rx_floor_bps)
            .is_some_and(|(rate, floor)| floor > rate)
        {
            return Err("TCP RX floor cannot exceed the offered rate".into());
        }

        Ok(self)
    }
}

const fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Rx => "rx",
        Direction::Tx => "tx",
        Direction::Bidirectional => "bidirectional",
    }
}

#[cfg(test)]
mod tests;
