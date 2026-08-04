//! Deterministic connected-TX reset frontier and reboot recovery cell.

use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    time::Duration,
};

use open_esp_radio_hil_protocol::{
    Completion, Direction, FlowConfig, Ipv4Endpoint, SessionConfig, StationFaultClassification,
    StationFaultEvidence, StationFaultInjection, Transport,
};

use crate::{
    Result,
    controlled_ap::{ControlledAp, require_credentials_environment},
    traffic_capture::{SerialCapture, await_device_marker},
};

const DEFAULT_SERIAL: &str = "/dev/ttyACM0";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);
const DEVICE_HINT: Ipv4Addr = Ipv4Addr::new(10, 42, 0, 2);
const TARGET_TX_PORT: u16 = 4_324;
const HOST_RX_PORT: u16 = 9_002;
const DEVICE_TX_READY_MARKER: &str = "result=PASS stage=udp-tx-ready ";

struct Options {
    serial: PathBuf,
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
    require_credentials_environment()?;
    let output = root.join("target/hil/esp32s31/qualification/station-tx-fault");
    fs::create_dir_all(&output)?;

    let ap = ControlledAp::start()?;
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, HOST_RX_PORT))?;
    let capture = SerialCapture::start_with_reset(&options.serial);
    let result = qualify_fault(&capture, &socket, options.timeout);
    let log = capture.finish();
    fs::write(output.join("uart-fault.log"), &log)?;
    let evidence = result?;
    fs::write(
        output.join("evidence.txt"),
        format!(
            "injection={:?}\nclassification={:?}\nrunner_returned={}\n\
             executor_tasks_stopped={}\nrx_dma_stopped={}\ntx_owner_reset_required={}\n",
            evidence.injection,
            evidence.classification,
            evidence.runner_returned,
            evidence.executor_tasks_stopped,
            evidence.rx_dma_stopped,
            evidence.tx_owner_reset_required,
        ),
    )?;

    // A reset-required owner is intentionally not reused in place. Reboot the
    // same image and prove that cold platform recovery reaches a fresh
    // connected/network-ready epoch while the controlled AP remains present.
    let recovery = SerialCapture::start_with_reset(&options.serial);
    await_device_marker(
        &recovery,
        DEVICE_TX_READY_MARKER,
        DEVICE_HINT,
        options.timeout,
    )?;
    let recovery_log = recovery.finish();
    fs::write(output.join("uart-recovery.log"), &recovery_log)?;
    drop(ap);

    println!("station_tx_fault=PASS");
    println!("uart_fault_log={}", output.join("uart-fault.log").display());
    println!(
        "uart_recovery_log={}",
        output.join("uart-recovery.log").display()
    );
    Ok(())
}

fn qualify_fault(
    capture: &SerialCapture,
    socket: &UdpSocket,
    timeout: Duration,
) -> Result<StationFaultEvidence> {
    let ready = await_device_marker(capture, DEVICE_TX_READY_MARKER, DEVICE_HINT, timeout)?;
    let capabilities = capture.request_capabilities(Duration::from_secs(10))?;
    if !capabilities.features.station_fault_injection {
        return Err("firmware does not advertise station fault injection".into());
    }
    if !ready.runtime_session {
        return Err("station TX fault cell requires runtime TX sessions".into());
    }

    let route_probe = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    route_probe.connect(SocketAddrV4::new(ready.address, TARGET_TX_PORT))?;
    let host_address = match route_probe.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("station TX fault cell requires IPv4".into()),
    };
    socket.send_to(&[0], SocketAddrV4::new(ready.address, TARGET_TX_PORT))?;

    let handle = capture
        .request_station_fault_injection(StationFaultInjection::ConnectedTxAfterPublication)?;
    let _session = capture.start_session(SessionConfig {
        transport: Transport::Udp,
        direction: Direction::Tx,
        completion: Completion::DurationMillis(5_000),
        peer: Some(Ipv4Endpoint {
            address: host_address.octets(),
            port: HOST_RX_PORT,
        }),
        target_rx: None,
        target_tx: Some(FlowConfig {
            payload_bytes: 512,
            offered_rate_bps: Some(1_000_000),
        }),
    })?;
    let evidence = capture.wait_station_fault(handle, timeout)?;
    validate_fault_evidence(evidence)?;
    println!(
        "station_tx_fault_frontier=PASS runner_returned=1 tasks_stopped=1 \
         rx_dma_stopped=1 tx_reset_required=1"
    );
    Ok(evidence)
}

fn validate_fault_evidence(evidence: StationFaultEvidence) -> Result<()> {
    if evidence.injection != StationFaultInjection::ConnectedTxAfterPublication {
        return Err(format!("wrong station fault injection: {evidence:?}").into());
    }
    if evidence.classification != StationFaultClassification::RadioResetRequired {
        return Err(
            format!("station fault was not classified reset-required: {evidence:?}").into(),
        );
    }
    if !evidence.is_complete() {
        return Err(format!("incomplete station fault owner frontier: {evidence:?}").into());
    }
    Ok(())
}

fn parse_options(arguments: &[String]) -> Result<Options> {
    let mut options = Options {
        serial: PathBuf::from(DEFAULT_SERIAL),
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
            argument => return Err(format!("unknown station TX-fault option `{argument}`").into()),
        }
        index += 1;
    }
    Ok(options)
}

fn print_help() {
    println!(
        "cargo hil station tx-fault [options]\n\n\
         --serial <path>          diagnostics device (default /dev/ttyACM0)\n\
         --timeout-seconds <n>    deadline for fault and reboot edges, 30..=180 (default 45)\n\n\
         Requires the `cargo hil flash station-tx-fault` image. It arms a one-shot fault\n\
         after real connected TX descriptor publication, requires the typed\n\
         reset-required owner frontier, resets the target, and proves a fresh\n\
         network-ready epoch against the repository-controlled AP."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_reset_frontier_is_required() {
        let complete = StationFaultEvidence {
            injection: StationFaultInjection::ConnectedTxAfterPublication,
            classification: StationFaultClassification::RadioResetRequired,
            runner_returned: true,
            executor_tasks_stopped: true,
            rx_dma_stopped: true,
            tx_owner_reset_required: true,
        };
        validate_fault_evidence(complete).unwrap();
        let incomplete = StationFaultEvidence {
            tx_owner_reset_required: false,
            ..complete
        };
        assert!(validate_fault_evidence(incomplete).is_err());
    }

    #[test]
    fn parser_keeps_fault_deadline_bounded() {
        let options = parse_options(&[
            "--serial".into(),
            "/dev/test-radio".into(),
            "--timeout-seconds".into(),
            "60".into(),
        ])
        .unwrap();
        assert_eq!(options.serial, PathBuf::from("/dev/test-radio"));
        assert_eq!(options.timeout, Duration::from_secs(60));
    }
}
