//! Shared UART capture and end-to-end readiness probes for traffic HIL cells.

use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::Result;

const RX_BENCH_INTERVAL_COMPLETE_MARKER: &str = "stage=udp-rx-interval-complete";
const RADIO_RUNNER_FAILURE_MARKER: &str = "result=FAIL stage=production-runner";
const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const DHCP_DISCOVERY_GRACE: Duration = Duration::from_millis(500);

/// Concurrent UART transcript retained across traffic setup and measurement.
pub(crate) struct SerialCapture {
    stop: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SerialCapture {
    pub(crate) fn start(port: &Path) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let worker_stop = Arc::clone(&stop);
        let worker_bytes = Arc::clone(&bytes);
        let port = port.to_owned();
        let worker = thread::spawn(move || {
            let mut serial = match serialport::new(port.to_string_lossy(), 115_200)
                .timeout(Duration::from_millis(100))
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
            let mut buffer = [0_u8; 2_048];
            while !worker_stop.load(Ordering::Acquire) {
                match serial.read(&mut buffer) {
                    Ok(length) => append(&worker_bytes, &buffer[..length]),
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
            worker: Some(worker),
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
        if let Some(discovered) = observed_dhcp_ipv4(&capture.transcript())
            && discovered != address
        {
            address = discovered;
            socket.connect(SocketAddrV4::new(address, port))?;
        }
        if !capture.contains("stage=udp-rx-ready")
            || observed_dhcp_ipv4(&capture.transcript()).is_none()
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
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.contains(RADIO_RUNNER_FAILURE_MARKER) {
            return Err(format!("radio runner failed before `{marker}`").into());
        }
        if capture.contains(marker) {
            let discovery_deadline = Instant::now() + DHCP_DISCOVERY_GRACE;
            while Instant::now() < discovery_deadline {
                if let Some(address) = observed_dhcp_ipv4(&capture.transcript()) {
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
