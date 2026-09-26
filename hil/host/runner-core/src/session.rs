//! Shared UART capture and end-to-end readiness probes for traffic HIL cells.

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use oer_hil_protocol::{
    Capabilities, Command, DecodeCounters, Direction, Envelope, Event, EvidenceRecord, Finished,
    FlowTransportEvidence, FrameDecoder, FrameEncoder, Ieee802154AirCheckEvidence,
    Ieee802154AirCheckRequest, Ieee802154EdEventProbeEvidence, Ieee802154EdEventProbeRequest,
    Ieee802154EventStatusProbeEvidence, Ieee802154EventStatusProbeRequest, LinkHealth,
    NetworkSchedulerEvidence, OperationStatus, RadioEvidence, RxDeliveryEvidence, RxRadioEvidence,
    SESSION_FLOW_CAPACITY, SessionConfig, SessionLinkRequirements, SessionReady, SessionState,
    StackUsage, StartupArtifactChunk, StartupArtifactStatus, StateChange, StationEpochEvidence,
    StationLifecycleEvent, TimebaseProbeEvidence, TimebaseProbeRequest, Transport,
    TransportEvidence, TxAggregateTimingEvidence, TxRadioEvidence, WifiMonitorCaptureRequest,
    WifiMonitorEvidence, WifiMonitorFrameChunk, WifiMonitorRequest, WifiNetworkInterface,
    WifiRadioRestartEvidence, WifiRadioRetainedCycleEvidence, WifiRoleTransitionEvidence,
    WifiScanEvidence, WifiScanRequest, evidence_crc32c,
};
use zeroize::Zeroizing;

use crate::Result;
mod airtime;
mod target;
pub use target::{Settings, Target};
mod reboot;
use reboot::{ExpectedReboot, RebootObservation};

const RX_PROBE_PAYLOAD: usize = 64;
const RX_PROBE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
const PROTOCOL_READY_TIMEOUT: Duration = Duration::from_secs(10);
// Configure, Arm, Start and up to two directional SessionReady waits. The
// collector runs before these operations and must cover their failure bounds.
pub const SESSION_START_TIMEOUT: Duration = PROTOCOL_READY_TIMEOUT.saturating_mul(5);
const STARTUP_ARTIFACT_TIMEOUT: Duration = Duration::from_secs(30);
const SERIAL_OPEN_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const SERIAL_OPEN_BUSY_RETRY: Duration = Duration::from_millis(50);
const PROTOCOL_EVENT_CAPACITY: usize = 16_384;

#[derive(Default)]
struct ProtocolEvents {
    state: Mutex<ProtocolState>,
    changed: Condvar,
}

#[derive(Default)]
struct ProtocolState {
    expected_reboot: Option<ExpectedReboot>,
    observed_reboot: Option<RebootObservation>,
    messages: Vec<Envelope<Event>>,
    health: ProtocolHealth,
    failure: Option<LinkError>,
    closed: bool,
}

impl ProtocolState {
    fn check(&self) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone().into());
        }
        if self.closed {
            return Err(LinkError::transport("serial capture is closed").into());
        }
        Ok(())
    }

    fn fail(&mut self, error: LinkError) {
        // A later reset or teardown must not replace the original cause.
        self.failure.get_or_insert(error);
    }
}

impl ProtocolEvents {
    fn close(&self, error: Option<LinkError>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(error) = error {
            state.fail(error);
        }
        state.closed = true;
        self.changed.notify_all();
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
struct ProtocolHealth {
    origin: CaptureOrigin,
    active: bool,
    boot_id: Option<u64>,
    next_sequence: u32,
    counters: DecodeCounters,
    #[serde(skip)]
    decoder_baseline: DecodeCounters,
    failure: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CaptureOrigin {
    #[default]
    Boot,
    Attachment,
}

impl ProtocolHealth {
    fn observe(&mut self, message: &Envelope<Event>, decoder_counters: DecodeCounters) {
        match self.boot_id {
            None => {
                self.begin_boot(message, decoder_counters);
            }
            Some(boot_id) if boot_id != message.boot_id => {
                self.begin_boot(message, decoder_counters);
            }
            Some(_) if message.message_sequence != self.next_sequence => {
                let expected = self.next_sequence;
                self.next_sequence = message.message_sequence.wrapping_add(1);
                self.fail(format!(
                    "target message sequence discontinuity: expected {expected}, observed {}",
                    message.message_sequence
                ));
            }
            Some(_) => self.next_sequence = self.next_sequence.wrapping_add(1),
        }
    }

    fn begin_boot(&mut self, message: &Envelope<Event>, decoder_counters: DecodeCounters) {
        self.active = true;
        self.boot_id = Some(message.boot_id);
        self.next_sequence = message.message_sequence.wrapping_add(1);
        self.decoder_baseline = decoder_counters;
        self.counters = DecodeCounters::default();
        self.failure = None;
        if message.boot_id == 0 {
            self.fail("target published a reserved zero boot identity".into());
        } else if self.origin == CaptureOrigin::Boot
            && (message.message_sequence != 0 || !matches!(message.body, Event::Hello(_)))
        {
            self.fail(format!(
                "boot {} began with {:?} at target message sequence {}, expected Hello at 0",
                message.boot_id, message.body, message.message_sequence
            ));
        }
    }

    fn update_decoder_counters(&mut self, totals: DecodeCounters) {
        self.counters = decode_counter_delta(totals, self.decoder_baseline);
    }

    fn decode_error(&mut self, error: impl std::fmt::Display) {
        if self.active {
            self.fail(format!(
                "HIL wire decode failure after protocol activation: {error}"
            ));
        }
    }

    fn fail(&mut self, failure: String) {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
    }
}

fn decode_counter_delta(totals: DecodeCounters, baseline: DecodeCounters) -> DecodeCounters {
    DecodeCounters {
        frames: totals.frames.saturating_sub(baseline.frames),
        cobs_errors: totals.cobs_errors.saturating_sub(baseline.cobs_errors),
        too_short: totals.too_short.saturating_sub(baseline.too_short),
        header_errors: totals.header_errors.saturating_sub(baseline.header_errors),
        framing_version_errors: totals
            .framing_version_errors
            .saturating_sub(baseline.framing_version_errors),
        message_kind_errors: totals
            .message_kind_errors
            .saturating_sub(baseline.message_kind_errors),
        protocol_version_errors: totals
            .protocol_version_errors
            .saturating_sub(baseline.protocol_version_errors),
        payload_length_errors: totals
            .payload_length_errors
            .saturating_sub(baseline.payload_length_errors),
        checksum_errors: totals
            .checksum_errors
            .saturating_sub(baseline.checksum_errors),
        deserialize_errors: totals
            .deserialize_errors
            .saturating_sub(baseline.deserialize_errors),
        overflows: totals.overflows.saturating_sub(baseline.overflows),
    }
}

/// Concurrent UART transcript retained across traffic setup and measurement.
pub struct SerialCapture {
    stop: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
    protocol: Arc<ProtocolEvents>,
    outbound: mpsc::Sender<Zeroizing<Vec<u8>>>,
    worker_wake: Arc<mio::Waker>,
    _cancellation: oer_process::CancellationNotification,
    next_host_sequence: AtomicU32,
    next_session_id: AtomicU64,
    worker: Option<thread::JoinHandle<()>>,
    output: PathBuf,
    persisted: bool,
    measurements: Option<crate::evidence::measurements::CaptureRecorder>,
}

fn command_response_matches(
    message: &Envelope<Event>,
    boot_id: u64,
    session_id: u64,
    request_id: u32,
) -> bool {
    message.boot_id == boot_id
        && message.session_id == session_id
        && message.request_id == request_id
}

#[derive(Clone, Copy, Debug)]
pub struct UdpRxReady {
    pub address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub struct UdpTxReady {
    pub address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub struct TcpReady {
    pub address: Ipv4Addr,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionHandle {
    session_id: u64,
    first_event: usize,
    flow_ids: [Option<u8>; SESSION_FLOW_CAPACITY],
}

#[derive(Clone, Copy, Debug)]
pub struct StationEpochHandle {
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct WifiCommandHandle {
    boot_id: u64,
    request_id: u32,
    first_event: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationConnectionObservation {
    pub generation: u32,
    pub association_bandwidth_mhz: Option<u16>,
    pub security: Option<oer_hil_protocol::StationLinkSecurity>,
    pub event_cursor_after: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionEvidence {
    pub transport: TransportEvidence,
    pub flow_transport: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    pub radio: Option<RadioEvidence>,
    pub tx_timing: Option<TxAggregateTimingEvidence>,
    pub rx_delivery: Option<RxDeliveryEvidence>,
    pub network_scheduler: Option<NetworkSchedulerEvidence>,
    pub stack: StackUsage,
    pub link: LinkHealth,
    pub finished: Finished,
}

pub struct MonitorCaptureEvidence {
    pub chunks: Vec<WifiMonitorFrameChunk>,
    pub summary: WifiMonitorEvidence,
}

fn open_serial_after_busy_release(port: &Path) -> serialport::Result<serialport::TTYPort> {
    let deadline = Instant::now() + SERIAL_OPEN_BUSY_TIMEOUT;
    loop {
        match serialport::new(port.to_string_lossy(), 115_200)
            .preserve_dtr_on_open()
            .open_native()
        {
            Ok(serial) => return Ok(serial),
            Err(error)
                if error.kind == serialport::ErrorKind::NoDevice
                    && port.exists()
                    && Instant::now() < deadline =>
            {
                thread::sleep(SERIAL_OPEN_BUSY_RETRY);
            }
            Err(error) => return Err(error),
        }
    }
}

impl Drop for SerialCapture {
    fn drop(&mut self) {
        self.stop_and_join();
        if !self.persisted
            && let Err(error) = self.persist(None, false)
        {
            eprintln!("cannot preserve partial serial capture: {error}");
        }
    }
}

fn append(bytes: &Mutex<Vec<u8>>, chunk: &[u8]) {
    bytes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend_from_slice(chunk);
}

mod reset;
use reset::reset_usb_serial_jtag;
mod capture;
#[cfg(any(test, feature = "test-support"))]
pub use capture::test_support;
pub mod error;
use error::LinkError;
mod protocol;
mod readiness;
#[cfg(test)]
mod tests;
mod validation;

#[cfg(test)]
use protocol::beacon_loss_count_in;
use readiness::session_ready_covers;
pub use readiness::{
    await_network_ready, await_tcp_ready, await_udp_rx_ready, await_udp_tx_ready,
    prepare_udp_reverse_flow, probe_udp_rx_ready, probe_udp_rx_ready_via,
};
use validation::validate_stack_usage;

pub mod startup_artifact;
