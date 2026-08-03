//! Shared UART capture and end-to-end readiness probes for traffic HIL cells.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Capabilities, Command, Direction, Envelope, Event, FrameDecoder, FrameEncoder,
    NetworkCredentials, Transport,
};
use zeroize::Zeroizing;

use crate::Result;

const RX_BENCH_INTERVAL_COMPLETE_MARKER: &str = "stage=udp-rx-interval-complete";
const RADIO_RUNNER_FAILURE_MARKER: &str = "result=FAIL stage=production-runner";
const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const DHCP_DISCOVERY_GRACE: Duration = Duration::from_millis(500);
const PROTOCOL_READY_TIMEOUT: Duration = Duration::from_secs(10);

struct ProtocolEvents {
    messages: Mutex<Vec<Envelope<Event>>>,
    changed: Condvar,
}

/// Concurrent UART transcript retained across traffic setup and measurement.
pub(crate) struct SerialCapture {
    stop: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
    protocol: Arc<ProtocolEvents>,
    outbound: mpsc::Sender<Zeroizing<Vec<u8>>>,
    next_host_sequence: AtomicU32,
    worker: Option<thread::JoinHandle<()>>,
}

impl SerialCapture {
    pub(crate) fn start(port: &Path) -> Self {
        Self::start_inner(port, false)
    }

    /// Open the diagnostics owner before resetting the USB-Serial/JTAG target.
    ///
    /// Traffic qualification needs the DHCP and UDP-ready records from the
    /// current boot. Resetting through a second process after opening this
    /// handle is impossible because `serialport` owns the device exclusively.
    pub(crate) fn start_with_reset(port: &Path) -> Self {
        Self::start_inner(port, true)
    }

    fn start_inner(port: &Path, reset_target: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let protocol = Arc::new(ProtocolEvents {
            messages: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        });
        let (outbound, outbound_rx) = mpsc::channel::<Zeroizing<Vec<u8>>>();
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let worker_protocol = Arc::clone(&protocol);
        let port = port.to_owned();
        let worker = thread::spawn(move || {
            let mut serial = match serialport::new(port.to_string_lossy(), 115_200)
                .timeout(Duration::from_millis(20))
                .open()
            {
                Ok(serial) => serial,
                Err(error) => {
                    append(
                        &worker_bytes,
                        format!("serial capture failed for {}: {error}\n", port.display())
                            .as_bytes(),
                    );
                    return;
                }
            };
            if reset_target {
                let _ = serial.clear(serialport::ClearBuffer::Input);
            }
            if reset_target && let Err(error) = reset_usb_serial_jtag(&mut *serial) {
                append(
                    &worker_bytes,
                    format!(
                        "serial target reset failed for {}: {error}\n",
                        port.display()
                    )
                    .as_bytes(),
                );
                return;
            }
            let mut decoder = FrameDecoder::new();
            let mut buffer = [0_u8; 2_048];
            while !worker_stop.load(Ordering::Acquire) {
                while let Ok(frame) = outbound_rx.try_recv() {
                    if let Err(error) = serial.write_all(frame.as_slice()) {
                        append(
                            &worker_bytes,
                            format!("\nserial write failed: {error}\n").as_bytes(),
                        );
                        return;
                    }
                }
                match serial.read(&mut buffer) {
                    Ok(length) => {
                        append(&worker_bytes, &buffer[..length]);
                        decoder.feed::<Envelope<Event>>(&buffer[..length], |message| {
                            if let Ok(message) = message {
                                let mut messages = worker_protocol
                                    .messages
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                messages.push(message);
                                worker_protocol.changed.notify_all();
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(error) => {
                        append(
                            &worker_bytes,
                            format!("\nserial read failed: {error}\n").as_bytes(),
                        );
                        break;
                    }
                }
            }
        });
        Self {
            stop,
            bytes,
            protocol,
            outbound,
            next_host_sequence: AtomicU32::new(1),
            worker: Some(worker),
        }
    }

    /// Performs one typed host-to-target round trip and returns the current
    /// image capabilities. The old text readiness path remains active only as
    /// a compatibility oracle while benchmark evidence is migrated.
    pub(crate) fn request_capabilities(&self, timeout: Duration) -> Result<Capabilities> {
        let hello = self
            .wait_for_protocol_after(0, timeout, |message| {
                matches!(message.body, Event::Hello(_))
            })
            .ok_or("device did not publish a HIL protocol hello")?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let command = Envelope::new(
            hello.boot_id,
            request_id,
            0,
            request_id,
            Command::GetCapabilities,
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL capability request: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| "serial worker stopped before capability request")?;
        let response = self
            .wait_for_protocol_after(event_count, timeout, |message| {
                message.boot_id == hello.boot_id
                    && message.request_id == request_id
                    && matches!(message.body, Event::Hello(_))
            })
            .ok_or("device did not answer the HIL capability request")?;
        match response.body {
            Event::Hello(capabilities) => Ok(capabilities),
            _ => unreachable!("protocol event predicate accepted only Hello"),
        }
    }

    /// Establishes the typed link and provisions this boot from host-owned
    /// environment secrets. The passphrase is never echoed by the target or
    /// appended to the UART capture.
    fn prepare_protocol(&self) -> Result<Capabilities> {
        let capabilities = self.request_capabilities(PROTOCOL_READY_TIMEOUT)?;
        if capabilities.features.network_provisioning {
            self.provision_network(PROTOCOL_READY_TIMEOUT)?;
        }
        Ok(capabilities)
    }

    fn provision_network(&self, timeout: Duration) -> Result<()> {
        let ssid = network_environment("OPEN_RADIO_HIL_STA_SSID", "OPEN_RADIO_STA_SSID")?;
        let passphrase = Zeroizing::new(network_environment(
            "OPEN_RADIO_HIL_STA_PASSWORD",
            "OPEN_RADIO_STA_PASSWORD",
        )?);
        let credentials = NetworkCredentials::try_new(ssid.as_bytes(), passphrase.as_bytes())
            .map_err(|error| format!("invalid HIL network credentials: {error}"))?;
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before network provisioning")?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let command = Envelope::new(
            boot_id,
            request_id,
            0,
            request_id,
            Command::ProvisionNetwork(credentials),
        );
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL network provisioning: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| "serial worker stopped before network provisioning")?;
        let response = self
            .wait_for_protocol_after(event_count, timeout, |message| {
                message.boot_id == boot_id && message.request_id == request_id
            })
            .ok_or("device did not acknowledge HIL network provisioning")?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected HIL network provisioning: {reason:?}").into())
            }
            _ => Err("device returned an invalid network provisioning response".into()),
        }
    }

    fn latest_boot_id(&self) -> Option<u64> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages.iter().rev().find_map(|message| {
            if matches!(message.body, Event::Hello(_)) {
                Some(message.boot_id)
            } else {
                None
            }
        })
    }

    pub(crate) fn observed_protocol_ipv4(&self) -> Option<Ipv4Addr> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages
            .iter()
            .rev()
            .find_map(|message| match message.body {
                Event::NetworkReady(network) => Some(Ipv4Addr::from(network.address)),
                _ => None,
            })
    }

    fn observed_udp_service(&self, direction: Direction, port: u16) -> bool {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages.iter().any(|message| match message.body {
            Event::ServiceReady(service) => {
                service.transport == Transport::Udp
                    && service.direction == direction
                    && service.local_port == port
            }
            _ => false,
        })
    }

    fn protocol_event_count(&self) -> usize {
        self.protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn wait_for_protocol_after(
        &self,
        start: usize,
        timeout: Duration,
        predicate: impl Fn(&Envelope<Event>) -> bool,
    ) -> Option<Envelope<Event>> {
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(message) = messages
                .get(start..)
                .unwrap_or_default()
                .iter()
                .find(|message| predicate(message))
            {
                return Some(message.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return None;
            }
        }
    }

    pub(crate) fn contains(&self, marker: &str) -> bool {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        bytes
            .windows(marker.len())
            .any(|candidate| candidate == marker.as_bytes())
    }

    fn marker_count(&self, marker: &str) -> usize {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        bytes
            .windows(marker.len())
            .filter(|candidate| *candidate == marker.as_bytes())
            .count()
    }

    fn wait_for_marker_after(
        &self,
        marker: &str,
        previous_count: usize,
        timeout: Duration,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.marker_count(marker) > previous_count {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::sleep((deadline - now).min(Duration::from_millis(20)));
        }
    }

    fn transcript(&self) -> String {
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(crate) fn finish(mut self) -> String {
        self.stop_and_join();
        let bytes = self
            .bytes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn stop_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Reset an ESP USB-Serial/JTAG target without giving up the capture handle.
///
/// This is the `espflash` `reset_after_flash` USB-Serial/JTAG sequence. DTR is
/// kept high at the board pin, while the RTS transition issues the chip reset.
fn reset_usb_serial_jtag(serial: &mut dyn serialport::SerialPort) -> serialport::Result<()> {
    thread::sleep(Duration::from_millis(100));
    serial.write_data_terminal_ready(false)?;
    thread::sleep(Duration::from_millis(100));
    serial.write_request_to_send(true)?;
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(true)?;
    thread::sleep(Duration::from_millis(100));
    serial.write_request_to_send(false)
}

impl Drop for SerialCapture {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn append(bytes: &Mutex<Vec<u8>>, chunk: &[u8]) {
    bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend_from_slice(chunk);
}

fn network_environment(primary: &str, compatibility: &str) -> Result<String> {
    std::env::var(primary)
        .or_else(|_| std::env::var(compatibility))
        .map_err(|_| {
            format!(
                "missing `{primary}`; provide network credentials to the HIL runner environment"
            )
            .into()
        })
}

/// Prove that the target owns its IPv4 address, UART and complete UDP RX path.
///
/// One positive datagram followed by the target's idle timeout creates one
/// deliberately unqualified sample. The probe is sent only after the UDP
/// socket and DHCP configuration are visible. It deliberately has no negative
/// terminal datagram: a delayed control packet must never split the beginning
/// of the measured stream. Waiting for a *new* interval-complete marker also
/// prevents a previous probe's UART record from satisfying a retry.
pub(crate) fn await_udp_rx_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<Ipv4Addr> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.udp || !capabilities.features.rx {
        return Err("firmware does not advertise UDP RX capability".into());
    }
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    let mut address = address_hint;
    socket.connect(SocketAddrV4::new(address, port))?;
    socket.set_write_timeout(Some(Duration::from_millis(250)))?;
    let mut packet = [0x5a; RX_PROBE_PAYLOAD];
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if capture.contains(RADIO_RUNNER_FAILURE_MARKER) {
            return Err("radio runner failed before UDP RX became ready".into());
        }
        if let Some(discovered) = capture
            .observed_protocol_ipv4()
            .or_else(|| observed_dhcp_ipv4(&capture.transcript()))
            && discovered != address
        {
            address = discovered;
            socket.connect(SocketAddrV4::new(address, port))?;
        }
        if !(capture.observed_udp_service(Direction::Rx, port)
            || capture.contains("stage=udp-rx-ready"))
            || capture
                .observed_protocol_ipv4()
                .or_else(|| observed_dhcp_ipv4(&capture.transcript()))
                .is_none()
        {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        let completed_intervals = capture.marker_count(RX_BENCH_INTERVAL_COMPLETE_MARKER);
        packet[..4].copy_from_slice(&0_i32.to_be_bytes());
        let _ = socket.send(&packet);
        if capture.wait_for_marker_after(
            RX_BENCH_INTERVAL_COMPLETE_MARKER,
            completed_intervals,
            RX_PROBE_RESPONSE_TIMEOUT,
        ) {
            // The marker follows the last compact telemetry record. Leave
            // one small scheduling interval for the benchmark task to yield
            // and close any network-ready wait that overlapped the probe.
            thread::sleep(Duration::from_millis(10));
            return Ok(address);
        }
    }

    Err(format!(
        "device {address}:{port} did not confirm end-to-end UDP RX within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

pub(crate) fn await_device_marker(
    capture: &SerialCapture,
    marker: &str,
    address_hint: Ipv4Addr,
    timeout: Duration,
) -> Result<Ipv4Addr> {
    let _ = capture.prepare_protocol()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.contains(RADIO_RUNNER_FAILURE_MARKER) {
            return Err(format!("radio runner failed before `{marker}`").into());
        }
        if capture.observed_udp_service(Direction::Tx, 4_324) || capture.contains(marker) {
            let discovery_deadline = Instant::now() + DHCP_DISCOVERY_GRACE;
            while Instant::now() < discovery_deadline {
                if let Some(address) = capture
                    .observed_protocol_ipv4()
                    .or_else(|| observed_dhcp_ipv4(&capture.transcript()))
                {
                    return Ok(address);
                }
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(address_hint);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish `{marker}` within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

fn observed_dhcp_ipv4(transcript: &str) -> Option<Ipv4Addr> {
    transcript.lines().rev().find_map(|line| {
        if !line.contains("stage=embassy-net-dhcp") {
            return None;
        }
        let address = line
            .split_whitespace()
            .find_map(|token| token.strip_prefix("address="))?
            .split('/')
            .next()?;
        address.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::observed_dhcp_ipv4;

    #[test]
    fn extracts_latest_dhcp_address_from_uart_transcript() {
        let transcript = "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-dhcp address=192.168.178.120/24 gateway=None\n\
                          OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-dhcp address=192.168.178.121/24 gateway=None\n";

        assert_eq!(
            observed_dhcp_ipv4(transcript),
            Some("192.168.178.121".parse().unwrap())
        );
    }
}
