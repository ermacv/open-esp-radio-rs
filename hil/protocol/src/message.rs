use crate::{MemoryBenchmarkEvidence, MemoryBenchmarkRequest};
// The payload names of the families this build leaves out.
#[allow(unused_imports, reason = "empty while every family is on")]
use crate::absent::stand_ins::*;

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 193;
// Keep command envelopes small: startup artifacts are transferred as an
// ordered CRC-protected stream, so a large per-command inline buffer only
// inflates UART queues and executor futures without improving semantics.
pub const STARTUP_ARTIFACT_CHUNK_MAX_LEN: usize = 160;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol_version: u16,
    pub boot_id: u64,
    pub message_sequence: u32,
    pub session_id: u64,
    pub request_id: u32,
    pub body: T,
}

impl Envelope<Command> {
    /// Bind every operation to one boot. Only a session-free capability query
    /// may discover an unknown boot; it cannot initialize or mutate the radio.
    pub fn validate_target(&self, target_boot_id: u64) -> Result<(), RejectReason> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(RejectReason::ProtocolVersion);
        }
        let discovery = self.boot_id == 0
            && self.session_id == 0
            && matches!(self.body, Command::GetCapabilities);
        if target_boot_id == 0 || (self.boot_id != target_boot_id && !discovery) {
            return Err(RejectReason::BootId);
        }
        Ok(())
    }
}

/// Message direction encoded in the fixed wire header.
///
/// Keeping this outside the postcard body lets a decoder reject a frame sent
/// to the wrong endpoint before interpreting the command or event enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WireKind {
    Command = 1,
    Event = 2,
}

pub trait WireBody {
    const WIRE_KIND: WireKind;
}

impl<T> Envelope<T> {
    pub const fn new(
        boot_id: u64,
        message_sequence: u32,
        session_id: u64,
        request_id: u32,
        body: T,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            boot_id,
            message_sequence,
            session_id,
            request_id,
            body,
        }
    }
}

mod diagnostic;
pub use diagnostic::{DiagnosticFeature, DiagnosticFeatures};
#[cfg(feature = "wifi")]
mod session;
#[cfg(feature = "wifi")]
pub use session::*;
#[cfg(feature = "wifi")]
mod wifi_traffic;
#[cfg(feature = "wifi")]
pub use wifi_traffic::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureCapabilities {
    /// Numeric-Comparison-only application, RAM bonds and explicit UI decisions.
    #[serde(default)]
    pub bluetooth_secure_gatt: bool,
    /// Plaintext Trouble GATT application with observation-only HIL control.
    #[serde(default)]
    pub bluetooth_gatt: bool,
    /// Independent SoC deadline diagnostic; requires no radio protocol.
    #[serde(default)]
    pub system_watchdog: bool,
    /// Destructive checkpoints in actual PHY maintenance; diagnostic only.
    #[serde(default)]
    pub phy_fault_injection: bool,
    /// Windows of the radio-PHY register image and of the analog image.
    #[serde(default)]
    pub phy_register_image: bool,
    pub bluetooth_dtm: bool,
    /// Raw HCI exchanges with the image's Controller.
    #[serde(default)]
    pub bluetooth_hci: bool,
    pub udp: bool,
    pub tcp: bool,
    pub rx: bool,
    pub tx: bool,
    pub bidirectional: bool,
    pub runtime_initialization: bool,
    pub runtime_configuration: bool,
    pub structured_evidence: bool,
    /// One UDP session can execute and account more than one peer flow.
    /// Merely carrying the bounded flow table on the wire does not imply this
    /// capability.
    pub udp_multi_flow: bool,
    /// This image accepts one opaque, host-owned startup artifact and can
    /// return its current value after initialization.
    pub startup_artifact: bool,
    /// This image can stop one healthy connected STA epoch at a safe runner
    /// boundary and use the returned owners to exercise reassociation.
    pub station_epoch_control: bool,
    /// This image exposes explicit role-neutral Wi-Fi lifecycle commands.
    pub wifi_role_control: bool,
    /// This image can materialize and stop the bounded WPA2-Personal access
    /// point role described by [`WifiAccessPointRequest`].
    pub wifi_access_point: bool,
    /// This image can materialize one same-channel station plus access point
    /// owner and expose both network endpoints at the same time.
    pub simultaneous_station_access_point: bool,
    /// This image can run one finite normalized monitor capture and export
    /// typed frame chunks without using UART text as a data protocol.
    pub wifi_monitor_capture: bool,
    /// This image reliably reports connected generations and proved peer-loss
    /// transitions independently of lossy text diagnostics.
    pub station_lifecycle_events: bool,
    /// This image installs driver-side value observers. Performance images
    /// leave the observation graph absent from the compiled datapath.
    pub driver_observation_evidence: bool,
    /// UDP RX sessions can return typed evidence for every delivery frontier
    /// from post-reorder publication through the application socket.
    pub rx_delivery_evidence: bool,
    /// Diagnostic build features compiled into this image.
    pub diagnostic_features: DiagnosticFeatures,
    /// The direct source-owned RX-gain transaction executes from internal SRAM.
    pub phy_rx_hot_sram: bool,
    /// This image instruments bounded Embassy task poll residence. Ordinary
    /// qualification images deliberately omit this timing perturbation.
    pub task_poll_evidence: bool,
    /// This image also links invasive alternative TX backing and
    /// materialization implementations for same-ELF A/B experiments. Such an
    /// image is not a production residence budget even when the runtime
    /// selects direct SRAM.
    pub tx_architecture_probe: bool,
    /// This image additionally instruments the intrusive Core0 RX phase and
    /// service histograms. It is separate from ordinary task-poll residence.
    pub core0_rx_cycle_evidence: bool,
    /// This image samples MAC interrupt publication timestamps in the hard
    /// ISR. Ordinary correctness and performance images keep that extended
    /// SRAM call graph absent.
    pub mac_irq_evidence: bool,
    /// Ordinary thread/task stacks live in external PSRAM while trap and CLIC
    /// interrupt contexts use dedicated per-hart internal-SRAM stacks.
    pub psram_task_stack: bool,
    /// This image publishes aggregate cooperative network scheduler evidence.
    pub network_scheduler_evidence: bool,
    /// Startup provisioning can select the data-plane executor topology
    /// without requiring another firmware image.
    pub data_plane_placement: bool,
    /// This image can compare Embassy alarm deadlines with the target's
    /// monotonic clock before radio initialization.
    pub timebase_probe: bool,
    /// This image supports bounded pre-initialization CPU/GDMA copy diagnostics.
    pub memory_benchmark: bool,
    /// This image can run the bounded IEEE 802.15.4 `EVENT_STATUS` observation
    /// probe and return its typed snapshots.
    pub ieee802154_event_status_probe: bool,
    /// This image can run the bounded ED-DONE/TIMER0 selective-write
    /// discriminator and retain RX-ABORT diagnostics.
    pub ieee802154_ed_event_probe: bool,
    /// This image can run the single-device IEEE 802.15.4 on-air check.
    pub ieee802154_air_check: bool,
    /// This image can run an IEEE 802.15.4 peer session.
    pub ieee802154_session: bool,
    /// This image can run a Thread session: OpenThread over the composed
    /// IEEE 802.15.4 client.
    pub ieee802154_thread: bool,
    /// This image can run the IEEE 802.15.4 route probe: same-bit arrival
    /// and level retrigger of the source-132 route.
    pub ieee802154_route_probe: bool,
}

/// Bounded alarm/clock agreement probe. It is intentionally independent of
/// Wi-Fi initialization so a broken platform timer cannot qualify radio code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimebaseProbeRequest {
    pub intervals: u16,
    pub period_micros: u32,
}

impl TimebaseProbeRequest {
    pub const fn validate(self) -> bool {
        self.intervals >= 2
            && self.intervals <= 100
            && self.period_micros >= 1_000
            && self.period_micros <= 1_000_000
    }
}

/// Target-side timing evidence for one timebase probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TimebaseProbeEvidence {
    pub intervals: u16,
    pub period_micros: u32,
    pub elapsed_micros: u64,
    pub minimum_interval_micros: u32,
    pub maximum_interval_micros: u32,
    pub early_intervals: u16,
}

#[cfg(feature = "ieee802154")]
mod ieee802154;
#[cfg(feature = "ieee802154")]
pub use ieee802154::*;

mod artifact;
pub use artifact::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub features: FeatureCapabilities,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Inject a terminal read error only at the reached shutdown reader gate.
    FailBluetoothGattResetRead {
        epoch: u32,
    },
    /// Secure HIL only: delay the shutdown Reset reader, without fabricating a response.
    BluetoothGattResetReadGate {
        epoch: u32,
        release: bool,
    },
    /// Secure HIL composition only: fail one real bond-store load after disconnect.
    FailNextBluetoothGattBondLoad {
        epoch: u32,
    },
    /// Restart the secure application Controller epoch, retaining RAM bonds.
    RestartBluetoothGatt {
        epoch: u32,
    },
    QueryBluetoothSecureGatt,
    ConfirmBluetoothGatt(crate::BluetoothNumericDecision),
    /// Observe the shared Trouble application; does not access HCI directly.
    QueryBluetoothGatt,
    /// Diagnostic image only; tests the SoC deadline service, not RF cessation.
    SystemWatchdogTest(crate::WatchdogTestMode),
    /// Query the current platform boot without changing any radio state.
    GetBootStatus,
    /// Page through the previous boot's post-mortem checkpoints from `first`.
    GetPostMortemCheckpoints {
        first: u8,
    },
    /// Diagnostic: stall `HangTarget`'s executor with interrupts enabled, so
    /// the hang watchdog records a post-mortem and resets the chip.
    InjectHang(crate::HangTarget),
    /// Report, start or re-mask the reset-retained event trace.
    TraceControl(crate::TraceControl),
    /// Page through the trace's storage slots from `first`.
    GetTraceEntries {
        first: u16,
    },
    /// Page through the snapshot in `slot` from word `offset`.
    GetTraceSnapshot {
        slot: u8,
        offset: u16,
    },
    PhyFault(crate::PhyFaultCommand),
    /// Suspend, resume or report the shared PHY's periodic tracking timer.
    PhyTracking(crate::PhyTrackingCommand),
    BluetoothDtm(crate::BluetoothDtmOperation),
    /// Raw HCI exchange with the Controller of a `bluetooth_hci` image.
    BluetoothHci(crate::BluetoothHciRequest),
    GetCapabilities,
    /// Return the boot-lifetime CPU stack high-water marks. This diagnostic
    /// query is valid only outside an active traffic session.
    QueryStackUsage,
    /// Sample dedicated IRQ stacks on their own harts in thread mode.
    QueryInterruptStackUsage,
    /// Return boot-lifetime transport and serialized-text health counters.
    QueryLinkHealth,
    /// Compare alarm deadlines with the monotonic clock before initializing
    /// the radio/network runtime.
    ProbeTimebase(TimebaseProbeRequest),
    ProbeMemoryBenchmark(MemoryBenchmarkRequest),
    /// Run one bounded, observation-only IEEE 802.15.4 `EVENT_STATUS` probe.
    ProbeIeee802154EventStatus(Ieee802154EventStatusProbeRequest),
    /// Run the IEEE 802.15.4 same-bit and level-retrigger route probe.
    ProbeIeee802154Route(Ieee802154RouteProbeRequest),
    /// Run the bounded ED-DONE/TIMER0 selective-write discriminator.
    ProbeIeee802154EdEvent(Ieee802154EdEventProbeRequest),
    /// Run the single-device IEEE 802.15.4 on-air check.
    RunIeee802154AirCheck(Ieee802154AirCheckRequest),
    /// Start an IEEE 802.15.4 peer session with this identity and filter.
    StartIeee802154Session(Ieee802154SessionConfig),
    /// Transmit one frame in the running session.
    TransmitIeee802154Session(Ieee802154SessionTransmitRequest),
    /// Enter receive mode; frames accumulate until collected.
    ReceiveIeee802154Session,
    /// Return and forget the frames received since the last collection.
    CollectIeee802154Session,
    /// Change the automatic frame-pending decision.
    SetIeee802154SessionPending(Ieee802154SessionPendingRequest),
    /// Stop the running session.
    StopIeee802154Session,
    /// Run shared PHY tracking now if it is due.
    MaintainIeee802154SessionPhy,
    /// Measure the energy on one channel and assess it once.
    AssessIeee802154SessionChannel(Ieee802154SessionAssessRequest),
    /// Stop the session's client and start it again, as between the air
    /// check's cycles, then apply the session configuration and receive.
    RestartIeee802154SessionRadio,
    /// Read the live RSSI of the most recent baseband reception
    /// (`esp_ieee802154_get_recent_rssi`).
    ReadIeee802154SessionRecentRssi,
    /// Start OpenThread over the composed client and join the dataset's
    /// network.
    StartIeee802154Thread(Ieee802154ThreadStartRequest),
    /// Report the device's Thread interface.
    QueryIeee802154Thread,
    /// Send one datagram from the device's socket.
    SendIeee802154Thread(Ieee802154ThreadSendRequest),
    /// Return and forget the datagrams received since the last collection.
    CollectIeee802154Thread,
    /// Leave the network and stop the client.
    StopIeee802154Thread,
    UploadStartupArtifact(StartupArtifactChunk),
    /// Initialize calibration and the network stack without materializing a
    /// Wi-Fi role. This command is accepted exactly once per boot.
    Initialize(InitializationConfiguration),
    Configure(SessionConfig),
    Arm,
    Start,
    /// Request one connected STA teardown/reassociation cycle. This is a HIL
    /// lifecycle operation, not a transport-session stop and not evidence of
    /// peer link loss.
    CycleStationEpoch,
    /// Stop the active station and return to role-neutral Wi-Fi ownership.
    StopStation,
    /// Materialize a station from the role-neutral Wi-Fi owner.
    StartStation(NetworkCredentials),
    /// Run one finite standalone scan and return to role-neutral ownership.
    ScanWifi(WifiScanRequest),
    /// Materialize a standalone monitor from the role-neutral Wi-Fi owner.
    StartMonitor(WifiMonitorRequest),
    /// Stop the active monitor and return to role-neutral Wi-Fi ownership.
    StopMonitor,
    /// Materialize one bounded WPA2-Personal access point from role-neutral
    /// Wi-Fi ownership. Credentials belong to this AP epoch and are cleared
    /// when the command value is dropped.
    StartAccessPoint(WifiAccessPointRequest),
    /// Stop the active access point and return to role-neutral Wi-Fi ownership.
    StopAccessPoint,
    /// Materialize one same-channel upstream station plus downstream SoftAP.
    StartStationAccessPoint(WifiStationAccessPointRequest),
    /// Stop both paired roles and return every physical owner to idle.
    StopStationAccessPoint,
    /// Run one finite monitor epoch, export its captured frames, return to
    /// idle and publish a terminal capture summary.
    CaptureMonitor(WifiMonitorCaptureRequest),
    /// Return the current operation state and retained session identities.
    GetStatus,
    /// Cancel a configured or armed session before any workload starts.
    Cancel,
    /// Replay the retained evidence for a completed session.
    ReplayResult,
    /// Explicitly discard a terminal result and return to idle ownership.
    Recover,
    AcknowledgeResult,
    /// Take Wi-Fi off the shared radio and bring it up again from
    /// role-neutral ownership.
    RestartRadio,
    /// Read a window of the radio-PHY register image without effect.
    PhyRegisterImage(crate::PhyRegisterImageRequest),
    /// Read a window of the analog image: one analog-I2C read per register.
    PhyAnalogImage(crate::PhyRegisterImageRequest),
}

impl WireBody for Command {
    const WIRE_KIND: WireKind = WireKind::Command;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionState {
    Booting,
    WaitingForInitialization,
    Idle,
    Configured,
    Armed,
    Running,
    Draining,
    Finished,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateChange {
    pub previous: SessionState,
    pub current: SessionState,
}

/// Query result used to recover after an uncertain UART response without
/// guessing whether a session was configured, started or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationStatus {
    pub state: SessionState,
    pub configured_session_id: Option<u64>,
    pub completed_session_id: Option<u64>,
}

#[cfg(feature = "wifi")]
mod wifi;
#[cfg(feature = "wifi")]
pub use wifi::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RejectReason {
    ProtocolVersion,
    BootId,
    SessionId,
    InvalidState,
    InvalidConfiguration,
    Unsupported,
    Busy,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FailureCode {
    Configuration,
    Network,
    Transport,
    Timeout,
    EvidenceOverflow,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkHealth {
    pub rx_frames: u32,
    pub rx_cobs_errors: u32,
    pub rx_checksum_errors: u32,
    pub rx_decode_errors: u32,
    pub rx_overflows: u32,
    pub tx_frames: u32,
    pub tx_dropped: u32,
    pub text_dropped: u32,
    pub text_truncated: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StackWatermark {
    pub capacity_bytes: u32,
    pub free_bytes: u32,
    pub used_bytes: u32,
    pub minimum_free_bytes: u32,
}

impl StackWatermark {
    /// Whether this measured stack retains its nonzero required reserve.
    /// Inconsistent measurements cannot establish headroom.
    pub const fn has_required_headroom(self) -> bool {
        self.minimum_free_bytes > 0
            && self.free_bytes >= self.minimum_free_bytes
            && matches!(self.free_bytes.checked_add(self.used_bytes), Some(total) if total == self.capacity_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StackUsage {
    pub cpu0: StackWatermark,
    pub cpu1: StackWatermark,
    /// Own-hart measurements of dedicated IRQ stacks. `None` means this image
    /// shares that hart's task stack; it never means a failed measurement.
    pub cpu0_irq: Option<StackWatermark>,
    pub cpu1_irq: Option<StackWatermark>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EvidenceRecord {
    Transport(TransportEvidence),
    FlowTransport(FlowTransportEvidence),
    Radio(RadioEvidence),
    TxAggregateTiming(TxAggregateTimingEvidence),
    RxDelivery(RxDeliveryEvidence),
    NetworkScheduler(NetworkSchedulerEvidence),
    Link(LinkHealth),
    Stack(StackUsage),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Event {
    BluetoothGatt(crate::BluetoothGattEvidence),
    BluetoothSecureGatt(crate::BluetoothSecureGattEvidence),
    /// UI decision queued once, not a successful pairing acknowledgement.
    BluetoothGattDecisionRecorded(crate::BluetoothNumericDecision),
    /// The explicit test budget is armed (or completed for `Complete`).
    SystemWatchdogTest(crate::WatchdogTestMode),
    BootStatus(crate::BootEvidence),
    PostMortemCheckpoints(crate::PostMortemCheckpoints),
    /// The stall is armed; the chip resets once the watchdog detects it.
    HangInjected(crate::HangTarget),
    TraceStatus(crate::TraceStatus),
    TraceEntries(crate::TraceEntries),
    /// `None` when the slot holds no snapshot, or another one by now.
    TraceSnapshot(Option<crate::TraceSnapshotPage>),
    PhyFault(crate::PhyFaultEvidence),
    PhyTracking(crate::PhyTrackingEvidence),
    BluetoothDtm(crate::BluetoothDtmEvidence),
    BluetoothHci(crate::BluetoothHciResponse),
    /// AP-epoch modelled service accounting, emitted before the correlated stop.
    WifiAirtimePeer(crate::WifiAirtimePeerEvidence),
    /// Completeness of the preceding bounded peer records for this AP epoch.
    WifiAirtimeReport(crate::WifiAirtimeReport),
    Hello(Capabilities),
    /// The radio and shared Wi-Fi owner are ready in the role-neutral idle
    /// state. The request ID correlates this edge with [`Command::Initialize`].
    Initialized,
    /// Correlated response to [`Command::QueryStackUsage`].
    StackUsage(StackUsage),
    /// `None` denotes no dedicated stack on that hart (shared task stack or inactive hart).
    /// A failed measurement must reject/fail rather than return `None`.
    InterruptStackUsage {
        cpu0: Option<StackWatermark>,
        cpu1: Option<StackWatermark>,
    },
    /// Correlated response to [`Command::QueryLinkHealth`].
    LinkHealth(LinkHealth),
    /// Correlated response to [`Command::ProbeTimebase`].
    TimebaseProbeCompleted(TimebaseProbeEvidence),
    /// Correlated terminal result for one memory-copy diagnostic case.
    MemoryBenchmarkCompleted(MemoryBenchmarkEvidence),
    /// Correlated observation from [`Command::ProbeIeee802154EventStatus`].
    ///
    /// This event does not attest to same-bit concurrency, level-triggered
    /// retrigger behavior, or production interrupt readiness.
    Ieee802154EventStatusProbeCompleted(Ieee802154EventStatusProbeEvidence),
    /// Correlated observation from [`Command::ProbeIeee802154Route`].
    Ieee802154RouteProbeCompleted(Ieee802154RouteProbeEvidence),
    /// Correlated observation from [`Command::ProbeIeee802154EdEvent`].
    Ieee802154EdEventProbeCompleted(Ieee802154EdEventProbeEvidence),
    /// Correlated observation from [`Command::RunIeee802154AirCheck`].
    ///
    /// It records single-device outcomes only; it does not attest to a peer
    /// receiving the transmitted frames or to calibrated output power.
    Ieee802154AirCheckCompleted(Ieee802154AirCheckEvidence),
    /// Correlated result of [`Command::StartIeee802154Session`].
    Ieee802154SessionStarted(Ieee802154SessionResult),
    /// Correlated result of [`Command::TransmitIeee802154Session`].
    Ieee802154SessionTransmitted(Ieee802154SessionTransmitEvidence),
    /// Correlated result of [`Command::CollectIeee802154Session`].
    Ieee802154SessionReceived(Ieee802154SessionReceiveEvidence),
    /// Correlated result of [`Command::StopIeee802154Session`].
    Ieee802154SessionStopped(Ieee802154SessionStopEvidence),
    /// Correlated result of [`Command::MaintainIeee802154SessionPhy`].
    Ieee802154SessionPhyMaintained(Ieee802154SessionPhyMaintenance),
    /// Correlated result of [`Command::AssessIeee802154SessionChannel`].
    Ieee802154SessionAssessed(Ieee802154SessionAssessment),
    /// Correlated result of [`Command::RestartIeee802154SessionRadio`].
    Ieee802154SessionRadioRestarted(Ieee802154SessionRestartEvidence),
    /// Correlated result of [`Command::ReadIeee802154SessionRecentRssi`].
    Ieee802154SessionRecentRssi(Ieee802154SessionRecentRssi),
    /// Correlated result of [`Command::StartIeee802154Thread`].
    Ieee802154ThreadStarted(Ieee802154SessionResult),
    /// Correlated result of [`Command::QueryIeee802154Thread`].
    Ieee802154ThreadState(Ieee802154ThreadState),
    /// Correlated result of [`Command::SendIeee802154Thread`].
    Ieee802154ThreadSent(Ieee802154SessionResult),
    /// Correlated result of [`Command::CollectIeee802154Thread`].
    Ieee802154ThreadReceived(Ieee802154ThreadReceiveEvidence),
    /// Correlated result of [`Command::StopIeee802154Thread`].
    Ieee802154ThreadStopped(Ieee802154SessionResult),
    Accepted,
    Rejected(RejectReason),
    State(StateChange),
    /// Correlated response to [`Command::GetStatus`].
    OperationStatus(OperationStatus),
    /// Reliable completion acknowledgement for `CycleStationEpoch`.
    /// The envelope request ID identifies the command being completed.
    StationEpochCompleted(StationEpochEvidence),
    /// Reliable completion of a Wi-Fi role transition or idle radio restart.
    WifiRoleTransitioned(WifiRoleTransitionEvidence),
    /// Reliable completion of one finite `ScanWifi` request.
    WifiScanCompleted(WifiScanEvidence),
    /// Reliable completion of `StartMonitor`.
    WifiMonitorStarted(WifiRoleTransitionEvidence),
    /// Reliable completion of `StopMonitor` and its bounded capture summary.
    WifiMonitorStopped(WifiMonitorEvidence),
    /// Reliable completion of `StartAccessPoint`.
    WifiAccessPointStarted(WifiRoleTransitionEvidence),
    /// Reliable completion of `StopAccessPoint` and its bounded AP summary.
    WifiAccessPointStopped(WifiAccessPointEvidence),
    /// Reliable completion of `StopStationAccessPoint` with the AP-side
    /// timing report retained from the same physical epoch.
    WifiStationAccessPointStopped(WifiStationAccessPointStopEvidence),
    /// Terminal failure of a correlated role start/stop command.
    WifiRoleFailed(WifiRoleFailureEvidence),
    /// One ordered chunk emitted by `CaptureMonitor`.
    WifiMonitorFrame(WifiMonitorFrameChunk),
    /// Terminal completion of one finite `CaptureMonitor` request.
    WifiMonitorCaptureCompleted(WifiMonitorEvidence),
    /// Unsolicited, reliable station generation/link transition.
    StationLifecycle(StationLifecycleEvent),
    NetworkReady(NetworkInfo),
    ServiceReady(ServiceInfo),
    /// The selected data-plane worker has consumed the session configuration
    /// and is ready for host traffic in this direction.
    SessionReady(SessionReady),
    /// Single-flow UDP consumer reached its first 256 valid data packets.
    /// Session-correlated delivery, distinct from socket readiness or host send.
    UdpRxStarted {
        datagrams: u64,
    },
    Evidence(EvidenceRecord),
    Finished(Finished),
    Failed(FailureCode),
    StartupArtifactReady(StartupArtifactStatus),
    StartupArtifact(StartupArtifactChunk),
    /// Reliable completion of an idle Wi-Fi restart on the shared radio.
    WifiRadioRestarted(WifiRadioRestartEvidence),
    /// Correlated response to [`Command::PhyRegisterImage`].
    PhyRegisterImage(crate::PhyRegisterImageWords),
    /// Correlated response to [`Command::PhyAnalogImage`].
    PhyAnalogImage(crate::PhyAnalogImageBytes),
}

impl WireBody for Event {
    const WIRE_KIND: WireKind = WireKind::Event;
}

#[cfg(test)]
mod tests;
