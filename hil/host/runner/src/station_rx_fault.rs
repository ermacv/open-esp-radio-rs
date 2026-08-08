//! Host qualification for recoverable connected-RX discard ownership.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, SessionConfig, SessionLinkRequirements,
    StationFaultClassification, StationFaultEvidence, StationFaultInjection, Transport,
};

use crate::{
    Result,
    paced_udp::{Config as PacedUdpConfig, send as send_paced_udp},
    traffic_capture::{SerialCapture, await_udp_rx_ready},
};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
const DEFAULT_ADDRESS_HINT: Ipv4Addr = Ipv4Addr::new(192, 168, 178, 120);
const TARGET_RX_PORT: u16 = 4_323;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const RECOVERY_TRAFFIC_DURATION: Duration = Duration::from_millis(500);
const RECOVERY_TRAFFIC_RATE_BPS: u64 = 500_000;
const RECOVERY_TRAFFIC_PAYLOAD: usize = 256;

struct Options {
    serial: PathBuf,
    address_hint: Ipv4Addr,
    timeout: Duration,
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
    let output = root.join("target/hil/esp32s31/qualification/station-rx-fault");
    fs::create_dir_all(&output)?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify(&capture, options.address_hint, options.timeout);
    let log = capture.finish();
    fs::write(output.join("uart.log"), &log)?;
    result?;
    println!("station_rx_fault=PASS");
    println!("uart_log={}", output.join("uart.log").display());
    Ok(())
}

fn qualify(capture: &SerialCapture, address_hint: Ipv4Addr, timeout: Duration) -> Result<()> {
    let ready = await_udp_rx_ready(capture, address_hint, TARGET_RX_PORT, timeout)?;
    let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
    if !capabilities.features.station_fault_injection {
        return Err("firmware does not advertise station fault injection".into());
    }
    if !capabilities.features.station_epoch_control {
        return Err("firmware does not advertise station epoch control".into());
    }
    if !ready.runtime_session {
        return Err("station RX fault cell requires runtime RX sessions".into());
    }

    let handle = capture.request_station_fault_injection(
        StationFaultInjection::ConnectedRxBeforeStagingOverCapacity,
    )?;
    send_negative_rx_probes(ready.address)?;
    let evidence = capture.wait_station_fault(handle, timeout)?;
    validate_fault_evidence(evidence)?;
    println!(
        "station_rx_fault_frontier=PASS descriptor_reloaded=1 following_staged=1 \
         same_ring_live=1 service_result_ok=1"
    );

    let epoch = capture.request_station_epoch_cycle()?;
    let deadline = Instant::now() + timeout;
    let epoch_evidence = loop {
        if let Some(evidence) = capture.observed_station_epoch_completion(epoch) {
            break evidence;
        }
        if Instant::now() >= deadline {
            return Err("station did not complete a fresh epoch after RX recovery".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    if !epoch_evidence.is_complete() {
        return Err(format!(
            "station returned incomplete post-RX-fault epoch evidence: {epoch_evidence:?}"
        )
        .into());
    }
    println!("station_rx_fault_epoch_recovery=PASS");

    // `ConnectedRunnerStarted` closes driver ownership, but it does not claim
    // that peer BlockAck/network ingress has reached a steady measurement
    // state. Reuse the ordinary RX qualifier's out-of-band negative warm-up
    // and explicit settle interval before opening the exact-delivery sample.
    send_negative_rx_probes(ready.address)?;
    thread::sleep(Duration::from_secs(1));

    qualify_post_reconnect_rx(capture, ready.address, timeout)
}

fn send_negative_rx_probes(address: Ipv4Addr) -> Result<()> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect(SocketAddrV4::new(address, TARGET_RX_PORT))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut packet = [0x5a; 64];
    // Negative sequence records are already the target's out-of-band warm-up
    // vocabulary. They exercise the complete Wi-Fi RX path but cannot become
    // measured traffic when the later recovery session starts.
    packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
    for _ in 0..8 {
        if socket.send(&packet)? != packet.len() {
            return Err("short station RX fault probe".into());
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

fn qualify_post_reconnect_rx(
    capture: &SerialCapture,
    address: Ipv4Addr,
    timeout: Duration,
) -> Result<()> {
    let session = capture.start_session(SessionConfig {
        transport: Transport::Udp,
        direction: Direction::Rx,
        completion: Completion::DurationMillis(u32::try_from(
            RECOVERY_TRAFFIC_DURATION.as_millis(),
        )?),
        peer: None,
        target_rx: Some(FlowConfig {
            payload_bytes: RECOVERY_TRAFFIC_PAYLOAD as u16,
            offered_rate_bps: Some(RECOVERY_TRAFFIC_RATE_BPS),
        }),
        target_tx: None,
        link_requirements: SessionLinkRequirements::NONE,
    })?;
    let host = send_paced_udp(PacedUdpConfig {
        address,
        port: TARGET_RX_PORT,
        rate_bps: RECOVERY_TRAFFIC_RATE_BPS,
        duration: RECOVERY_TRAFFIC_DURATION,
        payload: RECOVERY_TRAFFIC_PAYLOAD,
    })?;
    let evidence = capture.wait_for_session(session, timeout)?;
    capture.acknowledge_session(session)?;
    if !evidence.finished.summary.passed
        || evidence.transport.transport_errors != 0
        || evidence.transport.rx_units != host.datagrams
        || evidence.transport.rx_bytes != host.bytes
        || evidence.transport.tx_units != 0
        || evidence.transport.tx_bytes != 0
    {
        return Err(format!(
            "post-RX-fault traffic did not preserve exact delivery: host={}/{} target={:?}",
            host.bytes, host.datagrams, evidence.transport,
        )
        .into());
    }
    println!(
        "station_rx_fault_post_reconnect_traffic=PASS bytes={} datagrams={}",
        host.bytes, host.datagrams,
    );
    Ok(())
}

fn validate_fault_evidence(evidence: StationFaultEvidence) -> Result<()> {
    if evidence.injection() != StationFaultInjection::ConnectedRxBeforeStagingOverCapacity {
        return Err(format!("wrong station RX fault injection: {evidence:?}").into());
    }
    if evidence.classification() != StationFaultClassification::RecoverableFrameDiscard {
        return Err(format!("station RX fault was not recoverable: {evidence:?}").into());
    }
    if !evidence.is_complete() {
        return Err(format!("incomplete station RX recovery frontier: {evidence:?}").into());
    }
    Ok(())
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
        address_hint: DEFAULT_ADDRESS_HINT,
        timeout: DEFAULT_TIMEOUT,
    };
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--serial" => {
                index += 1;
                options.serial = PathBuf::from(
                    arguments
                        .get(index)
                        .ok_or("--serial requires a device path")?,
                );
            }
            "--address-hint" => {
                index += 1;
                options.address_hint = arguments
                    .get(index)
                    .ok_or("--address-hint requires an IPv4 address")?
                    .parse()?;
            }
            "--timeout-seconds" => {
                index += 1;
                let seconds = arguments
                    .get(index)
                    .ok_or("--timeout-seconds requires a value")?
                    .parse::<u64>()?;
                if !(30..=180).contains(&seconds) {
                    return Err("--timeout-seconds must be in 30..=180".into());
                }
                options.timeout = Duration::from_secs(seconds);
            }
            argument => return Err(format!("unknown station RX-fault option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn print_help() {
    println!(
        "cargo hil station rx-fault [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --address-hint <ipv4>    fallback before typed DHCP evidence\n\
         --timeout-seconds <n>    per-edge deadline, 30..=180 (default 60)\n\n\
         Requires the `cargo hil flash radio` image. It narrows admission for\n\
         one real completed RX unit before staging, requires descriptor reload\n\
         plus a following staged unit on the same live ring, then proves a new\n\
         station epoch and exact post-reconnect UDP RX delivery."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_recoverable_frontier_is_required() {
        let complete = StationFaultEvidence::ConnectedRxOverCapacityRecovered {
            classification: StationFaultClassification::RecoverableFrameDiscard,
            descriptor_reloaded: true,
            following_unit_staged: true,
            same_ring_live: true,
            service_result_ok: true,
        };
        validate_fault_evidence(complete).unwrap();
        let incomplete = StationFaultEvidence::ConnectedRxOverCapacityRecovered {
            classification: StationFaultClassification::RecoverableFrameDiscard,
            descriptor_reloaded: true,
            following_unit_staged: false,
            same_ring_live: true,
            service_result_ok: true,
        };
        assert!(validate_fault_evidence(incomplete).is_err());
    }

    #[test]
    fn parser_keeps_rx_fault_deadline_bounded() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--address-hint".into(),
            "192.0.2.31".into(),
            "--timeout-seconds".into(),
            "90".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
        assert_eq!(options.address_hint, Ipv4Addr::new(192, 0, 2, 31));
        assert_eq!(options.timeout, Duration::from_secs(90));
    }
}
