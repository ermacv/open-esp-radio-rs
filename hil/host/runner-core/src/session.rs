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
    DecodeCounters, Envelope, FrameDecoder, FrameEncoder, base::LinkHealth,
    ieee802154::Ieee802154AirCheckEvidence, ieee802154::Ieee802154AirCheckRequest,
    ieee802154::Ieee802154EdEventProbeEvidence, ieee802154::Ieee802154EdEventProbeRequest,
    ieee802154::Ieee802154EventStatusProbeEvidence, ieee802154::Ieee802154EventStatusProbeRequest,
    ieee802154::Ieee802154RouteProbeEvidence, ieee802154::Ieee802154RouteProbeRequest,
    ieee802154::Ieee802154SessionAssessRequest, ieee802154::Ieee802154SessionAssessment,
    ieee802154::Ieee802154SessionConfig, ieee802154::Ieee802154SessionPendingRequest,
    ieee802154::Ieee802154SessionPhyMaintenance, ieee802154::Ieee802154SessionReceiveEvidence,
    ieee802154::Ieee802154SessionRecentRssi, ieee802154::Ieee802154SessionRestartEvidence,
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154SessionStopEvidence,
    ieee802154::Ieee802154SessionTransmitEvidence, ieee802154::Ieee802154SessionTransmitRequest,
    ieee802154::Ieee802154ThreadReceiveEvidence, ieee802154::Ieee802154ThreadSendRequest,
    ieee802154::Ieee802154ThreadStartRequest, ieee802154::Ieee802154ThreadState,
    network::Direction, network::EvidenceRecord, network::Finished, network::FlowTransportEvidence,
    network::NetworkSchedulerEvidence, network::OperationStatus, network::RadioEvidence,
    network::RxDeliveryEvidence, network::RxRadioEvidence, network::RxZeroCopyEvidence,
    network::SESSION_FLOW_CAPACITY, network::SessionConfig, network::SessionLinkRequirements,
    network::SessionReady, network::Transport, network::TransportEvidence,
    network::TxAggregateTimingEvidence, network::TxRadioEvidence, network::evidence_crc32c,
    phy::StartupArtifactChunk, phy::StartupArtifactStatus, system::StackUsage,
    system::TimebaseProbeEvidence, system::TimebaseProbeRequest, wifi::StationEpochEvidence,
    wifi::StationLifecycleEvent, wifi::WifiMonitorCaptureRequest, wifi::WifiMonitorEvidence,
    wifi::WifiMonitorFrameChunk, wifi::WifiMonitorRequest, wifi::WifiNetworkInterface,
    wifi::WifiRadioRestartEvidence, wifi::WifiRoleTransitionEvidence, wifi::WifiScanEvidence,
    wifi::WifiScanRequest,
};
use zeroize::Zeroizing;

use crate::Result;
mod airtime;
mod target;
pub use target::{Settings, Target};
mod reboot;
use reboot::{ExpectedReboot, RebootObservation};
mod received;
pub use received::{DeviceImageKeys, Received, message_info};

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
    messages: Vec<Received>,
    /// When the host decoded each of `messages`, in Unix microseconds.
    received_unix_micros: Vec<u64>,
    /// Every command the host sent, in order.
    sent: Vec<SentCommand>,
    health: ProtocolHealth,
    failure: Option<LinkError>,
    closed: bool,
}

/// A command the host sent: its identity and when, never its payload, which
/// can carry credentials.
#[derive(serde::Serialize)]
struct SentCommand {
    request_id: u32,
    session_id: u64,
    /// The command's message path.
    path: &'static str,
    host_sent_unix_micros: u64,
}

/// Microseconds since the Unix epoch on the host clock.
fn host_unix_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros() as u64)
}

/// What a sent message was: its path.
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
    /// The boot began with the answer to the host's capability request,
    /// because the link lost the boot's own Hello.
    #[serde(skip_serializing_if = "Option::is_none")]
    solicited_hello: Option<SolicitedHello>,
    /// Whether a boot may begin with a solicited Hello; set only while the
    /// host asks a boot whose Hello it missed.
    #[serde(skip)]
    accept_solicited_hello: bool,
}

/// A boot that began with the answer to the host's capability request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
struct SolicitedHello {
    /// The answer's target message sequence: the boot's messages before it
    /// were lost on the link.
    message_sequence: u32,
}

/// The latest target message sequence a solicited Hello may begin a boot
/// at: the boot's own Hello and the few messages a boot sends unasked.
const SOLICITED_HELLO_SEQUENCE_LIMIT: u32 = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum CaptureOrigin {
    #[default]
    Boot,
    Attachment,
}

impl ProtocolHealth {
    fn observe(&mut self, message: &Received, decoder_counters: DecodeCounters) {
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

    fn begin_boot(&mut self, message: &Received, decoder_counters: DecodeCounters) {
        self.active = true;
        self.boot_id = Some(message.boot_id);
        self.next_sequence = message.message_sequence.wrapping_add(1);
        self.decoder_baseline = decoder_counters;
        self.counters = DecodeCounters::default();
        self.failure = None;
        if message.boot_id == 0 {
            self.fail("target published a reserved zero boot identity".into());
        } else if self.origin == CaptureOrigin::Boot
            && (message.message_sequence != 0 || !message.is::<oer_hil_protocol::base::Hello>())
        {
            if self.accept_solicited_hello
                && message.is::<oer_hil_protocol::base::Hello>()
                && message.request_id != 0
                && message.message_sequence <= SOLICITED_HELLO_SEQUENCE_LIMIT
            {
                self.solicited_hello = Some(SolicitedHello {
                    message_sequence: message.message_sequence,
                });
                return;
            }
            self.fail(format!(
                "boot {} began with {} at target message sequence {}, expected Hello at 0",
                message.boot_id,
                message.path(),
                message.message_sequence
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
    /// The armed program-counter profile, drained when the capture finishes.
    profile: Option<crate::scenario::ProfileRequest>,
}

fn command_response_matches(
    message: &Received,
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
    pub security: Option<oer_hil_protocol::wifi::StationLinkSecurity>,
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
    pub rx_zero_copy: Option<RxZeroCopyEvidence>,
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

use oer_hil_board::reset::reset_usb_serial_jtag;
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
