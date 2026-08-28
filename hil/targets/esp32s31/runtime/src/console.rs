//! Early-console and runtime logging backend.
//!
//! Application code should use the macros from the `log` crate. Direct ROM
//! output remains available for the boot and panic paths, where the executor
//! and the asynchronous logging transport may not be running yet.

use core::{
    cell::RefCell,
    fmt::{Arguments, Write},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};
use embassy_futures::{
    select::{Either, Either3, select, select3},
    yield_now,
};
use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    channel::Channel,
    mutex::Mutex as AsyncMutex,
};
use embassy_time::{Instant, Timer};
use embedded_io_async::{Read as _, Write as _};
use esp_hal::{
    Async,
    peripherals::USB_DEVICE,
    usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagTx},
};
#[cfg(feature = "ieee802154-ed-event-probe")]
use open_esp_radio_hil_protocol::Ieee802154EdEventProbeRequest;
#[cfg(feature = "ieee802154-event-status-probe")]
use open_esp_radio_hil_protocol::Ieee802154EventStatusProbeRequest;
use open_esp_radio_hil_protocol::{
    Capabilities, Command, Completion, Direction, Envelope, Event, EvidenceRecord, FailureCode,
    Finished, FrameDecoder, FrameEncoder, LinkHealth, NetworkCredentials, NetworkIpv4Configuration,
    PROTOCOL_VERSION, RejectReason, ResultSummary, RxDeliveryEvidence,
    STARTUP_ARTIFACT_CHUNK_MAX_LEN, SessionConfig, SessionState, StartupArtifactChunk,
    StartupArtifactDisposition, StartupArtifactStatus, StateChange, StationEpochEvidence,
    StationLifecycleEvent, TimebaseProbeEvidence, TimebaseProbeRequest, Transport,
    TransportEvidence, WifiAccessPointEvidence, WifiAccessPointRequest, WifiMonitorCaptureRequest,
    WifiMonitorEvidence, WifiMonitorFrameChunk, WifiMonitorRequest, WifiRole,
    WifiRoleFailureEvidence, WifiRoleTransitionEvidence, WifiScanEvidence, WifiScanRequest,
    WifiStationAccessPointRequest, WifiStationAccessPointStopEvidence, evidence_crc32c,
    startup_artifact_crc32c,
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
// Sequence reservation and queue insertion are one producer transaction.
// Reserving before an async `send` permits a later task to enqueue sequence
// N+1 ahead of N, which makes an otherwise lossless wire stream fail closed.
#[unsafe(link_section = ".critical.data.logging")]
static EVENT_PUBLISH: AsyncMutex<CriticalSectionRawMutex, ()> = AsyncMutex::new(());
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_DROPPED: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_TX_FRAMES: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_RX_FRAMES: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_RX_COBS_ERRORS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_RX_CHECKSUM_ERRORS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_RX_DECODE_ERRORS: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static PROTOCOL_RX_OVERFLOWS: AtomicU32 = AtomicU32::new(0);
/// One plus the last event sequence fully written to the USB endpoint.
/// Zero means that no event from the current boot has crossed that boundary.
#[unsafe(link_section = ".critical.data.logging")]
static SERIALIZED_WIFI_EVENT_NEXT: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.data.logging")]
static WIFI_ROLE_STATE: AtomicU32 = AtomicU32::new(0);
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
#[cfg(feature = "ieee802154-event-status-probe")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_EVENT_STATUS_PROBES: Channel<
    CriticalSectionRawMutex,
    Ieee802154EventStatusProbe,
    1,
> = Channel::new();
#[cfg(feature = "ieee802154-ed-event-probe")]
#[unsafe(link_section = ".critical.data.logging")]
static IEEE802154_ED_EVENT_PROBES: Channel<CriticalSectionRawMutex, Ieee802154EdEventProbe, 1> =
    Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_STARTS: Channel<CriticalSectionRawMutex, ActiveSession, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static SESSION_RESULTS: Channel<CriticalSectionRawMutex, SessionResult, 1> = Channel::new();
#[unsafe(link_section = ".critical.data.logging")]
static WIFI_CONTROL_REQUESTS: Channel<CriticalSectionRawMutex, WifiControlRequest, 1> =
    Channel::new();

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WifiControlRequest {
    Cycle {
        request_id: u32,
    },
    StopStation {
        request_id: u32,
    },
    StartStation {
        request_id: u32,
        credentials: NetworkCredentials,
    },
    Scan {
        request_id: u32,
        request: WifiScanRequest,
    },
    StartMonitor {
        request_id: u32,
        request: WifiMonitorRequest,
    },
    StopMonitor {
        request_id: u32,
    },
    CaptureMonitor {
        request_id: u32,
        request: WifiMonitorCaptureRequest,
    },
    StartAccessPoint {
        request_id: u32,
        request: WifiAccessPointRequest,
    },
    StopAccessPoint {
        request_id: u32,
    },
    StartStationAccessPoint {
        request_id: u32,
        request: WifiStationAccessPointRequest,
    },
    StopStationAccessPoint {
        request_id: u32,
    },
}

#[derive(Clone, Copy)]
pub struct ActiveSession {
    pub session_id: u64,
    pub config: SessionConfig,
}

pub struct StartupConfiguration {
    pub request_id: u32,
    pub ipv4: NetworkIpv4Configuration,
    pub data_plane: open_esp_radio_hil_protocol::WifiDataPlanePlacement,
    pub rx_checksum: open_esp_radio_hil_protocol::WifiRxChecksumPolicy,
    pub rx_admission: open_esp_radio_hil_protocol::WifiRxAdmissionPolicy,
    pub phy_calibration_artifact: Option<StartupArtifact>,
}

/// The single owner-consuming operation selected before radio initialization.
///
/// The IEEE 802.15.4 variant is a bounded diagnostic discriminator only. It
/// neither enables the CPU interrupt route nor attests production IRQ
/// readiness, and the target returns after publishing its one completion.
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc target moves the existing bounded startup artifact through this one-shot owner handoff"
)]
pub enum PreInitializationRequest {
    Startup(StartupConfiguration),
    #[cfg(feature = "ieee802154-event-status-probe")]
    Ieee802154EventStatus(Ieee802154EventStatusProbe),
    #[cfg(feature = "ieee802154-ed-event-probe")]
    Ieee802154EdEvent(Ieee802154EdEventProbe),
}

#[cfg(feature = "ieee802154-event-status-probe")]
pub struct Ieee802154EventStatusProbe {
    pub request_id: u32,
    pub request: Ieee802154EventStatusProbeRequest,
}

#[cfg(feature = "ieee802154-ed-event-probe")]
pub struct Ieee802154EdEventProbe {
    pub request_id: u32,
    pub request: Ieee802154EdEventProbeRequest,
}

#[derive(Clone, Copy)]
pub struct StartupArtifact {
    bytes: [u8; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
    len: u16,
}

impl StartupArtifact {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

struct StartupArtifactAssembler {
    bytes: [u8; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
    expected_total: Option<u16>,
    expected_crc32c: u32,
    received: usize,
    complete: bool,
}

impl StartupArtifactAssembler {
    const fn new() -> Self {
        Self {
            bytes: [0; crate::phy_calibration_artifact::MAX_ENCODED_LEN],
            expected_total: None,
            expected_crc32c: 0,
            received: 0,
            complete: false,
        }
    }

    fn push(&mut self, chunk: &StartupArtifactChunk) -> Result<(), ()> {
        chunk.validate().map_err(|_| ())?;
        if usize::from(chunk.total_length()) > self.bytes.len() {
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
            if self.received != usize::from(chunk.total_length())
                || startup_artifact_crc32c(&self.bytes[..self.received]) != self.expected_crc32c
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

    fn completed_artifact(&self) -> Option<StartupArtifact> {
        self.complete.then_some(StartupArtifact {
            bytes: self.bytes,
            len: u16::try_from(self.received).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct SessionResult {
    session_id: u64,
    evidence: TransportEvidence,
    radio: Option<open_esp_radio_hil_protocol::RadioEvidence>,
    tx_timing: Option<open_esp_radio_hil_protocol::TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    passed: bool,
}

#[derive(Clone, Copy)]
struct ProtocolSession {
    active: ActiveSession,
    state: SessionState,
}

static RETAINED_SESSION_RESULTS: Mutex<
    CriticalSectionRawMutex,
    RefCell<[Option<SessionResult>; 2]>,
> = Mutex::new(RefCell::new([None; 2]));

fn retained_session_result(session_id: u64) -> Option<SessionResult> {
    RETAINED_SESSION_RESULTS.lock(|results| {
        results
            .borrow()
            .iter()
            .flatten()
            .copied()
            .find(|result| result.session_id == session_id)
    })
}

fn retain_session_result(result: SessionResult) -> bool {
    RETAINED_SESSION_RESULTS.lock(|results| {
        let mut results = results.borrow_mut();
        let Some(slot) = results.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(result);
        true
    })
}

fn discard_session_result(session_id: u64) -> bool {
    RETAINED_SESSION_RESULTS.lock(|results| {
        let mut results = results.borrow_mut();
        let Some(slot) = results
            .iter_mut()
            .find(|slot| slot.is_some_and(|result| result.session_id == session_id))
        else {
            return false;
        };
        *slot = None;
        true
    })
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

/// Queues one best-effort diagnostic line on the runtime USB transport.
///
/// Unlike [`emergency_log`], this path is serialized by [`logger_task`] with
/// binary protocol frames. Runtime code must use this function so a ROM write
/// cannot overtake a USB packet that the asynchronous HAL has only submitted.
pub fn runtime_log(args: Arguments<'_>) {
    submit_line(args);
}

/// Queues one diagnostic line without allowing the bounded text queue to
/// erase it.
///
/// This is intentionally restricted to reporting outside measured hot paths:
/// awaiting text capacity inside radio, network or traffic service would make
/// USB progress part of their runtime contract.
pub async fn runtime_log_reliably(args: Arguments<'_>) {
    if RUNTIME_ACTIVE.load(Ordering::Acquire) {
        RECORDS.send(format_record(args)).await;
    } else {
        write_record_immediate(&format_record(args));
    }
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
    SERIALIZED_WIFI_EVENT_NEXT.store(0, Ordering::Relaxed);
    WIFI_ROLE_STATE.store(0, Ordering::Relaxed);
}

/// Publish the application-visible Wi-Fi owner state used for command
/// admission. Zero is reserved for the boot interval before a role exists.
pub fn set_wifi_role(role: WifiRole) {
    let encoded = match role {
        WifiRole::Idle => 1,
        WifiRole::Station => 2,
        WifiRole::Monitor => 3,
        WifiRole::AccessPoint => 4,
        WifiRole::StationAccessPoint => 5,
    };
    WIFI_ROLE_STATE.store(encoded, Ordering::Release);
}

fn wifi_role_is(role: WifiRole) -> bool {
    let expected = match role {
        WifiRole::Idle => 1,
        WifiRole::Station => 2,
        WifiRole::Monitor => 3,
        WifiRole::AccessPoint => 4,
        WifiRole::StationAccessPoint => 5,
    };
    WIFI_ROLE_STATE.load(Ordering::Acquire) == expected
}

fn boot_id() -> u64 {
    u64::from(BOOT_ID_LOW.load(Ordering::Acquire))
        | (u64::from(BOOT_ID_HIGH.load(Ordering::Acquire)) << 32)
}

/// Queues a typed event without making a radio or network task wait for USB.
pub fn publish_event(session_id: u64, request_id: u32, body: Event) {
    let Ok(_publisher) = EVENT_PUBLISH.try_lock() else {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    let message_sequence = EVENT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let event = Envelope::new(boot_id(), message_sequence, session_id, request_id, body);
    if EVENTS.try_send(event).is_err() {
        PROTOCOL_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Waits without polling until the host chooses the boot's unique radio owner.
pub async fn receive_pre_initialization_request() -> PreInitializationRequest {
    #[cfg(all(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-ed-event-probe"
    ))]
    {
        match select3(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_EVENT_STATUS_PROBES.receive(),
            IEEE802154_ED_EVENT_PROBES.receive(),
        )
        .await
        {
            Either3::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either3::Second(probe) => PreInitializationRequest::Ieee802154EventStatus(probe),
            Either3::Third(probe) => PreInitializationRequest::Ieee802154EdEvent(probe),
        }
    }
    #[cfg(all(
        feature = "ieee802154-event-status-probe",
        not(feature = "ieee802154-ed-event-probe")
    ))]
    {
        match select(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_EVENT_STATUS_PROBES.receive(),
        )
        .await
        {
            Either::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either::Second(probe) => PreInitializationRequest::Ieee802154EventStatus(probe),
        }
    }
    #[cfg(all(
        feature = "ieee802154-ed-event-probe",
        not(feature = "ieee802154-event-status-probe")
    ))]
    {
        match select(
            STARTUP_CONFIGURATIONS.receive(),
            IEEE802154_ED_EVENT_PROBES.receive(),
        )
        .await
        {
            Either::First(configuration) => PreInitializationRequest::Startup(configuration),
            Either::Second(probe) => PreInitializationRequest::Ieee802154EdEvent(probe),
        }
    }
    #[cfg(not(any(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-ed-event-probe"
    )))]
    {
        PreInitializationRequest::Startup(STARTUP_CONFIGURATIONS.receive().await)
    }
}

/// Publish the role-neutral completion edge for one initialization command.
pub async fn complete_initialization(request_id: u32) {
    publish_event_reliably(0, request_id, Event::Initialized).await;
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
pub async fn receive_wifi_control_request() -> WifiControlRequest {
    WIFI_CONTROL_REQUESTS.receive().await
}

/// Reliably acknowledge a completed target-side station ownership cycle.
///
/// Unlike text diagnostics, this event is serialized by the protocol owner
/// and retains the command request ID used by the host qualifier.
pub async fn complete_station_epoch_cycle(request_id: u32, evidence: StationEpochEvidence) {
    publish_event_reliably(0, request_id, Event::StationEpochCompleted(evidence)).await;
}

/// Reliably acknowledge complete station dematerialization and reconstruction
/// of the role-neutral Wi-Fi owner.
pub async fn complete_wifi_role_transition(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence = queue_event_reliably(0, request_id, Event::WifiRoleTransitioned(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_wifi_scan(request_id: u32, evidence: WifiScanEvidence) {
    let sequence = queue_event_reliably(0, request_id, Event::WifiScanCompleted(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_monitor_start(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence = queue_event_reliably(0, request_id, Event::WifiMonitorStarted(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_monitor_stop(request_id: u32, evidence: WifiMonitorEvidence) {
    let sequence = queue_event_reliably(0, request_id, Event::WifiMonitorStopped(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_access_point_start(request_id: u32, evidence: WifiRoleTransitionEvidence) {
    let sequence =
        queue_event_reliably(0, request_id, Event::WifiAccessPointStarted(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_access_point_stop(request_id: u32, evidence: WifiAccessPointEvidence) {
    let sequence =
        queue_event_reliably(0, request_id, Event::WifiAccessPointStopped(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_station_access_point_stop(
    request_id: u32,
    evidence: WifiStationAccessPointStopEvidence,
) {
    let sequence = queue_event_reliably(
        0,
        request_id,
        Event::WifiStationAccessPointStopped(evidence),
    )
    .await;
    wait_until_serialized(sequence).await;
}

pub async fn complete_wifi_role_failure(request_id: u32, evidence: WifiRoleFailureEvidence) {
    let sequence = queue_event_reliably(0, request_id, Event::WifiRoleFailed(evidence)).await;
    wait_until_serialized(sequence).await;
}

pub async fn publish_monitor_frame(request_id: u32, chunk: WifiMonitorFrameChunk) {
    queue_event_reliably(0, request_id, Event::WifiMonitorFrame(chunk)).await;
}

pub async fn complete_monitor_capture(request_id: u32, evidence: WifiMonitorEvidence) {
    let sequence =
        queue_event_reliably(0, request_id, Event::WifiMonitorCaptureCompleted(evidence)).await;
    wait_until_serialized(sequence).await;
}

async fn wait_until_serialized(sequence: u32) {
    let target = sequence.wrapping_add(1);
    while SERIALIZED_WIFI_EVENT_NEXT.load(Ordering::Acquire) != target {
        yield_now().await;
    }
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
    while SERIALIZED_WIFI_EVENT_NEXT.load(Ordering::Acquire) != target {
        yield_now().await;
    }
}

/// Hands a completed in-memory measurement back to the protocol owner.
///
/// USB serialization happens in another task and therefore cannot extend the
/// benchmark's measured interval.
pub async fn complete_session(
    session_id: u64,
    evidence: TransportEvidence,
    radio: Option<open_esp_radio_hil_protocol::RadioEvidence>,
    tx_timing: Option<open_esp_radio_hil_protocol::TxAggregateTimingEvidence>,
    rx_delivery: Option<RxDeliveryEvidence>,
    passed: bool,
) {
    SESSION_RESULTS
        .send(SessionResult {
            session_id,
            evidence,
            radio,
            tx_timing,
            rx_delivery,
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
    let _publisher = EVENT_PUBLISH.lock().await;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ieee802154EventStatusProbeAdmission {
    Admit,
    Reject(RejectReason),
}

/// Decide admission without touching the radio owner or any validation MMIO.
///
/// Unsupported images take precedence over state and request validation.
/// Once supported, every owner/state mismatch takes precedence over malformed
/// bounds so a caller cannot use configuration errors to probe runtime state.
const fn ieee802154_event_status_probe_admission(
    supported: bool,
    initialized: bool,
    waiting_for_initialization: bool,
    session_id: u64,
    already_requested: bool,
    request_valid: bool,
) -> Ieee802154EventStatusProbeAdmission {
    if !supported {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::Unsupported)
    } else if initialized || !waiting_for_initialization || session_id != 0 || already_requested {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    } else if !request_valid {
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidConfiguration)
    } else {
        Ieee802154EventStatusProbeAdmission::Admit
    }
}

/// Reserve normal initialization once either initialization itself or the
/// one-shot owner-consuming IEEE diagnostic has been admitted.
const fn initialization_owner_rejection(
    initialized: bool,
    ieee802154_diagnostic_requested: bool,
) -> Option<RejectReason> {
    if initialized || ieee802154_diagnostic_requested {
        Some(RejectReason::InvalidState)
    } else {
        None
    }
}

// Compile-time regression matrix for the pure pre-initialization admission
// boundary. These assertions are built in both ordinary and diagnostic target
// graphs and add no runtime code or validation capability.
const _: () = {
    assert!(matches!(
        ieee802154_event_status_probe_admission(false, true, false, 7, true, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::Unsupported)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, true, true, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, false, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 1, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, true, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidState)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, false, false),
        Ieee802154EventStatusProbeAdmission::Reject(RejectReason::InvalidConfiguration)
    ));
    assert!(matches!(
        ieee802154_event_status_probe_admission(true, false, true, 0, false, true),
        Ieee802154EventStatusProbeAdmission::Admit
    ));
    assert!(matches!(
        initialization_owner_rejection(false, true),
        Some(RejectReason::InvalidState)
    ));
};

/// Owns commands while individual benchmark services migrate to
/// runtime-configured sessions. Unsupported mutations receive an explicit
/// response instead of being silently ignored.
#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the protocol task moves bounded typed session results into its static Embassy arena; the linked-image stack audit remains authoritative"
)]
pub async fn protocol_task(capabilities: Capabilities) {
    publish_event_reliably(0, 0, Event::Hello(capabilities)).await;
    publish_event_reliably(
        0,
        0,
        Event::State(StateChange {
            previous: SessionState::Booting,
            current: SessionState::WaitingForInitialization,
        }),
    )
    .await;
    let mut initialized = false;
    let mut state = SessionState::WaitingForInitialization;
    // Once this owner-consuming diagnostic has been queued, normal radio
    // initialization must not race it. The diagnostic image is one-shot and
    // publishes its completion before the product task returns.
    #[cfg(any(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-ed-event-probe"
    ))]
    let mut ieee802154_diagnostic_requested = false;
    #[cfg(not(any(
        feature = "ieee802154-event-status-probe",
        feature = "ieee802154-ed-event-probe"
    )))]
    let ieee802154_diagnostic_requested = false;
    // One slot per physical STA+AP network endpoint. A slot is keyed by its
    // opaque session ID and no two live slots may target the same interface.
    let mut sessions = [None::<ProtocolSession>; 2];
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
                    Command::QueryStackUsage => {
                        let response = if initialized
                            && state == SessionState::Idle
                            && sessions.iter().all(Option::is_none)
                            && session_id == 0
                        {
                            Event::StackUsage(crate::stack_usage_snapshot())
                        } else {
                            Event::Rejected(RejectReason::InvalidState)
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::QueryLinkHealth => {
                        let response = if session_id == 0 {
                            Event::LinkHealth(link_health_snapshot())
                        } else {
                            Event::Rejected(RejectReason::InvalidState)
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::ProbeTimebase(request) => {
                        let response = if !capabilities.features.timebase_probe {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if initialized
                            || state != SessionState::WaitingForInitialization
                            || session_id != 0
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if !request.validate() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else {
                            Event::TimebaseProbeCompleted(run_timebase_probe(request).await)
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::ProbeIeee802154EventStatus(request) => {
                        let admission = ieee802154_event_status_probe_admission(
                            capabilities.features.ieee802154_event_status_probe,
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    Event::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "ieee802154-event-status-probe")]
                                {
                                    if IEEE802154_EVENT_STATUS_PROBES
                                        .try_send(Ieee802154EventStatusProbe {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            Event::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "ieee802154-event-status-probe"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    Event::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    Command::ProbeIeee802154EdEvent(request) => {
                        let admission = ieee802154_event_status_probe_admission(
                            capabilities.features.ieee802154_ed_event_probe,
                            initialized,
                            state == SessionState::WaitingForInitialization,
                            session_id,
                            ieee802154_diagnostic_requested,
                            request.validate(),
                        );
                        match admission {
                            Ieee802154EventStatusProbeAdmission::Reject(reason) => {
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    Event::Rejected(reason),
                                )
                                .await;
                            }
                            Ieee802154EventStatusProbeAdmission::Admit => {
                                #[cfg(feature = "ieee802154-ed-event-probe")]
                                {
                                    if IEEE802154_ED_EVENT_PROBES
                                        .try_send(Ieee802154EdEventProbe {
                                            request_id,
                                            request,
                                        })
                                        .is_err()
                                    {
                                        publish_event_reliably(
                                            session_id,
                                            request_id,
                                            Event::Rejected(RejectReason::Busy),
                                        )
                                        .await;
                                    } else {
                                        ieee802154_diagnostic_requested = true;
                                    }
                                }
                                #[cfg(not(feature = "ieee802154-ed-event-probe"))]
                                publish_event_reliably(
                                    session_id,
                                    request_id,
                                    Event::Rejected(RejectReason::Unsupported),
                                )
                                .await;
                            }
                        }
                    }
                    Command::UploadStartupArtifact(chunk) => {
                        let response = if !capabilities.features.startup_artifact {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if initialized || state != SessionState::WaitingForInitialization {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if startup_artifact.push(&chunk).is_err() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::Initialize(configuration) => {
                        let (response, accepted) = if let Some(reason) =
                            initialization_owner_rejection(
                                initialized,
                                ieee802154_diagnostic_requested,
                            ) {
                            (Event::Rejected(reason), false)
                        } else if !configuration.validate()
                            || startup_artifact.started_but_incomplete()
                        {
                            (Event::Rejected(RejectReason::InvalidConfiguration), false)
                        } else if STARTUP_CONFIGURATIONS
                            .try_send(StartupConfiguration {
                                request_id,
                                ipv4: configuration.ipv4,
                                data_plane: configuration.data_plane,
                                rx_checksum: configuration.rx_checksum,
                                rx_admission: configuration.rx_admission,
                                phy_calibration_artifact: startup_artifact.completed_artifact(),
                            })
                            .is_err()
                        {
                            (Event::Rejected(RejectReason::Busy), false)
                        } else {
                            initialized = true;
                            (Event::Accepted, true)
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                        if accepted {
                            transition_state(&mut state, SessionState::Idle, 0, request_id).await;
                        }
                    }
                    Command::Configure(config) => {
                        let duplicate_interface = sessions.iter().flatten().any(|session| {
                            session.active.config.network_interface == config.network_interface
                        });
                        let free = sessions.iter().position(Option::is_none);
                        let rejection = if !capabilities.features.runtime_configuration {
                            Some(RejectReason::Unsupported)
                        } else if !initialized || state != SessionState::Idle {
                            Some(RejectReason::InvalidState)
                        } else if session_id == 0 {
                            Some(RejectReason::SessionId)
                        } else if !valid_session_config(config, capabilities) {
                            Some(RejectReason::InvalidConfiguration)
                        } else if duplicate_interface || free.is_none() {
                            Some(RejectReason::Busy)
                        } else {
                            None
                        };
                        if let Some(reason) = rejection {
                            publish_event_reliably(session_id, request_id, Event::Rejected(reason))
                                .await;
                        } else {
                            let index = free.expect("validated session capacity has a free slot");
                            sessions[index] = Some(ProtocolSession {
                                active: ActiveSession { session_id, config },
                                state: SessionState::Idle,
                            });
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(
                                &mut sessions[index]
                                    .as_mut()
                                    .expect("configured slot remains owned")
                                    .state,
                                SessionState::Configured,
                                session_id,
                                request_id,
                            )
                            .await;
                        }
                    }
                    Command::Arm => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| session.active.session_id == session_id)
                        });
                        if slot.is_none_or(|index| {
                            sessions[index]
                                .is_none_or(|session| session.state != SessionState::Configured)
                        }) {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        } else {
                            let index = slot.expect("validated arm has a session slot");
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            transition_state(
                                &mut sessions[index]
                                    .as_mut()
                                    .expect("armed slot remains owned")
                                    .state,
                                SessionState::Armed,
                                session_id,
                                request_id,
                            )
                            .await;
                        }
                    }
                    Command::Start => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && session.state == SessionState::Armed
                            })
                        });
                        if let Some(index) = slot {
                            let session = sessions[index]
                                .expect("located session slot remains owned")
                                .active;
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
                                transition_state(
                                    &mut sessions[index]
                                        .as_mut()
                                        .expect("running slot remains owned")
                                        .state,
                                    SessionState::Running,
                                    session_id,
                                    request_id,
                                )
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
                    Command::GetStatus => {
                        let session = sessions.iter().flatten().find(|session| {
                            session_id == 0 || session.active.session_id == session_id
                        });
                        publish_event_reliably(
                            session_id,
                            request_id,
                            Event::OperationStatus(open_esp_radio_hil_protocol::OperationStatus {
                                state: session.map_or(state, |session| session.state),
                                configured_session_id: session
                                    .map(|session| session.active.session_id),
                                completed_session_id: session
                                    .and_then(|session| {
                                        retained_session_result(session.active.session_id)
                                    })
                                    .map(|result| result.session_id),
                            }),
                        )
                        .await;
                    }
                    Command::Cancel => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && matches!(
                                        session.state,
                                        SessionState::Configured | SessionState::Armed
                                    )
                            })
                        });
                        if let Some(index) = slot {
                            let previous = sessions[index]
                                .expect("located cancel slot remains owned")
                                .state;
                            sessions[index] = None;
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::State(StateChange {
                                    previous,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::ReplayResult => {
                        if let Some(result) = retained_session_result(session_id) {
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
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && session.state == SessionState::Finished
                                    && retained_session_result(session_id).is_some()
                            })
                        });
                        if let Some(index) = slot {
                            let _ = discard_session_result(session_id);
                            sessions[index] = None;
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::State(StateChange {
                                    previous: SessionState::Finished,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
                        } else {
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::Rejected(RejectReason::InvalidState),
                            )
                            .await;
                        }
                    }
                    Command::Recover => {
                        let slot = sessions.iter().position(|slot| {
                            slot.is_some_and(|session| {
                                session.active.session_id == session_id
                                    && matches!(
                                        session.state,
                                        SessionState::Finished | SessionState::Failed
                                    )
                            })
                        });
                        if let Some(index) = slot {
                            let previous = sessions[index]
                                .expect("located recovery slot remains owned")
                                .state;
                            sessions[index] = None;
                            let _ = discard_session_result(session_id);
                            publish_event_reliably(session_id, request_id, Event::Accepted).await;
                            publish_event_reliably(
                                session_id,
                                request_id,
                                Event::State(StateChange {
                                    previous,
                                    current: SessionState::Idle,
                                }),
                            )
                            .await;
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
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Station)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::Cycle { request_id })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StopStation => {
                        let response = if !capabilities.features.wifi_role_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Station)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopStation { request_id })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StartStation(credentials) => {
                        let response = if !capabilities.features.wifi_role_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if credentials.validate().is_err() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartStation {
                                request_id,
                                credentials,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::ScanWifi(request) => {
                        let valid = request.channel_mask_2_4_ghz != 0
                            && request.channel_mask_2_4_ghz & !0x1fff == 0
                            && (1..=1_000).contains(&request.dwell_millis);
                        let response = if !capabilities.features.wifi_role_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !valid {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::Scan {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StartMonitor(request) => {
                        let valid =
                            (1..=13).contains(&request.channel) && request.snapshot_length <= 2_304;
                        let response = if !capabilities.features.wifi_role_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !valid {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartMonitor {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StopMonitor => {
                        let response = if !capabilities.features.wifi_role_control {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Monitor)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopMonitor { request_id })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StartAccessPoint(request) => {
                        let response = if !capabilities.features.wifi_access_point {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if request.validate().is_err() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartAccessPoint {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StopAccessPoint => {
                        let response = if !capabilities.features.wifi_access_point {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::AccessPoint)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopAccessPoint { request_id })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StartStationAccessPoint(request) => {
                        let response = if !capabilities.features.simultaneous_station_access_point {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if request.validate().is_err() {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StartStationAccessPoint {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::StopStationAccessPoint => {
                        let response = if !capabilities.features.simultaneous_station_access_point {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::StationAccessPoint)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::StopStationAccessPoint { request_id })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                    Command::CaptureMonitor(request) => {
                        let valid = (1..=13).contains(&request.channel)
                            && request.snapshot_length <= 2_304
                            && (100..=30_000).contains(&request.duration_millis);
                        let response = if !capabilities.features.wifi_monitor_capture {
                            Event::Rejected(RejectReason::Unsupported)
                        } else if !valid {
                            Event::Rejected(RejectReason::InvalidConfiguration)
                        } else if !initialized
                            || state != SessionState::Idle
                            || sessions.iter().any(Option::is_some)
                            || session_id != 0
                            || !wifi_role_is(WifiRole::Idle)
                        {
                            Event::Rejected(RejectReason::InvalidState)
                        } else if WIFI_CONTROL_REQUESTS
                            .try_send(WifiControlRequest::CaptureMonitor {
                                request_id,
                                request,
                            })
                            .is_err()
                        {
                            Event::Rejected(RejectReason::Busy)
                        } else {
                            Event::Accepted
                        };
                        publish_event_reliably(session_id, request_id, response).await;
                    }
                }
            }
            Either::Second(result) => {
                let slot = sessions.iter().position(|slot| {
                    slot.is_some_and(|session| {
                        session.state == SessionState::Running
                            && session.active.session_id == result.session_id
                    })
                });
                if let Some(index) = slot {
                    transition_state(
                        &mut sessions[index]
                            .as_mut()
                            .expect("completed slot remains owned")
                            .state,
                        SessionState::Draining,
                        result.session_id,
                        0,
                    )
                    .await;
                    publish_result(result, 0).await;
                    if !retain_session_result(result) {
                        publish_event_reliably(
                            result.session_id,
                            0,
                            Event::Failed(FailureCode::EvidenceOverflow),
                        )
                        .await;
                        transition_state(
                            &mut sessions[index]
                                .as_mut()
                                .expect("failed slot remains owned")
                                .state,
                            SessionState::Failed,
                            result.session_id,
                            0,
                        )
                        .await;
                        continue;
                    }
                    transition_state(
                        &mut sessions[index]
                            .as_mut()
                            .expect("finished slot remains owned")
                            .state,
                        SessionState::Finished,
                        result.session_id,
                        0,
                    )
                    .await;
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
    let link_requirements_valid = match config.link_requirements.tx_block_ack_tid {
        None => true,
        Some(tid) => {
            tid < 8
                && matches!(config.direction, Direction::Tx | Direction::Bidirectional)
                && config.target_tx.is_some()
        }
    };
    transport_valid
        && peer_valid
        && direction_valid
        && link_requirements_valid
        && matches!(config.completion, Completion::DurationMillis(duration) if (1..=300_000).contains(&duration))
}

async fn transition_state(
    state: &mut SessionState,
    current: SessionState,
    session_id: u64,
    request_id: u32,
) {
    let previous = *state;
    *state = current;
    publish_event_reliably(
        session_id,
        request_id,
        Event::State(StateChange { previous, current }),
    )
    .await;
}

async fn publish_result(result: SessionResult, request_id: u32) {
    let mut evidence = heapless::Vec::<EvidenceRecord, 7>::new();
    evidence
        .push(EvidenceRecord::Transport(result.evidence))
        .expect("session evidence has fixed capacity");
    if let Some(radio) = result.radio {
        evidence
            .push(EvidenceRecord::Radio(radio))
            .expect("session evidence has fixed capacity");
    }
    if let Some(timing) = result.tx_timing {
        evidence
            .push(EvidenceRecord::TxAggregateTiming(timing))
            .expect("session evidence has fixed capacity");
    }
    if let Some(rx_delivery) = result.rx_delivery {
        evidence
            .push(EvidenceRecord::RxDelivery(rx_delivery))
            .expect("session evidence has fixed capacity");
    }
    let link = link_health_snapshot();
    evidence
        .push(EvidenceRecord::Link(link))
        .expect("session evidence has fixed capacity");
    evidence
        .push(EvidenceRecord::Stack(crate::stack_usage_snapshot()))
        .expect("session evidence has fixed capacity");
    let checksum = evidence_crc32c(evidence.as_slice())
        .expect("transport and stack evidence fit the protocol digest buffer");
    let evidence_records = evidence.len() as u16;
    for record in evidence {
        publish_event_reliably(result.session_id, request_id, Event::Evidence(record)).await;
    }
    publish_event_reliably(
        result.session_id,
        request_id,
        Event::Finished(Finished {
            summary: ResultSummary {
                passed: result.passed
                    && link.rx_cobs_errors == 0
                    && link.rx_checksum_errors == 0
                    && link.rx_decode_errors == 0
                    && link.rx_overflows == 0
                    && link.tx_dropped == 0,
                evidence_records,
            },
            evidence_crc32c: checksum,
        }),
    )
    .await;
}

fn link_health_snapshot() -> LinkHealth {
    LinkHealth {
        rx_frames: PROTOCOL_RX_FRAMES.load(Ordering::Acquire),
        rx_cobs_errors: PROTOCOL_RX_COBS_ERRORS.load(Ordering::Acquire),
        rx_checksum_errors: PROTOCOL_RX_CHECKSUM_ERRORS.load(Ordering::Acquire),
        rx_decode_errors: PROTOCOL_RX_DECODE_ERRORS.load(Ordering::Acquire),
        rx_overflows: PROTOCOL_RX_OVERFLOWS.load(Ordering::Acquire),
        tx_frames: PROTOCOL_TX_FRAMES.load(Ordering::Acquire),
        tx_dropped: PROTOCOL_DROPPED.load(Ordering::Acquire),
        text_dropped: DROPPED_RECORDS.load(Ordering::Acquire),
        text_truncated: TRUNCATED_RECORDS.load(Ordering::Acquire),
    }
}

async fn run_timebase_probe(request: TimebaseProbeRequest) -> TimebaseProbeEvidence {
    let started = Instant::now();
    let mut previous = started;
    let mut minimum_interval_micros = u32::MAX;
    let mut maximum_interval_micros = 0_u32;
    let mut early_intervals = 0_u16;
    for _ in 0..request.intervals {
        Timer::after_micros(u64::from(request.period_micros)).await;
        let now = Instant::now();
        let measured = now
            .duration_since(previous)
            .as_micros()
            .min(u64::from(u32::MAX)) as u32;
        minimum_interval_micros = minimum_interval_micros.min(measured);
        maximum_interval_micros = maximum_interval_micros.max(measured);
        early_intervals =
            early_intervals.saturating_add(u16::from(measured < request.period_micros));
        previous = now;
    }
    TimebaseProbeEvidence {
        intervals: request.intervals,
        period_micros: request.period_micros,
        elapsed_micros: Instant::now().duration_since(started).as_micros(),
        minimum_interval_micros,
        maximum_interval_micros,
        early_intervals,
    }
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
                decoder.feed::<Command>(&rx_buffer[..length], |message| {
                    if let Ok(command) = message {
                        receive_command(command);
                    }
                });
                let counters = decoder.counters();
                PROTOCOL_RX_FRAMES.store(counters.frames, Ordering::Release);
                PROTOCOL_RX_COBS_ERRORS.store(counters.cobs_errors, Ordering::Release);
                PROTOCOL_RX_CHECKSUM_ERRORS.store(counters.checksum_errors, Ordering::Release);
                PROTOCOL_RX_DECODE_ERRORS.store(
                    counters
                        .too_short
                        .saturating_add(counters.header_errors)
                        .saturating_add(counters.framing_version_errors)
                        .saturating_add(counters.message_kind_errors)
                        .saturating_add(counters.protocol_version_errors)
                        .saturating_add(counters.payload_length_errors)
                        .saturating_add(counters.deserialize_errors),
                    Ordering::Release,
                );
                PROTOCOL_RX_OVERFLOWS.store(counters.overflows, Ordering::Release);
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
        if matches!(
            &event.body,
            Event::StationLifecycle(_)
                | Event::WifiRoleTransitioned(_)
                | Event::WifiScanCompleted(_)
                | Event::WifiMonitorStarted(_)
                | Event::WifiMonitorStopped(_)
                | Event::WifiMonitorCaptureCompleted(_)
                | Event::WifiAccessPointStarted(_)
                | Event::WifiAccessPointStopped(_)
                | Event::WifiStationAccessPointStopped(_)
                | Event::WifiRoleFailed(_)
        ) {
            SERIALIZED_WIFI_EVENT_NEXT
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
        ::log::set_max_level(::log::LevelFilter::Info);
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
