use crate::{MemoryBenchmarkEvidence, MemoryBenchmarkRequest};
use core::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 168;
/// Maximum number of independently accounted transport flows in one network
/// interface session.
///
/// Two flows are sufficient for the first physical multi-client AP cell. The
/// fixed bound keeps the no-alloc wire contract explicit and does not change
/// the target rule that one session owns one network interface.
pub const SESSION_FLOW_CAPACITY: usize = 2;
// Keep command envelopes small: startup artifacts are transferred as an
// ordered CRC-protected stream, so a large per-command inline buffer only
// inflates UART queues and executor futures without improving semantics.
pub const STARTUP_ARTIFACT_CHUNK_MAX_LEN: usize = 160;
// Keep the largest protocol enum comfortably below one RX frame. This value
// bounds executor poll-stack pressure as well as wire latency; complete MPDUs
// are reconstructed from ordered chunks on the host.
pub const WIFI_MONITOR_FRAME_CHUNK_MAX_LEN: usize = 160;
pub const WPA2_SSID_MAX_LEN: usize = 32;
pub const WPA2_PASSPHRASE_MIN_LEN: usize = 8;
pub const WPA2_PASSPHRASE_MAX_LEN: usize = 63;

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

mod session;
pub use session::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureCapabilities {
    /// Numeric-Comparison-only application, RAM bonds and explicit UI decisions.
    #[serde(default)]
    pub bluetooth_secure_gatt: bool,
    /// Plaintext Trouble GATT application with observation-only HIL control.
    #[serde(default)]
    pub bluetooth_gatt: bool,
    /// Bounded connectable advertising and peripheral execution observations.
    pub bluetooth_peripheral: bool,
    /// Automatic production PHY maintenance with explicit diagnostic budgets.
    #[serde(default)]
    pub bluetooth_phy_maintenance: bool,
    /// Diagnostic MWDT reset during DTM; not automatic PHY deadline enforcement.
    #[serde(default)]
    pub bluetooth_watchdog_reset: bool,
    /// Independent SoC deadline diagnostic; requires no radio protocol.
    #[serde(default)]
    pub system_watchdog: bool,
    /// Destructive checkpoints in actual PHY maintenance; diagnostic only.
    #[serde(default)]
    pub phy_fault_injection: bool,
    pub bluetooth_dtm: bool,
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
    /// Explicit same-connection MAC/RX/IRQ pause without recalibration.
    pub station_pause: bool,
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

mod ieee802154;
pub use ieee802154::*;

mod artifact;
pub use artifact::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkCredentialsError {
    SsidLength,
    PassphraseLength,
}

impl fmt::Display for NetworkCredentialsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SsidLength => formatter.write_str("SSID must contain 1..=32 bytes"),
            Self::PassphraseLength => {
                formatter.write_str("WPA2 passphrase must contain 8..=63 bytes")
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkCredentials {
    ssid: [u8; WPA2_SSID_MAX_LEN],
    ssid_length: u8,
    passphrase: heapless::Vec<u8, WPA2_PASSPHRASE_MAX_LEN>,
}

impl NetworkCredentials {
    pub fn try_new(ssid: &[u8], passphrase: &[u8]) -> Result<Self, NetworkCredentialsError> {
        if ssid.is_empty() || ssid.len() > WPA2_SSID_MAX_LEN {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        let mut credentials = Self {
            ssid: [0; WPA2_SSID_MAX_LEN],
            ssid_length: ssid.len() as u8,
            passphrase: heapless::Vec::new(),
        };
        credentials.ssid[..ssid.len()].copy_from_slice(ssid);
        credentials
            .passphrase
            .extend_from_slice(passphrase)
            .map_err(|_| NetworkCredentialsError::PassphraseLength)?;
        Ok(credentials)
    }

    pub fn validate(&self) -> Result<(), NetworkCredentialsError> {
        let ssid_length = usize::from(self.ssid_length);
        if ssid_length == 0 || ssid_length > self.ssid.len() {
            return Err(NetworkCredentialsError::SsidLength);
        }
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&self.passphrase.len()) {
            return Err(NetworkCredentialsError::PassphraseLength);
        }
        Ok(())
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_length)]
    }

    pub fn passphrase(&self) -> &[u8] {
        self.passphrase.as_slice()
    }

    pub fn clear_passphrase(&mut self) {
        self.passphrase.as_mut_slice().zeroize();
        self.passphrase.clear();
    }
}

impl fmt::Debug for NetworkCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NetworkCredentials")
            .field("ssid_length", &self.ssid_length)
            .field("passphrase", &"<redacted>")
            .finish()
    }
}

impl Drop for NetworkCredentials {
    fn drop(&mut self) {
        self.ssid.zeroize();
        self.ssid_length = 0;
        self.clear_passphrase();
    }
}

/// IPv4 policy selected by the host for this boot.
///
/// Keeping this in startup provisioning lets one qualified firmware image run
/// against both an ordinary DHCP network and an isolated HIL access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NetworkIpv4Configuration {
    Dhcp,
    Static {
        address: [u8; 4],
        prefix_length: u8,
        gateway: Option<[u8; 4]>,
    },
}

impl NetworkIpv4Configuration {
    pub fn validate(self) -> bool {
        match self {
            Self::Dhcp => true,
            Self::Static {
                address,
                prefix_length,
                gateway,
            } => {
                prefix_length <= 32
                    && address != [0, 0, 0, 0]
                    && address != [255, 255, 255, 255]
                    && match gateway {
                        Some(gateway) => gateway != [0, 0, 0, 0] && gateway != [255, 255, 255, 255],
                        None => true,
                    }
            }
        }
    }
}

/// Executor placement selected once, before any Wi-Fi worker or IP stack is
/// materialized. Radio and RX protocol ownership remain on CPU0 in both modes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiDataPlanePlacement {
    /// Radio, RX protocol, IP stack and socket workloads share CPU0.
    SingleCore,
    /// Only the IP stack and socket workloads move to CPU1.
    #[default]
    SplitRadioNetwork,
}

/// IPv4/UDP receive checksum policy selected before the network stack starts.
///
/// The diagnostic variant exists only for a same-image HIL cost experiment;
/// it is not a claim that the Wi-Fi MAC performs transport checksum offload.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxChecksumPolicy {
    /// Validate IPv4 and UDP receive checksums in the software IP stack.
    #[default]
    Software,
    /// Trust the isolated HIL traffic generator and skip IPv4/UDP RX checks.
    AssumeValidDiagnostic,
}

/// IPv4 UDP transmit checksum policy selected before the network stack starts.
///
/// IPv4 permits a zero UDP checksum. The diagnostic variant uses that wire
/// representation to isolate software checksum cost without claiming hardware
/// offload or disabling the mandatory IPv4 header checksum.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxUdpChecksumPolicy {
    /// Generate the IPv4 UDP checksum in the software IP stack.
    #[default]
    Software,
    /// Emit a zero IPv4 UDP checksum for a same-image HIL cost experiment.
    OmitIpv4Diagnostic,
}

/// TX storage policy selected for a same-image hardware experiment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiTxBufferPolicy {
    /// Queue an owned general-memory packet, select its radio flow, then copy
    /// it once into the fixed internal-SRAM execution pool.
    #[default]
    OwnedSramPromotion,
    /// Keep owned-packet promotion, but have the HIL producer publish one bounded
    /// destination-homogeneous burst at a time. This isolates packet-selection
    /// order from physical SRAM capacity without changing the radio datapath.
    OwnedSramPromotionBurstDiagnostic,
    /// Keep Wi-Fi descriptors in internal SRAM but publish PSRAM packet-buffer
    /// addresses after an explicit cache writeback. Hardware support is not
    /// assumed; this value exists only for the bounded DMA-address HIL.
    PsramDirectDmaDiagnostic,
}

/// Continuation policy for a masked RX drain epoch in one coarse image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiRxContinuationPolicy {
    /// Preserve the production immediate software repost.
    #[default]
    ImmediateSoftwareProbe,
    /// Restore the level-triggered source after a recycled-only turn.
    LevelIrqDiagnostic,
    /// Retain source masking and repoll after 64 microseconds.
    DelayedProbe64Diagnostic,
    /// Retain source masking and repoll after 128 microseconds.
    DelayedProbe128Diagnostic,
    /// Retain source masking and repoll after 256 microseconds.
    DelayedProbe256Diagnostic,
    /// Retain source masking and repoll after 512 microseconds.
    DelayedProbe512Diagnostic,
    /// Retain source masking and repoll after 1024 microseconds.
    DelayedProbe1024Diagnostic,
    /// Select a bounded window from the completed physical batch geometry.
    AdaptiveProbeDiagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InitializationConfiguration {
    pub ap_scheduler: WifiApScheduler,
    pub ipv4: NetworkIpv4Configuration,
    pub data_plane: WifiDataPlanePlacement,
    pub rx_checksum: WifiRxChecksumPolicy,
    pub tx_udp_checksum: WifiTxUdpChecksumPolicy,
    pub tx_buffer: WifiTxBufferPolicy,
    pub rx_continuation: WifiRxContinuationPolicy,
    pub l1_cache_counters: bool,
}

/// Standalone AP scheduling experiment with one explicit response envelope.
/// Both arms use 3000-us quantum, 100-us minimum and a 32-byte OFDM24 response
/// plus 10-us SIFS per unicast publication. This is not measured airtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiApScheduler {
    #[default]
    Disabled,
    RrHtResponse24,
    DeficitHtResponse24,
}

impl InitializationConfiguration {
    pub fn validate(self) -> bool {
        self.ipv4.validate()
    }
}

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
    PhyFault(crate::PhyFaultCommand),
    BluetoothPeripheral(crate::BluetoothPeripheralOperation),
    BluetoothDtm(crate::BluetoothDtmOperation),
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
    /// Run the bounded ED-DONE/TIMER0 selective-write discriminator.
    ProbeIeee802154EdEvent(Ieee802154EdEventProbeRequest),
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
    /// Pause/resume the connected station, including during an active session.
    PauseStation {
        operation: StationPauseOperation,
    },
    /// Release the final Wi-Fi PHY client from role-neutral ownership, close
    /// the RF epoch, and start a fresh cold radio epoch.
    RestartRadio,
    /// Close and restore RF from role-neutral ownership while retaining the
    /// registered PHY calibration epoch.
    CycleRetainedRadio,
}

/// Work performed while the connected station retains its paused epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StationPauseOperation {
    Access,
    /// Matched pause control, bounded to 1..=200000 us by host and target.
    Synthetic {
        duration_micros: u32,
        notify_ap: bool,
    },
    Tracking,
    Calibration,
    Temperature,
    WifiPower,
    WifiI2c,
    CommonCalibration,
    TxCalibration,
    TrackingService,
    Rfpll,
    RfpllCheck,
    /// RFPLL evaluation requiring a recent completed sensor acquisition.
    RfpllObserved,
}

/// Outcome of a correlated physical pause round trip. Busy is not success.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationPauseResult {
    Resumed,
    PeerNotification,
    InvalidDuration,
    Unavailable,
    Busy,
    Interrupted,
    MacStop,
    RxBusy,
    RxPause,
    IrqPause,
    RxResume,
    IrqResume,
    RegisterReclaim,
    PhyAdmission,
    PhyRelease,
    RegisterRepublish,
    PhyTracking,
    MacRestoration,
    ReceivePolicyChanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationPhyTrackingEvidence {
    pub inhibited: bool,
    pub common_calibrated: bool,
    pub wifi_calibrated: bool,
    pub bluetooth_ieee802154_calibrated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationPauseEvidence {
    /// Full owner-handoff timeline; absent in compact images or aggregate reports.
    pub timeline: Option<crate::StationPauseTimeline>,
    /// None when timing observers are unavailable or no physical report returned.
    pub timings: Option<crate::PhyTimingEvidence>,
    pub tracking: Option<StationPhyTrackingEvidence>,
    pub result: StationPauseResult,
    pub elapsed_micros: u64,
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

mod wifi;
pub use wifi::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub network_interface: WifiNetworkInterface,
    pub address: [u8; 4],
    pub prefix_length: u8,
    pub gateway: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub network_interface: WifiNetworkInterface,
    pub transport: Transport,
    pub direction: Direction,
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
}

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
pub struct TransportEvidence {
    /// Complete observation-window silence, including its trailing interval.
    /// Present only for a single measured UDP RX flow; never inferred from throughput.
    pub rx_maximum_silence_micros: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

/// Transport accounting for one configured [`SessionFlowConfig`].
///
/// The session-wide [`TransportEvidence`] remains the independently checked
/// sum used by existing ceiling reports. Fairness verdicts consume these
/// records and must never infer a peer split from the aggregate.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlowTransportEvidence {
    pub rx_maximum_silence_micros: Option<u64>,
    pub flow_id: u8,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
}

impl FlowTransportEvidence {
    pub const fn from_session_total(flow_id: u8, total: TransportEvidence) -> Self {
        Self {
            rx_maximum_silence_micros: total.rx_maximum_silence_micros,
            flow_id,
            rx_bytes: total.rx_bytes,
            tx_bytes: total.tx_bytes,
            rx_units: total.rx_units,
            tx_units: total.tx_units,
            elapsed_micros: total.elapsed_micros,
            transport_errors: total.transport_errors,
        }
    }

    pub const fn as_session_total(self) -> TransportEvidence {
        TransportEvidence {
            rx_maximum_silence_micros: self.rx_maximum_silence_micros,
            rx_bytes: self.rx_bytes,
            tx_bytes: self.tx_bytes,
            rx_units: self.rx_units,
            tx_units: self.tx_units,
            elapsed_micros: self.elapsed_micros,
            transport_errors: self.transport_errors,
        }
    }
}

impl TransportEvidence {
    pub fn from_flows(flows: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY]) -> Self {
        // One missing flow observation must not masquerade as a complete
        // session value. Multi-flow users consume per-flow evidence instead.
        let mut active = flows.iter().flatten();
        let first = active
            .next()
            .and_then(|flow| flow.rx_maximum_silence_micros);
        let silence = if active.next().is_none() { first } else { None };
        flows.iter().flatten().copied().fold(
            Self {
                rx_maximum_silence_micros: silence,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_units: 0,
                tx_units: 0,
                elapsed_micros: 0,
                transport_errors: 0,
            },
            |mut total, flow| {
                total.rx_bytes = total.rx_bytes.saturating_add(flow.rx_bytes);
                total.tx_bytes = total.tx_bytes.saturating_add(flow.tx_bytes);
                total.rx_units = total.rx_units.saturating_add(flow.rx_units);
                total.tx_units = total.tx_units.saturating_add(flow.tx_units);
                total.elapsed_micros = total.elapsed_micros.max(flow.elapsed_micros);
                total.transport_errors =
                    total.transport_errors.saturating_add(flow.transport_errors);
                total
            },
        )
    }
}

/// Minimum typed radio evidence needed by ordinary UDP qualification. Richer
/// timing histograms remain diagnostic telemetry and never decide PASS/FAIL.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RadioEvidence {
    pub rx: Option<RxRadioEvidence>,
    pub tx: Option<TxRadioEvidence>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxRadioEvidence {
    /// ESP hardware RX format code observed for the measured interval.
    pub phy_format: u8,
    /// Complete benchmark-UDP HT40 observations at 800 ns GI.
    pub ht40_long_gi_frames: u32,
    /// Complete benchmark-UDP HT40 observations at 400 ns GI.
    pub ht40_short_gi_frames: u32,
    /// HT40 observations below MCS7. This is a subset of the two GI totals.
    pub ht40_below_mcs7_frames: u32,
    /// Benchmark vectors outside HT40 MCS0..7 geometry/format.
    pub ht_invalid_frames: u32,
    pub dma_buffer_full: u32,
    pub dma_fifo_overflow: u32,
    pub network_dropped: u32,
    pub irq_drain_saturated: u32,
    pub unhandled_irq_entries: u32,
    pub sequence_first: Option<u32>,
    pub sequence_highest: Option<u32>,
    pub sequence_gap_events: u32,
    pub sequence_forward_missing: u32,
    pub sequence_backward: u32,
    pub sequence_duplicates: u32,
    pub sequence_unsequenced: u32,
    pub s_mpdu_datagrams: u32,
    pub not_s_mpdu_datagrams: u32,
    pub s_mpdu_unavailable_datagrams: u32,
    pub s_mpdu_beacons: u32,
    pub not_s_mpdu_beacons: u32,
    pub s_mpdu_unavailable_beacons: u32,
    pub ampdu_datagrams: u32,
    pub not_ampdu_datagrams: u32,
    pub hardware_ampdu_datagrams: u32,
    pub hardware_not_ampdu_datagrams: u32,
    pub protocol_ampdu_datagrams: u32,
    pub protocol_not_ampdu_datagrams: u32,
    pub ampdu_unavailable_datagrams: u32,
    pub reorder_tid: u8,
    pub reorder_window: u16,
    pub reorder_first_samples: u32,
    pub reorder_first_tid: u8,
    pub reorder_first_start: u16,
    pub reorder_first_sequence: u16,
    pub reorder_first_distance: u16,
    pub reorder_current_occupied: u32,
    pub reorder_maximum_occupied: u32,
    pub rx_service_calls: u32,
    pub rx_frontier_histogram_samples: u32,
    pub mac_irq_entries: u32,
    pub mac_irq_classified_entries: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TxRadioEvidence {
    /// Station-only terminal receipts within the snapshot interval. These do
    /// not include live or quarantined exchanges, nor prove UDP host delivery.
    pub station_terminal: StationTxTerminalEvidence,
    pub bandwidth_mhz: u16,
    pub aggregate_rate_kbps: u32,
    pub aggregates_prepared: u32,
    pub aggregate_publications: u32,
    /// Outstanding publications at the two coherent radio-executor snapshots.
    pub publications_pending_start: u32,
    pub publications_pending_end: u32,
    pub aggregates_completed: u32,
    pub subframes_prepared: u32,
    pub subframes_acknowledged: u32,
    pub individual_retries: u32,
    pub hardware_timeouts: u32,
    pub collisions: u32,
    pub minimum_subframes: u8,
    pub maximum_subframes: u8,
    pub prepared_histogram: [u32; 8],
    pub stopped_at_frame_limit: u32,
    pub stopped_at_capacity_limit: u32,
    pub stopped_on_empty_queue: u32,
    pub block_ack_samples: u32,
    /// Publications for which hardware reported a physically received
    /// BlockAck frame. This is independent of bitmap coverage: a received
    /// BlockAck may acknowledge zero subframes.
    pub block_ack_received: u32,
    pub success_without_block_ack: u32,
    pub nonzero_block_ack_control: u32,
    /// BlockAck-processing samples classified by acknowledged bitmap
    /// coverage. `full + partial + empty == block_ack_samples`; `empty` also
    /// includes publications for which no BlockAck frame was received.
    pub full_block_ack: u32,
    pub partial_block_ack: u32,
    pub empty_block_ack: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_publication_to_irq_samples: u32,
}

/// Timing and software-pipeline evidence for one bounded aggregate-TX interval.
///
/// This is separate from [`TxRadioEvidence`]: radio correctness and timing are
/// independently complete protocol records, and neither is reconstructed from
/// best-effort UART text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TxAggregateTimingEvidence {
    pub preparation_micros: u32,
    pub preparation_max_micros: u32,
    pub publication_micros: u32,
    pub publication_max_micros: u32,
    pub exchange_micros: u32,
    pub exchange_max_micros: u32,
    pub first_exchanges: u32,
    pub first_exchange_micros: u32,
    pub first_exchange_max_micros: u32,
    pub retried_exchanges: u32,
    pub retry_publications: u32,
    pub retry_exchange_micros: u32,
    pub retry_exchange_max_micros: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_irq_service_micros: u32,
    pub tx_irq_service_max_micros: u32,
    pub tx_publication_to_irq_samples: u32,
    pub tx_publication_to_irq_micros: u32,
    pub tx_publication_to_irq_max_micros: u32,
    pub standby_prepared: u32,
    pub standby_published: u32,
    pub standby_cancelled: u32,
    /// Prepared owners retained across the measurement boundaries.
    pub standby_pending_start: u32,
    pub standby_pending_end: u32,
}

/// Sequence evidence collected at one finite UDP RX delivery stage.
///
/// Qualification traffic uses non-negative, non-wrapping `i32` sequence
/// numbers. Negative control markers are counted separately and never enter
/// the data-unit accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxSequenceStageEvidence {
    pub data_units: u32,
    pub first: Option<u32>,
    pub highest: Option<u32>,
    pub gap_events: u32,
    pub forward_missing: u32,
    pub late_recovered: u32,
    pub duplicates: u32,
    pub backward_unclassified: u32,
    pub first_anomaly: Option<u32>,
    pub control_markers: u32,
    pub data_after_terminal: u32,
}

/// Exact reconciliation of successful network admissions with UDP socket
/// consumption through a bounded qualification-only shadow ledger.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxConsumerLedgerEvidence {
    pub matched: u32,
    pub enqueued_not_consumed: u32,
    pub skipped_before_observed: u32,
    pub unexpected_consumer: u32,
    pub overflow: u32,
    pub first_expected: Option<u32>,
    pub first_observed: Option<u32>,
}

/// Correlation of application-level ordering defects with public QoS MAC
/// sequence/TID progression at the post-reorder frontier.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxMacOrderEvidence {
    /// First forward UDP gap with adjacent observations on the same QoS TID.
    /// MAC values are 12-bit sequence numbers, not an inferred loss count.
    pub first_forward_gap: Option<RxForwardGapEvidence>,
    pub backward_mac_backward: u32,
    pub backward_mac_same: u32,
    pub backward_mac_forward: u32,
    pub backward_mac_other_tid: u32,
    pub backward_mac_unavailable: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxForwardGapEvidence {
    pub previous_udp: u32,
    pub current_udp: u32,
    pub tid: u8,
    pub previous_mac: u16,
    pub current_mac: u16,
}

/// Reorder decisions relevant to delivery loss during one session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxReorderDeliveryEvidence {
    pub ingress: u32,
    pub ingress_retries: u32,
    pub direct: u32,
    pub buffered: u32,
    pub released: u32,
    pub missing: u32,
    pub stale: u32,
    pub gap_expiries: u32,
    pub maximum_occupied: u32,
    pub discarded: u32,
}

/// Complete typed evidence for the three UDP RX delivery frontiers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RxDeliveryEvidence {
    pub post_reorder: RxSequenceStageEvidence,
    pub network_enqueued: RxSequenceStageEvidence,
    pub udp_consumer: RxSequenceStageEvidence,
    pub consumer_ledger: RxConsumerLedgerEvidence,
    pub mac_order: RxMacOrderEvidence,
    pub reorder: RxReorderDeliveryEvidence,
    pub network_queue_full: u32,
    pub network_invalid_length: u32,
    /// A receive packet owner could not be acquired.
    pub network_pool_exhausted: u32,
    /// The logical network endpoint was inactive at admission.
    pub network_link_down: u32,
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

/// Aggregate cooperative network scheduler evidence collected since boot.
///
/// Diagnostic images are cold-booted for each qualification cell, so this is
/// also the complete scheduler interval for that cell. No per-packet trace is
/// transported over the control link.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkSchedulerEvidence {
    pub polls: u32,
    pub ingress_calls: u32,
    pub ingress_packets: u32,
    pub egress_passes: u32,
    pub egress_tx_tokens: u32,
    pub egress_blocked: u32,
    pub ingress_budget_exhausted: u32,
    pub egress_budget_exhausted: u32,
    pub started_with_ingress: u32,
    pub started_with_egress: u32,
    pub exit_drained: u32,
    pub exit_work_budget: u32,
    pub exit_egress_credit: u32,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResultSummary {
    pub passed: bool,
    pub evidence_records: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    pub summary: ResultSummary,
    pub evidence_crc32c: u32,
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
    PhyFault(crate::PhyFaultEvidence),
    BluetoothPeripheral(crate::BluetoothPeripheralEvidence),
    BluetoothDtm(crate::BluetoothDtmEvidence),
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
    /// Correlated observation from [`Command::ProbeIeee802154EdEvent`].
    Ieee802154EdEventProbeCompleted(Ieee802154EdEventProbeEvidence),
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
    StationPauseCompleted(StationPauseEvidence),
    StationPhyTxWaits(crate::PhyTxWaitEvidence),
    StationPhyRxGain(crate::PhyRxGainEvidence),
    StationTemperatureObserved(crate::TemperatureEvidence),
    StationRfpllObserved(crate::RfpllEvidence),
    StationTrackingService(crate::StationTrackingServiceEvidence),
    StationTimerObserved(crate::TimerWindowEvidence),
    /// Reliable completion of an idle whole-radio cold restart.
    WifiRadioRestarted(WifiRadioRestartEvidence),
    /// Reliable completion of an idle retained RF close/wake cycle.
    WifiRadioRetainedCycled(WifiRadioRetainedCycleEvidence),
}

impl WireBody for Event {
    const WIRE_KIND: WireKind = WireKind::Event;
}

#[cfg(test)]
mod tests;

/// Logical station A-MPDU results after all aggregate and detached retries.
/// An unacknowledged MPDU may still have arrived when its ACK was lost.
/// Snapshots are not a drain barrier and exclude unfinished exchanges.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationTxTerminalEvidence {
    pub exchanges: u32,
    pub mpdus: u32,
    pub acknowledged: u32,
    pub unacknowledged: u32,
    pub ordinary_recovered: u32,
    pub ordinary_failed: u32,
    pub invalid_statuses: u32,
}
