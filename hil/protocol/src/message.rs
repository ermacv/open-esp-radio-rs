use core::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 32;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Transport {
    Udp,
    Tcp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Direction {
    Rx,
    Tx,
    Bidirectional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Completion {
    DurationMillis(u32),
    TransferBytes(u64),
    HostStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Ipv4Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FlowConfig {
    pub payload_bytes: u16,
    pub offered_rate_bps: Option<u64>,
}

/// Link properties that must be true before a measured transport session may
/// advertise readiness.
///
/// Correctness cells deliberately use [`Self::NONE`]: a standards-compliant
/// peer may reject aggregation and the data plane must still work. Throughput
/// cells can require one negotiated TX BlockAck TID so an absent AddBA
/// response is reported as unavailable test precondition instead of being
/// misclassified as slow S-MPDU performance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionLinkRequirements {
    pub tx_block_ack_tid: Option<u8>,
}

impl SessionLinkRequirements {
    pub const NONE: Self = Self {
        tx_block_ack_tid: None,
    };

    pub const fn tx_block_ack(tid: u8) -> Self {
        Self {
            tx_block_ack_tid: Some(tid),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub transport: Transport,
    pub direction: Direction,
    pub completion: Completion,
    pub peer: Option<Ipv4Endpoint>,
    pub target_rx: Option<FlowConfig>,
    pub target_tx: Option<FlowConfig>,
    pub link_requirements: SessionLinkRequirements,
}

/// Data-plane readiness together with the exact link requirement proved by
/// the target before accepting measured traffic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionReady {
    pub direction: Direction,
    pub tx_block_ack_tid: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FeatureCapabilities {
    pub udp: bool,
    pub tcp: bool,
    pub rx: bool,
    pub tx: bool,
    pub bidirectional: bool,
    pub runtime_initialization: bool,
    pub runtime_configuration: bool,
    pub structured_evidence: bool,
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
    /// This image can run one finite normalized monitor capture and export
    /// typed frame chunks without using UART text as a data protocol.
    pub wifi_monitor_capture: bool,
    /// This image reliably reports connected generations and proved peer-loss
    /// transitions independently of lossy text diagnostics.
    pub station_lifecycle_events: bool,
    /// UDP RX sessions can return typed evidence for every delivery frontier
    /// from post-reorder publication through the application socket.
    pub rx_delivery_evidence: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupArtifactChunkError {
    EmptyArtifact,
    EmptyChunk,
    ChunkTooLarge,
    Range,
}

impl fmt::Display for StartupArtifactChunkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArtifact => formatter.write_str("startup artifact must not be empty"),
            Self::EmptyChunk => formatter.write_str("startup artifact chunk must not be empty"),
            Self::ChunkTooLarge => formatter.write_str("startup artifact chunk is too large"),
            Self::Range => formatter.write_str("startup artifact chunk range is invalid"),
        }
    }
}

/// One ordered fragment of a target-defined, host-owned startup artifact.
///
/// The protocol deliberately assigns no meaning to the artifact bytes. The
/// selected target adapter owns the exact length, validation and conversion
/// into a typed runtime value. Chunks are contiguous and carry a digest of the
/// complete artifact so a receiver never treats a partial transfer as valid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartupArtifactChunk {
    total_length: u16,
    offset: u16,
    crc32c: u32,
    bytes: heapless::Vec<u8, STARTUP_ARTIFACT_CHUNK_MAX_LEN>,
}

impl StartupArtifactChunk {
    pub fn try_new(
        total_length: u16,
        offset: u16,
        crc32c: u32,
        bytes: &[u8],
    ) -> Result<Self, StartupArtifactChunkError> {
        let mut chunk = Self {
            total_length,
            offset,
            crc32c,
            bytes: heapless::Vec::new(),
        };
        chunk
            .bytes
            .extend_from_slice(bytes)
            .map_err(|_| StartupArtifactChunkError::ChunkTooLarge)?;
        chunk.validate()?;
        Ok(chunk)
    }

    pub fn validate(&self) -> Result<(), StartupArtifactChunkError> {
        if self.total_length == 0 {
            return Err(StartupArtifactChunkError::EmptyArtifact);
        }
        if self.bytes.is_empty() {
            return Err(StartupArtifactChunkError::EmptyChunk);
        }
        let end = usize::from(self.offset)
            .checked_add(self.bytes.len())
            .ok_or(StartupArtifactChunkError::Range)?;
        if end > usize::from(self.total_length) {
            return Err(StartupArtifactChunkError::Range);
        }
        Ok(())
    }

    pub const fn total_length(&self) -> u16 {
        self.total_length
    }

    pub const fn offset(&self) -> u16 {
        self.offset
    }

    pub const fn crc32c(&self) -> u32 {
        self.crc32c
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.total_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StartupArtifactDisposition {
    /// No retained artifact was supplied; initialization created one.
    Created,
    /// The supplied artifact was accepted and restored.
    Restored,
    /// The supplied artifact was rejected and full initialization replaced it.
    Replaced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartupArtifactStatus {
    pub disposition: StartupArtifactDisposition,
    pub total_length: u16,
    pub initialization_elapsed_micros: u64,
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capabilities {
    pub features: FeatureCapabilities,
    pub maximum_payload_bytes: u16,
    pub maximum_wire_frame_bytes: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Command {
    GetCapabilities,
    /// Return the boot-lifetime CPU stack high-water marks. This diagnostic
    /// query is valid only outside an active traffic session.
    QueryStackUsage,
    UploadStartupArtifact(StartupArtifactChunk),
    /// Initialize calibration and the network stack without materializing a
    /// Wi-Fi role. This command is accepted exactly once per boot.
    Initialize(NetworkIpv4Configuration),
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

/// Target-observed ownership edges for one requested station epoch cycle.
///
/// This is deliberately semantic rather than target-specific: the target
/// adapter may implement the individual operations differently, but it may
/// publish completion only after every owned resource crossed these four
/// finite boundaries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationEpochEvidence {
    pub runner_stopped: bool,
    pub scan_owners_returned: bool,
    pub join_completed: bool,
    pub connected_runner_started: bool,
}

/// Wi-Fi role represented by the target-side application owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRole {
    Idle,
    Station,
    Monitor,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRoleOperation {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiRoleFailureReason {
    Rejected,
    HardwareFault,
    GenerationMismatch,
}

/// Terminal correlated failure for a role command. This prevents the host
/// from turning a target-side ownership fault into an opaque timeout.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiRoleFailureEvidence {
    pub role: WifiRole,
    pub operation: WifiRoleOperation,
    pub reason: WifiRoleFailureReason,
}

/// Complete target-neutral configuration for the first AP role.
///
/// Beacon interval (100 TU), DTIM period (2), HT20 width and the one-peer
/// limit are driver guarantees rather than HIL tuning parameters.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiAccessPointRequest {
    pub credentials: NetworkCredentials,
    pub channel: u8,
    /// IP configuration owned by the HIL application while the AP role is
    /// active. This is deliberately outside the radio-driver request.
    pub ipv4: NetworkIpv4Configuration,
}

impl WifiAccessPointRequest {
    pub fn validate(&self) -> Result<(), WifiAccessPointRequestError> {
        self.credentials
            .validate()
            .map_err(WifiAccessPointRequestError::Credentials)?;
        if !(1..=13).contains(&self.channel) {
            return Err(WifiAccessPointRequestError::Channel);
        }
        if !matches!(
            self.ipv4,
            NetworkIpv4Configuration::Static { gateway: None, .. }
        ) || !self.ipv4.validate()
        {
            return Err(WifiAccessPointRequestError::Ipv4);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiAccessPointRequestError {
    Credentials(NetworkCredentialsError),
    Channel,
    Ipv4,
}

impl fmt::Display for WifiAccessPointRequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(error) => error.fmt(formatter),
            Self::Channel => formatter.write_str("AP channel must be in 1..=13"),
            Self::Ipv4 => formatter
                .write_str("AP mode requires a valid gateway-free static IPv4 configuration"),
        }
    }
}

/// Compact, target-neutral standalone scan request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiScanRequest {
    /// Bit zero selects channel 1 and bit twelve selects channel 13.
    pub channel_mask_2_4_ghz: u16,
    pub dwell_millis: u16,
}

/// Compact, target-neutral standalone monitor request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorRequest {
    pub channel: u8,
    /// Zero retains the complete frame; a nonzero value truncates captures.
    pub snapshot_length: u16,
}

/// Finite typed monitor export request. Duration is owned by the target so a
/// congested serial link cannot postpone the role stop indefinitely.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorCaptureRequest {
    pub channel: u8,
    pub snapshot_length: u16,
    pub duration_millis: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiMonitorEvidenceSource {
    Hardware,
    Protocol,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorObserved<T> {
    pub source: WifiMonitorEvidenceSource,
    pub value: T,
}

/// Typed transport form of the ESP32-S31 receive vector. The raw hardware
/// rate code remains explicitly scoped to its decoded PHY format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorPhyEvidence {
    pub format: WifiMonitorPhyFormat,
    pub hardware_rate_code: u8,
    pub he_siga1: u32,
    pub he_siga2: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WifiMonitorPhyFormat {
    Dot11b,
    Ofdm,
    Ht,
    Vht,
    HeSu,
    HeMu,
    HeExtendedRangeSu,
    HeTriggerBased,
    VhtMu,
    Unknown(u8),
}

/// One independently checksummed piece of one captured normalized MPDU.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorFrameChunk {
    pub generation: u32,
    pub frame_sequence: u32,
    pub dequeued_at_micros: u64,
    pub captured_length: u16,
    pub logical_length: u16,
    pub offset: u16,
    pub channel: Option<WifiMonitorObserved<u8>>,
    pub rssi_dbm: Option<WifiMonitorObserved<i8>>,
    pub rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
    bytes: heapless::Vec<u8, WIFI_MONITOR_FRAME_CHUNK_MAX_LEN>,
}

impl WifiMonitorFrameChunk {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        generation: u32,
        frame_sequence: u32,
        dequeued_at_micros: u64,
        captured_length: u16,
        logical_length: u16,
        offset: u16,
        channel: Option<WifiMonitorObserved<u8>>,
        rssi_dbm: Option<WifiMonitorObserved<i8>>,
        rate: Option<WifiMonitorObserved<WifiMonitorPhyEvidence>>,
        bytes: &[u8],
    ) -> Result<Self, WifiMonitorFrameChunkError> {
        if captured_length == 0 || bytes.is_empty() {
            return Err(WifiMonitorFrameChunkError::Empty);
        }
        let end = usize::from(offset)
            .checked_add(bytes.len())
            .ok_or(WifiMonitorFrameChunkError::Range)?;
        if end > usize::from(captured_length) {
            return Err(WifiMonitorFrameChunkError::Range);
        }
        let mut body = heapless::Vec::new();
        body.extend_from_slice(bytes)
            .map_err(|_| WifiMonitorFrameChunkError::TooLarge)?;
        Ok(Self {
            generation,
            frame_sequence,
            dequeued_at_micros,
            captured_length,
            logical_length,
            offset,
            channel,
            rssi_dbm,
            rate,
            bytes: body,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.captured_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiMonitorFrameChunkError {
    Empty,
    TooLarge,
    Range,
}

/// Completion of one explicit role ownership transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiRoleTransitionEvidence {
    pub previous: WifiRole,
    pub current: WifiRole,
    pub generation: u32,
}

/// Bounded scan evidence. The complete BSS table stays in the driver API and
/// is intentionally not expanded to fit the UART protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiScanEvidence {
    pub generation: u32,
    pub observed_frames: u32,
    pub unique_bss: u8,
    pub dropped_unique_bss: u32,
    pub configured_ssid_found: bool,
    pub configured_ssid_channel: u8,
    pub configured_ssid_rssi_dbm: i8,
}

/// Observations retained by the HIL consumer during one monitor epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiMonitorEvidence {
    pub generation: u32,
    pub channel: u8,
    pub captured_frames: u32,
    pub captured_bytes: u64,
    pub generation_mismatches: u32,
    /// Frames whose RX metadata explicitly named another channel.
    pub channel_mismatches: u32,
    /// Frames for which the backend did not expose per-frame channel data.
    pub channel_unavailable: u32,
    /// Most recent explicit hardware/protocol channel, or zero if unavailable.
    pub last_observed_channel: u8,
    /// Capture-pool publication counters for this exact generation.
    pub published_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub discarded_frames: u32,
    /// Frames whose complete ordered chunk set was admitted to the protocol
    /// queue before this terminal evidence.
    pub exported_frames: u32,
}

/// Bounded evidence retained for one access-point ownership epoch.
///
/// These counters describe MAC/runtime work only. IP services and host-side
/// client observations belong to the qualification report, not the driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WifiAccessPointEvidence {
    pub generation: u32,
    pub channel: u8,
    pub beacons_transmitted: u32,
    pub missed_beacon_intervals: u32,
    pub maximum_beacon_lateness_micros: u32,
    pub authentication_responses: u32,
    pub association_responses: u32,
    pub authorized_peers: u32,
    pub peer_removals: u32,
    pub completed_rx_descriptors: u32,
    pub ignored_rx_frames: u32,
    pub rx_mic_failures: u32,
    pub rx_quarantined_frames: u32,
    pub rx_view_rejected: u32,
    pub control_frames_staged: u32,
    pub control_frames_dropped_while_busy: u32,
    pub ethernet_frames_staged: u32,
    pub ethernet_arp_requests_staged: u32,
    pub ethernet_tcp_frames_staged: u32,
    pub network_tx_frames_observed: u32,
    pub network_tx_arp_requests: u32,
    pub network_tx_arp_replies: u32,
    pub network_tx_rejected_no_peer: u32,
    pub network_tx_rejected_destination: u32,
    pub network_tx_frames_rejected: u32,
    pub data_frames_transmitted: u32,
    pub tx_hardware_failures: u8,
    pub tx_hardware_timeouts: u8,
    pub tx_collision_limits: u8,
    pub tx_last_hardware_status: u8,
    pub protected_data_frames: u32,
    pub protected_data_unauthorized: u32,
    pub protected_data_foreign: u32,
    pub protected_data_duplicates: u32,
    pub protected_data_radio_rejected: u32,
    pub protected_data_protocol_rejected: u32,
}

impl StationEpochEvidence {
    pub const COMPLETE: Self = Self {
        runner_stopped: true,
        scan_owners_returned: true,
        join_completed: true,
        connected_runner_started: true,
    };

    pub const fn is_complete(self) -> bool {
        self.runner_stopped
            && self.scan_owners_returned
            && self.join_completed
            && self.connected_runner_started
    }
}

/// Why a connected station generation returned to candidate selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationDisconnectReason {
    /// The connected beacon monitor proved that the selected AP disappeared.
    BeaconLoss,
    /// The AP sent an IEEE 802.11 deauthentication frame.
    PeerDeauthentication { reason_code: u16 },
    /// The AP sent an IEEE 802.11 disassociation frame.
    PeerDisassociation { reason_code: u16 },
    /// Restoring active power-management state failed at the peer-visible TX edge.
    ActiveStateRestoreFailed,
    /// Connected-state WPA2 Group Key Handshake failed closed.
    GroupKeyHandshakeFailed,
    /// Another connected link policy returned the peer owner without claiming
    /// a beacon deadline; this must not qualify an AP-loss test.
    LinkPolicy,
    /// The host/application requested a healthy connected-epoch cycle.
    ReconnectRequested,
}

/// Stable station stage vocabulary used by HIL lifecycle evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationFailureStage {
    CandidateSelection,
    Authentication,
    Association,
    Security,
    Connected,
    Hardware,
}

/// Target-independent classification of a failed station attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationAttemptFailureReason {
    /// A complete candidate scan did not find the configured network.
    NoCandidate,
    /// A finite peer protocol exchange failed or timed out.
    PeerProtocol,
    /// Hardware ownership or a bounded hardware transaction failed.
    Hardware,
    /// The adapter observed an impossible production ownership contract.
    ContractViolation,
}

/// Reliable target-observed station lifecycle edge.
///
/// Generation zero is the initial connection. The outer lifecycle increments
/// the generation only after a connected epoch returns its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationLifecycleEvent {
    Connected {
        generation: u32,
    },
    Disconnected {
        generation: u32,
        reason: StationDisconnectReason,
    },
    /// One complete attempt returned every owner and was classified for retry.
    AttemptFailed {
        generation: u32,
        attempt: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
    /// The bounded reconnect policy returned the final owner without another
    /// hidden attempt or backoff.
    RetryExhausted {
        generation: u32,
        attempts: u16,
        stage: StationFailureStage,
        reason: StationAttemptFailureReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub address: [u8; 4],
    pub prefix_length: u8,
    pub gateway: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
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
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_units: u64,
    pub tx_units: u64,
    pub elapsed_micros: u64,
    pub transport_errors: u32,
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
    pub dma_buffer_full: u32,
    pub dma_fifo_overflow: u32,
    pub network_dropped: u32,
    pub irq_drain_saturated: u32,
    pub unknown_irq_status: u32,
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
    pub bandwidth_mhz: u16,
    pub aggregate_rate_kbps: u32,
    pub aggregates_prepared: u32,
    pub aggregate_publications: u32,
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
    pub preparation_micros: u32,
    pub publication_micros: u32,
    pub exchange_micros: u32,
    pub block_ack_samples: u32,
    pub block_ack_received: u32,
    pub success_without_block_ack: u32,
    pub nonzero_block_ack_control: u32,
    pub full_block_ack: u32,
    pub partial_block_ack: u32,
    pub empty_block_ack: u32,
    pub tx_irq_epochs: u32,
    pub tx_irq_service_samples: u32,
    pub tx_irq_clock_skew_samples: u32,
    pub tx_publication_to_irq_samples: u32,
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
    pub backward_mac_backward: u32,
    pub backward_mac_same: u32,
    pub backward_mac_forward: u32,
    pub backward_mac_other_tid: u32,
    pub backward_mac_unavailable: u32,
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StackUsage {
    pub minimum_free_percent: u8,
    pub cpu0: StackWatermark,
    pub cpu1: StackWatermark,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EvidenceRecord {
    Transport(TransportEvidence),
    Radio(RadioEvidence),
    RxDelivery(RxDeliveryEvidence),
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
    Hello(Capabilities),
    /// The radio and shared Wi-Fi owner are ready in the role-neutral idle
    /// state. The request ID correlates this edge with [`Command::Initialize`].
    Initialized,
    /// Correlated response to [`Command::QueryStackUsage`].
    StackUsage(StackUsage),
    Accepted,
    Rejected(RejectReason),
    State(StateChange),
    /// Correlated response to [`Command::GetStatus`].
    OperationStatus(OperationStatus),
    /// Reliable completion acknowledgement for `CycleStationEpoch`.
    /// The envelope request ID identifies the command being completed.
    StationEpochCompleted(StationEpochEvidence),
    /// Reliable completion of `StartStation` or `StopStation`.
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
    Evidence(EvidenceRecord),
    Finished(Finished),
    Failed(FailureCode),
    StartupArtifactReady(StartupArtifactStatus),
    StartupArtifact(StartupArtifactChunk),
}

impl WireBody for Event {
    const WIRE_KIND: WireKind = WireKind::Event;
}
