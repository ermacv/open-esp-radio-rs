//! Shared UART capture and end-to-end readiness probes for traffic HIL cells.

use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use open_esp_radio_hil_protocol::{
    Capabilities, Command, Direction, Envelope, Event, EvidenceRecord, Finished, FrameDecoder,
    FrameEncoder, NetworkConfiguration, NetworkCredentials, NetworkIpv4Configuration,
    SessionConfig, StartupArtifactChunk, StartupArtifactStatus, StationEpochEvidence,
    StationFaultEvidence, StationFaultInjection, StationLifecycleEvent, Transport,
    TransportEvidence, evidence_crc32c,
};
use zeroize::Zeroizing;

use crate::Result;
use crate::startup_artifact;

const RX_BENCH_INTERVAL_COMPLETE_MARKER: &str = "stage=udp-rx-interval-complete";
const RADIO_RUNNER_FAILURE_MARKER: &str = "result=FAIL stage=production-runner";
const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const RX_SESSION_WARMUP_SETTLE: Duration = Duration::from_secs(1);
const DHCP_DISCOVERY_GRACE: Duration = Duration::from_millis(500);
const PROTOCOL_READY_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_ARTIFACT_TIMEOUT: Duration = Duration::from_secs(30);

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
    next_session_id: AtomicU64,
    worker: Option<thread::JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpRxReady {
    pub(crate) address: Ipv4Addr,
    pub(crate) runtime_session: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct UdpTxReady {
    pub(crate) address: Ipv4Addr,
    pub(crate) runtime_session: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TcpReady {
    pub(crate) address: Ipv4Addr,
    pub(crate) runtime_session: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionHandle {
    session_id: u64,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StationEpochHandle {
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StationFaultHandle {
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionEvidence {
    pub(crate) transport: TransportEvidence,
    pub(crate) finished: Finished,
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
            next_session_id: AtomicU64::new(1),
            worker: Some(worker),
        }
    }

    /// Performs one typed host-to-target round trip and returns the current
    /// image capabilities. The old text readiness path remains active only as
    /// a compatibility oracle while benchmark evidence is migrated.
    pub(crate) fn request_capabilities(&self, timeout: Duration) -> Result<Capabilities> {
        let _hello = self
            .wait_for_protocol_after(0, timeout, |message| {
                matches!(message.body, Event::Hello(_))
            })
            .ok_or("device did not publish a HIL protocol hello")?;
        let response = self.send_command(0, Command::GetCapabilities, timeout)?;
        match response.body {
            Event::Hello(capabilities) => Ok(capabilities),
            Event::Rejected(reason) => {
                Err(format!("device rejected HIL capability request: {reason:?}").into())
            }
            _ => Err("device returned an invalid HIL capability response".into()),
        }
    }

    /// Establishes the typed link and provisions this boot from host-owned
    /// environment secrets. The passphrase is never echoed by the target or
    /// appended to the UART capture.
    pub(crate) fn prepare_protocol(&self) -> Result<Capabilities> {
        let capabilities = self.request_capabilities(PROTOCOL_READY_TIMEOUT)?;
        let artifact_path = startup_artifact::configured_path()?;
        if artifact_path.is_some() && !capabilities.features.startup_artifact {
            return Err("firmware does not support a host-owned startup artifact".into());
        }
        let artifact_event_start = self.protocol_event_count();
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path.as_deref()
            && let Some(bytes) = startup_artifact::load_if_present(path)?
        {
            self.upload_startup_artifact(&bytes, PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.network_provisioning {
            self.provision_network(PROTOCOL_READY_TIMEOUT)?;
        }
        if capabilities.features.startup_artifact
            && let Some(path) = artifact_path.as_deref()
        {
            let status = self.wait_for_startup_artifact_status_after(
                artifact_event_start,
                STARTUP_ARTIFACT_TIMEOUT,
            )?;
            let bytes = self
                .wait_for_startup_artifact_after(artifact_event_start, STARTUP_ARTIFACT_TIMEOUT)?;
            if usize::from(status.total_length) != bytes.len() {
                return Err(format!(
                    "startup artifact status length {} does not match {} returned bytes",
                    status.total_length,
                    bytes.len()
                )
                .into());
            }
            startup_artifact::persist_atomically(path, &bytes)?;
            eprintln!(
                "startup_artifact={} disposition={:?} bytes={} initialization_elapsed_us={}",
                path.display(),
                status.disposition,
                bytes.len(),
                status.initialization_elapsed_micros,
            );
        }
        Ok(capabilities)
    }

    fn wait_for_startup_artifact_status_after(
        &self,
        start: usize,
        timeout: Duration,
    ) -> Result<StartupArtifactStatus> {
        let event = self
            .wait_for_protocol_after(start, timeout, |message| {
                matches!(&message.body, Event::StartupArtifactReady(_))
            })
            .ok_or("device did not report startup artifact initialization status")?;
        match event.body {
            Event::StartupArtifactReady(status) => Ok(status),
            _ => unreachable!("startup artifact status predicate accepted only status events"),
        }
    }

    fn upload_startup_artifact(&self, bytes: &[u8], timeout: Duration) -> Result<()> {
        for chunk in startup_artifact::chunks(bytes)? {
            match self
                .send_command(0, Command::UploadStartupArtifact(chunk), timeout)?
                .body
            {
                Event::Accepted => {}
                Event::Rejected(reason) => {
                    return Err(format!("device rejected HIL startup artifact: {reason:?}").into());
                }
                _ => return Err("device returned an invalid startup artifact response".into()),
            }
        }
        Ok(())
    }

    fn wait_for_startup_artifact_after(&self, start: usize, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        let mut cursor = start;
        let mut assembler = startup_artifact::Assembler::new();
        loop {
            let chunk = self
                .wait_for_startup_artifact_chunk(&mut cursor, deadline)
                .ok_or("device did not return its startup artifact")?;
            if let Some(bytes) = assembler.push(&chunk)? {
                return Ok(bytes);
            }
        }
    }

    fn wait_for_startup_artifact_chunk(
        &self,
        cursor: &mut usize,
        deadline: Instant,
    ) -> Option<StartupArtifactChunk> {
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some((relative, chunk)) = messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find_map(|(relative, message)| match &message.body {
                    Event::StartupArtifact(chunk) => Some((relative, chunk.clone())),
                    _ => None,
                })
            {
                *cursor += relative + 1;
                return Some(chunk);
            }
            *cursor = messages.len();
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

    fn provision_network(&self, timeout: Duration) -> Result<()> {
        let ssid = network_environment("OPEN_RADIO_HIL_STA_SSID", "OPEN_RADIO_STA_SSID")?;
        let passphrase = Zeroizing::new(network_environment(
            "OPEN_RADIO_HIL_STA_PASSWORD",
            "OPEN_RADIO_STA_PASSWORD",
        )?);
        let credentials = NetworkCredentials::try_new(ssid.as_bytes(), passphrase.as_bytes())
            .map_err(|error| format!("invalid HIL network credentials: {error}"))?;
        let configuration = NetworkConfiguration {
            credentials,
            ipv4: network_ipv4_configuration()?,
        };
        let response = self.send_command(0, Command::ProvisionNetwork(configuration), timeout)?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected HIL network provisioning: {reason:?}").into())
            }
            _ => Err("device returned an invalid network provisioning response".into()),
        }
    }

    fn send_command(
        &self,
        session_id: u64,
        body: Command,
        timeout: Duration,
    ) -> Result<Envelope<Event>> {
        let boot_id = self
            .latest_boot_id()
            .ok_or("HIL protocol hello disappeared before command")?;
        let request_id = self.next_host_sequence.fetch_add(1, Ordering::Relaxed);
        let event_count = self.protocol_event_count();
        let command = Envelope::new(boot_id, request_id, session_id, request_id, body);
        let mut encoder = FrameEncoder::new();
        let frame = encoder
            .encode(&command)
            .map_err(|error| format!("cannot encode HIL command: {error}"))?
            .to_vec();
        self.outbound
            .send(Zeroizing::new(frame))
            .map_err(|_| "serial worker stopped before HIL command")?;
        self.wait_for_protocol_after(event_count, timeout, |message| {
            message.boot_id == boot_id && message.request_id == request_id
        })
        .ok_or_else(|| "device did not answer HIL command".into())
    }

    fn expect_accepted(&self, session_id: u64, command: Command, operation: &str) -> Result<()> {
        let response = self
            .send_command(session_id, command, PROTOCOL_READY_TIMEOUT)
            .map_err(|error| format!("session {operation} command failed: {error}"))?;
        match response.body {
            Event::Accepted => Ok(()),
            Event::Rejected(reason) => {
                Err(format!("device rejected session {operation}: {reason:?}").into())
            }
            _ => Err(format!("device returned an invalid session {operation} response").into()),
        }
    }

    pub(crate) fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let first_event = self.protocol_event_count();
        let direction = config.direction;
        self.expect_accepted(session_id, Command::Configure(config), "configuration")?;
        self.expect_accepted(session_id, Command::Arm, "arm")?;
        self.expect_accepted(session_id, Command::Start, "start")?;
        let expected_directions: &[Direction] = match direction {
            Direction::Rx => &[Direction::Rx],
            Direction::Tx => &[Direction::Tx],
            Direction::Bidirectional => &[Direction::Rx, Direction::Tx],
        };
        for expected in expected_directions {
            self.wait_for_protocol_after(first_event, PROTOCOL_READY_TIMEOUT, |message| {
                message.session_id == session_id
                    && matches!(message.body, Event::SessionReady(direction) if direction == *expected)
            })
            .ok_or_else(|| {
                format!(
                    "device did not publish {expected:?} data-plane readiness for session {session_id}"
                )
            })?;
        }
        Ok(SessionHandle {
            session_id,
            first_event,
        })
    }

    pub(crate) fn wait_for_session(
        &self,
        session: SessionHandle,
        timeout: Duration,
    ) -> Result<SessionEvidence> {
        let deadline = Instant::now() + timeout;
        let evidence = self
            .wait_for_protocol_after(session.first_event, timeout, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Evidence(EvidenceRecord::Transport(_)))
            })
            .ok_or("device did not publish structured session evidence")?;
        let transport = match evidence.body {
            Event::Evidence(EvidenceRecord::Transport(transport)) => transport,
            _ => unreachable!("session evidence predicate accepted only transport evidence"),
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let finished = self
            .wait_for_protocol_after(session.first_event, remaining, |message| {
                message.session_id == session.session_id
                    && matches!(message.body, Event::Finished(_))
            })
            .ok_or("device did not finish the structured HIL session")?;
        let finished = match finished.body {
            Event::Finished(finished) => finished,
            _ => unreachable!("session completion predicate accepted only Finished"),
        };
        if finished.summary.evidence_records != 1 {
            return Err(format!(
                "device reported {} evidence records, host received one",
                finished.summary.evidence_records
            )
            .into());
        }
        let record = EvidenceRecord::Transport(transport);
        let checksum = evidence_crc32c(core::slice::from_ref(&record))
            .map_err(|error| format!("cannot checksum structured HIL evidence: {error}"))?;
        if checksum != finished.evidence_crc32c {
            return Err(format!(
                "structured HIL evidence checksum mismatch: host={checksum:#010x} device={:#010x}",
                finished.evidence_crc32c
            )
            .into());
        }
        Ok(SessionEvidence {
            transport,
            finished,
        })
    }

    pub(crate) fn acknowledge_session(&self, session: SessionHandle) -> Result<()> {
        self.expect_accepted(
            session.session_id,
            Command::AcknowledgeResult,
            "acknowledgement",
        )
    }

    pub(crate) fn request_station_epoch_cycle(&self) -> Result<StationEpochHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(0, Command::CycleStationEpoch, PROTOCOL_READY_TIMEOUT)?;
        match response.body {
            Event::Accepted => Ok(StationEpochHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected station epoch cycle: {reason:?}").into())
            }
            _ => Err("device returned an invalid station epoch cycle response".into()),
        }
    }

    pub(crate) fn observed_station_epoch_completion(
        &self,
        handle: StationEpochHandle,
    ) -> Option<StationEpochEvidence> {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages
            .get(handle.first_event..)
            .unwrap_or_default()
            .iter()
            .find_map(|message| {
                if message.request_id != handle.request_id {
                    return None;
                }
                match message.body {
                    Event::StationEpochCompleted(evidence) => Some(evidence),
                    _ => None,
                }
            })
    }

    /// Arm one fault below the station facade and retain its correlation ID.
    pub(crate) fn request_station_fault_injection(
        &self,
        injection: StationFaultInjection,
    ) -> Result<StationFaultHandle> {
        let first_event = self.protocol_event_count();
        let response = self.send_command(
            0,
            Command::InjectStationFault(injection),
            PROTOCOL_READY_TIMEOUT,
        )?;
        match response.body {
            Event::Accepted => Ok(StationFaultHandle {
                request_id: response.request_id,
                first_event,
            }),
            Event::Rejected(reason) => {
                Err(format!("device rejected station fault injection: {reason:?}").into())
            }
            _ => Err("device returned an invalid station fault response".into()),
        }
    }

    /// Wait for the reliable owner frontier correlated with one fault command.
    pub(crate) fn wait_station_fault(
        &self,
        handle: StationFaultHandle,
        timeout: Duration,
    ) -> Result<StationFaultEvidence> {
        let event = self
            .wait_for_protocol_after(handle.first_event, timeout, |message| {
                message.request_id == handle.request_id
                    && matches!(message.body, Event::StationFault(_))
            })
            .ok_or("device did not publish the requested station fault frontier")?;
        match event.body {
            Event::StationFault(evidence) => Ok(evidence),
            _ => unreachable!("station fault predicate accepted only StationFault"),
        }
    }

    /// Cursor for reliable unsolicited station lifecycle events.
    pub(crate) fn station_lifecycle_cursor(&self) -> usize {
        self.protocol_event_count()
    }

    /// Wait for the next station lifecycle event and advance past it.
    pub(crate) fn wait_station_lifecycle_event(
        &self,
        cursor: &mut usize,
        timeout: Duration,
    ) -> Result<StationLifecycleEvent> {
        let deadline = Instant::now() + timeout;
        let mut messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some((relative, event)) = messages
                .get(*cursor..)
                .unwrap_or_default()
                .iter()
                .enumerate()
                .find_map(|(relative, message)| match &message.body {
                    Event::StationLifecycle(event) => Some((relative, *event)),
                    _ => None,
                })
            {
                *cursor += relative + 1;
                return Ok(event);
            }
            *cursor = messages.len();
            let now = Instant::now();
            if now >= deadline {
                return Err("device did not publish the next station lifecycle event".into());
            }
            let (next, result) = self
                .protocol
                .changed
                .wait_timeout(messages, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            messages = next;
            if result.timed_out() {
                return Err("device did not publish the next station lifecycle event".into());
            }
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
        self.observed_service(Transport::Udp, direction, port)
    }

    fn observed_service(&self, transport: Transport, direction: Direction, port: u16) -> bool {
        let messages = self
            .protocol
            .messages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        messages.iter().any(|message| match message.body {
            Event::ServiceReady(service) => {
                service.transport == transport
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

    pub(crate) fn marker_count(&self, marker: &str) -> usize {
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

/// Wait for a runtime-configured TCP receive service and its current IPv4
/// address. Unlike UDP, readiness does not inject a probe connection: the
/// target begins listening only after the session `Start` transition, and the
/// measured host connection is the sole stream owned by that session.
pub(crate) fn await_tcp_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    direction: Direction,
    timeout: Duration,
) -> Result<TcpReady> {
    let capabilities = capture.prepare_protocol()?;
    let direction_supported = match direction {
        Direction::Rx => capabilities.features.rx,
        Direction::Tx => capabilities.features.tx,
        Direction::Bidirectional => capabilities.features.bidirectional,
    };
    if !capabilities.features.tcp || !direction_supported {
        return Err(format!("firmware does not advertise TCP {direction:?} capability").into());
    }
    if !capabilities.features.runtime_configuration || !capabilities.features.structured_evidence {
        return Err("TCP RX requires runtime sessions and structured evidence".into());
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if capture.contains(RADIO_RUNNER_FAILURE_MARKER) {
            return Err("radio runner failed before TCP RX became ready".into());
        }
        let address = capture
            .observed_protocol_ipv4()
            .or_else(|| observed_network_ipv4(&capture.transcript()));
        if capture.observed_service(Transport::Tcp, direction, port)
            && let Some(address) = address
        {
            return Ok(TcpReady {
                address,
                runtime_session: true,
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device {address_hint}:{port} did not publish TCP {direction:?} readiness within {} seconds",
        timeout.as_secs(),
    )
    .into())
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

fn network_ipv4_configuration() -> Result<NetworkIpv4Configuration> {
    let address = std::env::var("OPEN_RADIO_HIL_STA_IPV4_CIDR").ok();
    let gateway = std::env::var("OPEN_RADIO_HIL_STA_GATEWAY_IPV4").ok();
    parse_network_ipv4_configuration(address.as_deref(), gateway.as_deref())
}

fn parse_network_ipv4_configuration(
    address: Option<&str>,
    gateway: Option<&str>,
) -> Result<NetworkIpv4Configuration> {
    let Some(address) = address else {
        if gateway.is_some() {
            return Err(
                "OPEN_RADIO_HIL_STA_GATEWAY_IPV4 requires OPEN_RADIO_HIL_STA_IPV4_CIDR".into(),
            );
        }
        return Ok(NetworkIpv4Configuration::Dhcp);
    };
    let (address, prefix_length) = address
        .split_once('/')
        .ok_or("OPEN_RADIO_HIL_STA_IPV4_CIDR must be an IPv4 CIDR")?;
    let address = address
        .parse::<Ipv4Addr>()
        .map_err(|error| format!("invalid OPEN_RADIO_HIL_STA_IPV4_CIDR address: {error}"))?
        .octets();
    let prefix_length = prefix_length
        .parse::<u8>()
        .map_err(|error| format!("invalid OPEN_RADIO_HIL_STA_IPV4_CIDR prefix: {error}"))?;
    let gateway = gateway
        .map(|gateway| {
            gateway
                .parse::<Ipv4Addr>()
                .map(|address| address.octets())
                .map_err(|error| {
                    format!("invalid OPEN_RADIO_HIL_STA_GATEWAY_IPV4 address: {error}")
                })
        })
        .transpose()?;
    let configuration = NetworkIpv4Configuration::Static {
        address,
        prefix_length,
        gateway,
    };
    if !configuration.validate() {
        return Err("invalid static HIL IPv4 configuration".into());
    }
    Ok(configuration)
}

/// Wait until the target owns its IPv4 address and UDP RX service.
///
/// Runtime-session images use a negative warm-up datagram that cannot open a
/// sample; `Arm`/`Start` provide the measured synchronization. Compatibility
/// images still use one positive datagram and the target idle timeout to prove
/// the complete UDP RX path before a measured stream begins.
pub(crate) fn await_udp_rx_ready(
    capture: &SerialCapture,
    address_hint: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Result<UdpRxReady> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.udp || !capabilities.features.rx {
        return Err("firmware does not advertise UDP RX capability".into());
    }
    if capabilities.features.runtime_configuration && !capabilities.features.structured_evidence {
        return Err("firmware advertises runtime sessions without structured evidence".into());
    }
    let runtime_session =
        capabilities.features.runtime_configuration && capabilities.features.structured_evidence;
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
            .or_else(|| observed_network_ipv4(&capture.transcript()))
            && discovered != address
        {
            address = discovered;
            socket.connect(SocketAddrV4::new(address, port))?;
        }
        let rx_service_ready = capture.observed_udp_service(Direction::Rx, port)
            || capture.contains("stage=udp-rx-ready");
        let tx_service_ready = !capabilities.features.bidirectional
            || capture.observed_udp_service(Direction::Tx, 4_324)
            || capture.contains("stage=udp-tx-ready");
        if !rx_service_ready
            || !tx_service_ready
            || capture
                .observed_protocol_ipv4()
                .or_else(|| observed_network_ipv4(&capture.transcript()))
                .is_none()
        {
            thread::sleep(Duration::from_millis(20));
            continue;
        }

        if runtime_session {
            // Resolve the host neighbor and exercise the complete Wi-Fi/IP/UDP
            // ingress before the measured interval. A negative sequence is a
            // terminal control datagram: the target drains it after `Start`
            // without opening or accounting an RX sample.
            packet[..4].copy_from_slice(&(-1_i32).to_be_bytes());
            socket.send(&packet)?;
            thread::sleep(RX_SESSION_WARMUP_SETTLE);
            return Ok(UdpRxReady {
                address,
                runtime_session: true,
            });
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
            return Ok(UdpRxReady {
                address,
                runtime_session: false,
            });
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
) -> Result<UdpTxReady> {
    let capabilities = capture.prepare_protocol()?;
    if !capabilities.features.udp || !capabilities.features.tx {
        return Err("firmware does not advertise UDP TX capability".into());
    }
    if capabilities.features.runtime_configuration && !capabilities.features.structured_evidence {
        return Err("firmware advertises runtime sessions without structured evidence".into());
    }
    let runtime_session =
        capabilities.features.runtime_configuration && capabilities.features.structured_evidence;
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
                    .or_else(|| observed_network_ipv4(&capture.transcript()))
                {
                    return Ok(UdpTxReady {
                        address,
                        runtime_session,
                    });
                }
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(UdpTxReady {
                address: address_hint,
                runtime_session,
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(format!(
        "device did not publish `{marker}` within {} seconds",
        timeout.as_secs(),
    )
    .into())
}

fn observed_network_ipv4(transcript: &str) -> Option<Ipv4Addr> {
    transcript.lines().rev().find_map(|line| {
        if !line.contains("stage=embassy-net-ready") {
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
    use super::{
        NetworkIpv4Configuration, observed_network_ipv4, parse_network_ipv4_configuration,
    };

    #[test]
    fn extracts_latest_network_address_from_uart_transcript() {
        let transcript = "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-ready address=192.168.178.120/24 gateway=None\n\
                          OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-ready address=192.168.178.121/24 gateway=None\n";

        assert_eq!(
            observed_network_ipv4(transcript),
            Some("192.168.178.121".parse().unwrap())
        );
    }

    #[test]
    fn parses_runtime_static_ipv4_configuration() {
        assert_eq!(
            parse_network_ipv4_configuration(Some("10.42.0.138/24"), Some("10.42.0.1")).unwrap(),
            NetworkIpv4Configuration::Static {
                address: [10, 42, 0, 138],
                prefix_length: 24,
                gateway: Some([10, 42, 0, 1]),
            }
        );
    }

    #[test]
    fn defaults_runtime_ipv4_configuration_to_dhcp() {
        assert_eq!(
            parse_network_ipv4_configuration(None, None).unwrap(),
            NetworkIpv4Configuration::Dhcp
        );
    }

    #[test]
    fn rejects_gateway_without_static_address() {
        assert!(parse_network_ipv4_configuration(None, Some("10.42.0.1")).is_err());
    }
}
