//! Early-console and runtime logging backend.
//!
//! Application code should use the macros from the `log` crate. Direct ROM
//! output remains available for the boot and panic paths, where the executor
//! and the asynchronous logging transport may not be running yet.

use core::{
    fmt::{Arguments, Write},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::{
    select::{Either, Either3, select, select3},
    yield_now,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embedded_io_async::{Read as _, Write as _};
use esp_hal::{
    Async,
    peripherals::USB_DEVICE,
    usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagTx},
};
use open_esp_radio::esp32s31::phy::phy_cold::PHY_COLD_CALIBRATION_RECORD_LEN;
use open_esp_radio_hil_protocol::{
    Capabilities, Command, Completion, Direction, Envelope, Event, EvidenceRecord, Finished,
    FrameDecoder, FrameEncoder, NetworkConfiguration, PROTOCOL_VERSION, RejectReason,
    ResultSummary, STARTUP_ARTIFACT_CHUNK_MAX_LEN, SessionConfig, SessionState,
    StartupArtifactChunk, StartupArtifactDisposition, StartupArtifactStatus, StateChange,
    StationEpochEvidence, StationFaultEvidence, StationLifecycleEvent, Transport,
    TransportEvidence, evidence_crc32c, startup_artifact_crc32c,
};

const MESSAGE_CAPACITY: usize = 384;
const QUEUE_CAPACITY: usize = 8;
const DRAIN_BATCH: usize = 4;
const COMMAND_QUEUE_CAPACITY: usize = 4;
const EVENT_QUEUE_CAPACITY: usize = 8;
const USB_RX_CHUNK_BYTES: usize = 128;

#[unsafe(link_section = ".critical.data.logging")]
static WRITER_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_WRITER_WAITING: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.data.logging")]
static RUNTIME_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.data.logging")]
static DROPPED_RECORDS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static TRUNCATED_RECORDS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static BOOT_ID_LOW: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static BOOT_ID_HIGH: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static EVENT_SEQUENCE: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_DROPPED: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_TX_FRAMES: AtomicU32 = AtomicU32::new(0);
/// One plus the last event sequence fully written to the USB endpoint.
/// Zero means that no event from the current boot has crossed that boundary.
#[unsafe(link_section = ".critical.data.logging")]
static SERIALIZED_STATION_LIFECYCLE_NEXT: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static RECORDS: Channel<CriticalSectionRawMutex, TextBuffer<MESSAGE_CAPACITY>, QUEUE_CAPACITY> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static COMMANDS: Channel<CriticalSectionRawMutex, Envelope<Command>, COMMAND_QUEUE_CAPACITY> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static EVENTS: Channel<CriticalSectionRawMutex, Envelope<Event>, EVENT_QUEUE_CAPACITY> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static STARTUP_CONFIGURATIONS: Channel<CriticalSectionRawMutex, StartupConfiguration, 1> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_STARTS: Channel<CriticalSectionRawMutex, ActiveSession, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_RESULTS: Channel<CriticalSectionRawMutex, SessionResult, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static STATION_EPOCH_CYCLES: Channel<CriticalSectionRawMutex, u32, 1> = Channel::new();

#[derive(Clone, Copy)]
pub struct ActiveSession {
    pub session_id: u64,
    pub config: SessionConfig,
}

pub struct StartupConfiguration {
    pub network: NetworkConfiguration,
    pub phy_calibration_record: Option<[u8; PHY_COLD_CALIBRATION_RECORD_LEN]>,
}

struct StartupArtifactAssembler {
    bytes: [u8; PHY_COLD_CALIBRATION_RECORD_LEN],
    expected_total: Option<u16>,
    expected_crc32c: u32,
    received: usize,
    complete: bool,
}

impl StartupArtifactAssembler {
    const fn new() -> Self {
        Self {
            bytes: [0; PHY_COLD_CALIBRATION_RECORD_LEN],
            expected_total: None,
            expected_crc32c: 0,
            received: 0,
            complete: false,
        }
    }

    fn push(&mut self, chunk: &StartupArtifactChunk) -> Result<(), ()> {
        chunk.validate().map_err(|_| ())?;
        if usize::from(chunk.total_length()) != self.bytes.len() {
            return Err(());
        }
        if chunk.offset() == 0 {
            self.expected_total = Some(chunk.total_length());
            self.expected_crc32c = chunk.crc32c();
            self.received = 0;
            self.complete = false;
        }
        if self.complete
            || self.expected_total != Some(chunk.total_length())
            || self.expected_crc32c != chunk.crc32c()
            || usize::from(chunk.offset()) != self.received
        {
            return Err(());
        }
        let end = self.received + chunk.bytes().len();
        self.bytes[self.received..end].copy_from_slice(chunk.bytes());
        self.received = end;
        if chunk.is_final() {
            if self.received != self.bytes.len()
                || startup_artifact_crc32c(&self.bytes) != self.expected_crc32c
            {
                self.expected_total = None;
                self.received = 0;
                return Err(());
            }
            self.complete = true;
        }
        Ok(())
    }

    fn started_but_incomplete(&self) -> bool {
        self.expected_total.is_some() && !self.complete
    }

    fn completed_record(&self) -> Option<[u8; PHY_COLD_CALIBRATION_RECORD_LEN]> {
        self.complete.then_some(self.bytes)
    }
}

#[derive(Clone, Copy)]
struct SessionResult {
    session_id: u64,
    evidence: TransportEvidence,
    passed: bool,
}

unsafe extern "C" {
    fn ets_printf(format: *const u8, ...) -> i32;
}

struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> TextBuffer<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }

    fn as_c_string(&self) -> *const u8 {
        self.bytes.as_ptr()
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn was_truncated(&self) -> bool {
        self.truncated
    }
}

impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len() - 1 - self.len;
        let length = text.len().min(available);
        self.bytes[self.len..self.len + length].copy_from_slice(&text.as_bytes()[..length]);
        self.len += length;
        self.truncated |= length != text.len();
        Ok(())
    }
}

/// Reports the minimum architectural state needed to diagnose a panic.
///
/// This deliberately bypasses the global logger: a panic can happen while the
/// logger is already active, before it is installed, or while normal memory
/// and executor services are unavailable.
#[unsafe(link_section = ".rwtext.logging")]
pub fn panic_report(mcause: usize, mepc: usize, mtval: usize) {
    unsafe {
        ets_printf(
            c"panic mcause=%08x mepc=%08x mtval=%08x\r\n"
                .as_ptr()
                .cast(),
            mcause,
            mepc,
            mtval,
        );
    }
}

/// Formats and writes one emergency line immediately.
///
/// This bypasses the queue and is intended only for early boot, panic, and
/// last-resort diagnostics.
pub fn emergency_log(args: Arguments<'_>) {
    write_line_immediate(args);
}

/// Returns the number of records discarded because the queue was full or
/// another core/interrupt was already using the immediate writer.
pub fn dropped_records() -> u32 {
    DROPPED_RECORDS.load(Ordering::Relaxed)
}

/// Returns the number of records whose text exceeded [`MESSAGE_CAPACITY`].
pub fn truncated_records() -> u32 {
    TRUNCATED_RECORDS.load(Ordering::Relaxed)
}

/// Installs the identity of the current boot before protocol tasks start.
pub fn init_protocol(boot_id: u64) {
    BOOT_ID_LOW.store(boot_id as u32, Ordering::Relaxed);
    BOOT_ID_HIGH.store((boot_id >> 32) as u32, Ordering::Release);
    EVENT_SEQUENCE.store(0, Ordering::Relaxed);
    SERIALIZED_STATION_LIFECYCLE_NEXT.store(0, Ordering::Relaxed);
}

fn boot_id() -> u64 {
    u64::from(BOOT_ID_LOW.load(Ordering::Acquire))
        | (u64::from(BOOT_ID_HIGH.load(Ordering::Acquire)) << 32)
}

/// Queues a typed event without making a radio or network task wait for USB.
pub fn publish_event(session_id: u64, request_id: u32, body: Event) {
    let message_sequence = EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let event = Envelope::new(boot_id(), message_sequence, session_id, request_id, body);
    if EVENTS.try_send(event).is_err() {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Waits without polling until the host provisions this boot's complete
/// startup configuration.
pub async fn receive_startup_configuration() -> StartupConfiguration {
    STARTUP_CONFIGURATIONS.receive().await
}

/// Returns the current target-defined startup artifact to the host in bounded
/// wire frames. This runs before traffic measurement and never writes flash or
/// NVS on the target.
pub async fn publish_startup_artifact(
    disposition: StartupArtifactDisposition,
    initialization_elapsed_micros: u64,
    bytes: &[u8],
) {
    let Ok(total_length) = u16::try_from(bytes.len()) else {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let checksum = startup_artifact_crc32c(bytes);
    publish_event_reliably(
        0,
        0,
        Event::StartupArtifactReady(StartupArtifactStatus {
            disposition,
            total_length,
            initialization_elapsed_micros,
        }),
    )
    .await;
    for (index, part) in bytes.chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN).enumerate() {
        let offset = index * STARTUP_ARTIFACT_CHUNK_MAX_LEN;
        let Ok(offset) = u16::try_from(offset) else {
            PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let Ok(chunk) = StartupArtifactChunk::try_new(total_length, offset, checksum, part) else {
            PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        };
        publish_event_reliably(0, 0, Event::StartupArtifact(chunk)).await;
    }
}

/// Waits until the host has configured, armed, and started one benchmark.
pub async fn receive_session_start() -> ActiveSession {
    SESSION_STARTS.receive().await
}

/// Wait until the host requests one finite connected-STA lifecycle cycle.
///
/// The production runner observes this only at a hardware-safe transaction
/// boundary; the console owns command admission and never touches radio state.
pub async fn receive_station_epoch_cycle() -> u32 {
    STATION_EPOCH_CYCLES.receive().await
}

/// Reliably acknowledge a completed target-side station ownership cycle.
///
/// Unlike text diagnostics, this event is serialized by the protocol owner
/// and retains the command request ID used by the host qualifier.
pub async fn complete_station_epoch_cycle(request_id: u32, evidence: StationEpochEvidence) {
    publish_event_reliably(0, request_id, Event::StationEpochCompleted(evidence)).await;
}

/// Reliably publish one unsolicited station generation/link edge.
///
/// The caller emits this only after the corresponding ownership transition;
/// unlike UART text, it cannot be dropped under diagnostic pressure.
pub async fn publish_station_lifecycle(event: StationLifecycleEvent) {
    let sequence = queue_event_reliably(0, 0, Event::StationLifecycle(event)).await;
    let target = sequence.wrapping_add(1);
    // A lifecycle edge is qualification evidence, not a best-effort trace.
    // Queue admission alone is insufficient at a terminal station exit: the
    // producing task may return before the independent USB worker runs again.
    while SERIALIZED_STATION_LIFECYCLE_NEXT.load(Ordering::Acquire) != target {
        yield_now().await;
    }
}

/// Reliably publish the exact terminal owner frontier of one requested fault.
pub async fn publish_station_fault(request_id: u32, evidence: StationFaultEvidence) {
    publish_event_reliably(0, request_id, Event::StationFault(evidence)).await;
}

/// Hands a completed in-memory measurement back to the protocol owner.
///
/// USB serialization happens in another task and therefore cannot extend the
/// benchmark's measured interval.
pub async fn complete_session(session_id: u64, evidence: TransportEvidence, passed: bool) {
    SESSION_RESULTS
        .send(SessionResult {
            session_id,
            evidence,
            passed,
        })
        .await;
}

/// Queue a control-plane event without allowing a full telemetry queue to
/// erase a required host/target state transition.
///
/// Use this for readiness and lifecycle boundaries outside measured traffic.
/// High-rate observations should continue to use [`publish_event`] so they
/// cannot apply backpressure to the radio or network hot path.
pub(crate) async fn publish_event_reliably(session_id: u64, request_id: u32, body: Event) {
    let _ = queue_event_reliably(session_id, request_id, body).await;
}

async fn queue_event_reliably(session_id: u64, request_id: u32, body: Event) -> u32 {
    let message_sequence = EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    EVENTS
        .send(Envelope::new(
            boot_id(),
            message_sequence,
            session_id,
            request_id,
            body,
        ))
        .await;
    message_sequence
}

/// Owns commands while individual benchmark services migrate to
/// runtime-configured sessions. Unsupported mutations receive an explicit
/// response instead of being silently ignored.
#[embassy_executor::task]
pub async fn protocol_task(capabilities: Capabilities) {
    publish_event_reliably(0, 0, Event::Hello(capabilities)).await;
    publish_event_reliably(
        0,
        0,
        Event::State(StateChange {
            previous: SessionState::Booting,
            current: SessionState::WaitingForNetwork,
        }),
    )
    .await;
    let mut network_provisioned = false;
    let mut state = SessionState::WaitingForNetwork;
    let mut configured = None::<ActiveSession>;
    let mut last_result = None::<SessionResult>;
    let mut startup_artifact = StartupArtifactAssembler::new();
    loop {
        match select(COMMANDS.receive(), SESSION_RESULTS.receive()).await {
            Either::First(command) => {
                let session_id = command.session_id;
                let request_id = command.request_id;
                match command.body {
                    Command::GetCapabilities => {
                        publish_event_reliably(session_id, request_id, Event::Hello(capabilities))
                            .await;
                    }
                    Command::UploadStartupArtifact(chunk) => {
                        let response = if !capabilities.features.startup_artifact {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if network_provisioned || state != SessionState::WaitingForNetwork {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if startup_artifact.push(&chunk).is_err() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::ProvisionNetwork(network) => {
                        let (response, accepted) = if network_provisioned {
                            (Event::Rejected(RejectReason::InvalidState), false)
                        } else if network.validate().is_err() {
                            (Event::Rejected(RejectReason::InvalidConfiguration), false)
                        } else if startup_artifact.started_but_incomplete() {
                            (Event::Rejected(RejectReason::InvalidConfiguration), false)
                        } else if STARTUP_CONFIGURATIONS
                            .try_send(StartupConfiguration {
                                network,
                                phy_calibration_record: startup_artifact.completed_record(),
                            })
                            .is_err()
                        {
                            (Event::Rejected(RejectReason::Busy), false)
                        } else {
                            network_provisioned = true;
                            (Event::Accepted, true)
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                        if accepted {
                            transition_state(&mut state, SessionState::Idle, 0).await;
                        }
                    }
                    Command::Configure(config) => {
                        let rejection = if !capabilities.features.runtime_configuration {
                            Some(RejectReason::Unsupported)
                        } else if !network_provisioned || state != SessionState::Idle {
                            Some(RejectReason::InvalidState)
                        } else if session_id == 0 {
                            Some(RejectReason::SessionId)
                        } else if !valid_session_config(config, capabilities) {
                            Some(RejectReason::InvalidConfiguration)
                        } else {
                            None
                        };
                        if let Some(reason) = rejection {
                            publish_event_reliably(session_id, request_id, Event::Rejected(reason))
                                .await;
                        } else {
                            configured = Some(ActiveSession { session_id, config });
                            last_result = None;
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(&mut state, SessionState::Configured, session_id)
                                .await;
                        }
                    }
                    Command::Arm => {
                        if state != SessionState::Configured
                            || configured.is_none_or(|session| session.session_id != session_id)
                        {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        } else {
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(&mut state, SessionState::Armed, session_id).await;
                        }
                    }
                    Command::Start => {
                        let session = configured.filter(|configured| {
                            state == SessionState::Armed && configured.session_id == session_id
                        });
                        if let Some(session) = session {
                            if SESSION_STARTS.try_send(session).is_err() {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    Event::Rejected(RejectReason::Busy),
                                )
                                .await;
                            } else {
                                publish_event_reliably(session_id, request_id, Event::Accepted)
                                    .await;
                                transition_state(&mut state, SessionState::Running, session_id)
                                    .await;
                            }
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::Abort => {
                        if matches!(state, SessionState::Configured | SessionState::Armed)
                            && configured.is_some_and(|session| session.session_id == session_id)
                        {
                            configured = None;
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(&mut state, SessionState::Idle, session_id).await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::GetLastResult => {
                        if let Some(result) =
                            last_result.filter(|result| result.session_id == session_id)
                        {
                            publish_result(result, request_id).await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::AcknowledgeResult => {
                        if state == SessionState::Finished
                            && last_result.is_some_and(|result| result.session_id == session_id)
                        {
                            configured = None;
                            last_result = None;
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(&mut state, SessionState::Idle, session_id).await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::CycleStationEpoch => {
                        let response = if !capabilities.features.station_epoch_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !network_provisioned
                            || state != SessionState::Idle
                            || session_id != 0
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if STATION_EPOCH_CYCLES.try_send(request_id).is_err() {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::InjectStationFault(injection) => {
                        let response = if !capabilities.features.station_fault_injection {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !network_provisioned
                            || state != SessionState::Idle
                            || session_id != 0
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if !crate::radio_fault::STATION_FAULT_CONTROL
                            .try_arm(request_id, injection)
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::Stop => {
                        publish_event_reliably(
                            session_id,
                            request_id,
                            Event::Rejected(RejectReason::Unsupported),
                        )
                        .await;
                    }
                }
            }
            Either::Second(result) => {
                if state == SessionState::Running
                    && configured.is_some_and(|session| session.session_id == result.session_id)
                {
                    transition_state(&mut state, SessionState::Draining, result.session_id).await;
                    publish_result(result, 0).await;
                    last_result = Some(result);
                    transition_state(&mut state, SessionState::Finished, result.session_id).await;
                } else {
                    publish_event_reliably(
                        result.session_id,
                        0,
                        Event::Rejected(RejectReason::InvalidState),
                    )
                    .await;
                }
            }
        }
    }
}

fn valid_session_config(config: SessionConfig, capabilities: Capabilities) -> bool {
    let valid_flow = |flow: open_esp_radio_hil_protocol::FlowConfig| {
        flow.payload_bytes >= 64
            && flow.payload_bytes <= capabilities.maximum_payload_bytes
            && flow
                .offered_rate_bps
                .is_none_or(|rate| (100_000..=1_000_000_000).contains(&rate))
    };
    let peer_valid = match (config.transport, config.direction) {
        (Transport::Tcp, _) | (Transport::Udp, Direction::Rx) => config.peer.is_none(),
        (Transport::Udp, Direction::Tx | Direction::Bidirectional) => {
            config.peer.is_some_and(|peer| peer.port != 0)
        }
    };
    let direction_valid = match config.direction {
        Direction::Rx => {
            capabilities.features.rx
                && config.target_rx.is_some_and(valid_flow)
                && config.target_tx.is_none()
        }
        Direction::Tx => {
            capabilities.features.tx
                && config.target_rx.is_none()
                && config.target_tx.is_some_and(valid_flow)
        }
        Direction::Bidirectional => {
            capabilities.features.bidirectional
                && capabilities.features.rx
                && capabilities.features.tx
                && config.target_rx.is_some_and(valid_flow)
                && config.target_tx.is_some_and(valid_flow)
        }
    };
    let transport_valid = match config.transport {
        Transport::Udp => capabilities.features.udp,
        Transport::Tcp => capabilities.features.tcp,
    };
    transport_valid
        && peer_valid
        && direction_valid
        && matches!(config.completion, Completion::DurationMillis(duration) if (1..=300_000).contains(&duration))
}

async fn transition_state(state: &mut SessionState, current: SessionState, session_id: u64) {
    let previous = *state;
    *state = current;
    publish_event_reliably(
        session_id,
        0,
        Event::State(StateChange { previous, current }),
    )
    .await;
}

async fn publish_result(result: SessionResult, request_id: u32) {
    let evidence = EvidenceRecord::Transport(result.evidence);
    let checksum = evidence_crc32c(core::slice::from_ref(&evidence))
        .expect("one transport evidence record fits the protocol digest buffer");
    publish_event_reliably(result.session_id, request_id, Event::Evidence(evidence)).await;
    publish_event_reliably(
        result.session_id,
        request_id,
        Event::Finished(Finished {
            summary: ResultSummary {
                passed: result.passed,
                evidence_records: 1,
            },
            evidence_crc32c: checksum,
        }),
    )
    .await;
}

/// Runs the runtime transport worker.
///
/// Spawn this task once the Embassy executor starts. Before it starts, records
/// are written synchronously so early boot diagnostics remain visible. Once it
/// is active, normal `log` records use a bounded, non-blocking SRAM queue. The
/// worker sleeps while the USB endpoint is busy and resumes from its interrupt;
/// it never spins waiting for the host.
#[embassy_executor::task]
pub async fn logger_task(usb_device: USB_DEVICE<'static>) {
    let (mut rx, mut tx) = UsbSerialJtag::new(usb_device).into_async().split();
    RUNTIME_ACTIVE.store(true, Ordering::Release);
    let mut reported_dropped = 0;
    let mut reported_truncated = 0;
    let mut decoder = FrameDecoder::new();
    let mut encoder = FrameEncoder::new();
    let mut rx_buffer = [0_u8; USB_RX_CHUNK_BYTES];
    loop {
        match select3(EVENTS.receive(), rx.read(&mut rx_buffer), RECORDS.receive()).await {
            Either3::First(event) => write_event_async(&mut tx, &mut encoder, &event).await,
            Either3::Second(Ok(length)) => {
                decoder.feed::<Envelope<Command>>(&rx_buffer[..length], |message| {
                    if let Ok(command) = message {
                        receive_command(command);
                    }
                });
            }
            Either3::Second(Err(_)) => {}
            Either3::Third(record) => write_record_async(&mut tx, &record).await,
        }

        for _ in 1..DRAIN_BATCH {
            if let Ok(event) = EVENTS.try_receive() {
                write_event_async(&mut tx, &mut encoder, &event).await;
            } else if let Ok(record) = RECORDS.try_receive() {
                write_record_async(&mut tx, &record).await;
            } else {
                break;
            }
        }
        report_health_changes(&mut tx, &mut reported_dropped, &mut reported_truncated).await;
        embassy_futures::yield_now().await;
    }
}

fn receive_command(command: Envelope<Command>) {
    let session_id = command.session_id;
    let request_id = command.request_id;
    let rejection = if command.protocol_version != PROTOCOL_VERSION {
        Some(RejectReason::ProtocolVersion)
    } else if command.boot_id != boot_id() {
        Some(RejectReason::BootId)
    } else {
        None
    };
    if let Some(reason) = rejection {
        publish_event(session_id, request_id, Event::Rejected(reason));
    } else if COMMANDS.try_send(command).is_err() {
        publish_event(session_id, request_id, Event::Rejected(RejectReason::Busy));
    }
}

async fn write_event_async(
    tx: &mut UsbSerialJtagTx<'static, Async>,
    encoder: &mut FrameEncoder,
    event: &Envelope<Event>,
) {
    let Ok(frame) = encoder.encode(event) else {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let _guard = WriterGuard::acquire_protocol().await;
    if tx.write_all(frame).await.is_ok() && tx.write_all(b"\r\n").await.is_ok() {
        PROTOCOL_TX_FRAMES.fetch_add(1, Ordering::Relaxed);
        if matches!(&event.body, Event::StationLifecycle(_)) {
            SERIALIZED_STATION_LIFECYCLE_NEXT
                .store(event.message_sequence.wrapping_add(1), Ordering::Release);
        }
    } else {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

async fn report_health_changes(
    tx: &mut UsbSerialJtagTx<'static, Async>,
    reported_dropped: &mut u32,
    reported_truncated: &mut u32,
) {
    let dropped = dropped_records();
    let truncated = truncated_records();
    if dropped == *reported_dropped && truncated == *reported_truncated {
        return;
    }

    let record = format_record(format_args!(
        "[WARN logger] dropped_total={dropped} truncated_total={truncated}"
    ));
    write_record_async(tx, &record).await;
    *reported_dropped = dropped;
    *reported_truncated = truncated;
}

fn format_record(args: Arguments<'_>) -> TextBuffer<MESSAGE_CAPACITY> {
    let mut message = TextBuffer::<MESSAGE_CAPACITY>::new();
    let _ = message.write_fmt(args);
    if message.was_truncated() {
        TRUNCATED_RECORDS.fetch_add(1, Ordering::Relaxed);
    }
    message
}

fn submit_line(args: Arguments<'_>) {
    if RUNTIME_ACTIVE.load(Ordering::Acquire) {
        // Under sustained pressure, avoid paying even the formatting cost for
        // a record that cannot enter the bounded queue. This observation is a
        // best-effort fast path; try_send below remains the authoritative race-
        // safe capacity check.
        if RECORDS.is_full() {
            DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let record = format_record(args);
        if RECORDS.try_send(record).is_err() {
            DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        }
    } else {
        let record = format_record(args);
        write_record_immediate(&record);
    }
}

fn write_line_immediate(args: Arguments<'_>) {
    let record = format_record(args);
    write_record_immediate(&record);
}

fn write_record_immediate(message: &TextBuffer<MESSAGE_CAPACITY>) {
    let Ok(_guard) = WriterGuard::acquire() else {
        DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        return;
    };

    unsafe {
        ets_printf(c"%s\r\n".as_ptr().cast(), message.as_c_string());
    }
}

async fn write_record_async(
    tx: &mut UsbSerialJtagTx<'static, Async>,
    message: &TextBuffer<MESSAGE_CAPACITY>,
) {
    let Ok(_guard) = WriterGuard::acquire() else {
        DROPPED_RECORDS.fetch_add(1, Ordering::Relaxed);
        return;
    };

    // The HAL submits at most one 64-byte USB packet at a time. If the endpoint
    // is busy, this await parks the task until SERIAL_IN_EMPTY wakes it.
    let _ = tx.write_all(message.as_bytes()).await;
    let _ = tx.write_all(b"\r\n").await;
}

struct WriterGuard;

impl WriterGuard {
    fn acquire() -> Result<Self, ()> {
        if PROTOCOL_WRITER_WAITING.load(Ordering::Acquire) {
            return Err(());
        }
        WRITER_ACTIVE
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| ())
    }

    /// Give an admitted protocol frame priority over new best-effort text.
    ///
    /// The only concurrent holder is the synchronous ROM text writer; it
    /// cannot await while holding the guard. Once this intent flag is visible,
    /// new diagnostics fail fast and the protocol owner acquires on the next
    /// finite release rather than discarding a correlated response.
    async fn acquire_protocol() -> Self {
        PROTOCOL_WRITER_WAITING.store(true, Ordering::Release);
        loop {
            if WRITER_ACTIVE
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                PROTOCOL_WRITER_WAITING.store(false, Ordering::Release);
                return Self;
            }
            yield_now().await;
        }
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        WRITER_ACTIVE.store(false, Ordering::Release);
    }
}

struct ConsoleLogger;

impl ::log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &::log::Metadata<'_>) -> bool {
        metadata.level() <= ::log::STATIC_MAX_LEVEL
    }

    fn log(&self, record: &::log::Record<'_>) {
        if self.enabled(record.metadata()) {
            submit_line(format_args!(
                "[{} {}] {}",
                record.level(),
                record.target(),
                record.args()
            ));
        }
    }

    fn flush(&self) {}
}

/// Installs the firmware logger. Calling this more than once is harmless.
pub fn init_logger() {
    static LOGGER: ConsoleLogger = ConsoleLogger;
    if ::log::set_logger(&LOGGER).is_ok() {
        ::log::set_max_level(::log::STATIC_MAX_LEVEL);
    }
}

#[cfg(test)]
mod tests {
    use super::TextBuffer;
    use core::fmt::Write;

    #[test]
    fn text_buffer_keeps_space_for_nul() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcdef").unwrap();

        assert_eq!(&buffer.bytes, b"abcd\0");
        assert!(buffer.was_truncated());
    }

    #[test]
    fn exact_fit_is_not_truncated() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcd").unwrap();

        assert_eq!(&buffer.bytes, b"abcd\0");
        assert!(!buffer.was_truncated());
    }
}
