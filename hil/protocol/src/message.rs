use core::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 16;
pub const STARTUP_ARTIFACT_CHUNK_MAX_LEN: usize = 384;
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
    pub network_provisioning: bool,
    pub runtime_configuration: bool,
    pub structured_evidence: bool,
    /// This image accepts one opaque, host-owned startup artifact and can
    /// return its current value after initialization.
    pub startup_artifact: bool,
    /// This image can stop one healthy connected STA epoch at a safe runner
    /// boundary and use the returned owners to exercise reassociation.
    pub station_epoch_control: bool,
    /// This image can stop the complete station role and report reconstruction
    /// of the role-neutral Wi-Fi owner.
    pub station_stop_control: bool,
    /// This image reliably reports connected generations and proved peer-loss
    /// transitions independently of lossy text diagnostics.
    pub station_lifecycle_events: bool,
    /// This image can inject one fault below the station lifecycle, after a
    /// real LMAC transaction has acquired hardware ownership, and report the
    /// exact terminal owner frontier.
    pub station_fault_injection: bool,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetworkConfiguration {
    pub credentials: NetworkCredentials,
    pub ipv4: NetworkIpv4Configuration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkConfigurationError {
    Credentials(NetworkCredentialsError),
    Ipv4Configuration,
}

impl fmt::Display for NetworkConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(error) => error.fmt(formatter),
            Self::Ipv4Configuration => formatter.write_str("invalid IPv4 configuration"),
        }
    }
}

impl NetworkConfiguration {
    pub fn validate(&self) -> Result<(), NetworkConfigurationError> {
        self.credentials
            .validate()
            .map_err(NetworkConfigurationError::Credentials)?;
        if !self.ipv4.validate() {
            return Err(NetworkConfigurationError::Ipv4Configuration);
        }
        Ok(())
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
    UploadStartupArtifact(StartupArtifactChunk),
    ProvisionNetwork(NetworkConfiguration),
    Configure(SessionConfig),
    Arm,
    Start,
    /// Request one connected STA teardown/reassociation cycle. This is a HIL
    /// lifecycle operation, not a transport-session stop and not evidence of
    /// peer link loss.
    CycleStationEpoch,
    /// Stop the complete station role after all child tasks, DMA and interrupt
    /// routing have returned their exact owners.
    StopStation,
    /// Arm one deterministic fault below the station lifecycle facade.
    /// Injection is one-shot and occurs only after the named production
    /// transaction has crossed its hardware-ownership edge.
    InjectStationFault(StationFaultInjection),
    Stop,
    Abort,
    GetLastResult,
    AcknowledgeResult,
}

/// Production transaction edge selected by a HIL fault cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationFaultInjection {
    /// After a connected network TX has published a real descriptor, feed its
    /// service path a contradictory completion/timeout edge. The ordinary or
    /// aggregate owner must quarantine the descriptor for platform reset.
    ConnectedTxAfterPublication,
    /// After the RX owner has taken a real completed DMA unit but before it
    /// allocates/copies a staging lease, narrow admission so that unit follows
    /// the production over-capacity discard/reload path.
    ConnectedRxBeforeStagingOverCapacity,
}

/// Target-independent classification of a completed fault injection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationFaultClassification {
    /// The LMAC transaction rejected continued use of the radio and marked
    /// its descriptor owner reset-required.
    RadioResetRequired,
    /// The completed frame was deliberately discarded, every descriptor was
    /// reloaded, and the same live ring staged a following unit.
    RecoverableFrameDiscard,
    /// The requested injection returned through a different frontier and must
    /// fail qualification rather than being mislabeled as either recovery or
    /// reset-required quarantine.
    ContractViolation,
}

/// Reliable evidence for one station fault cell.
///
/// Terminal TX quarantine and recoverable RX discard deliberately use
/// different variants. This prevents a dropped frame from acquiring reset
/// semantics, or an ambiguous hardware owner from being mislabeled as a
/// recoverable data-plane loss.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StationFaultEvidence {
    ConnectedTxResetRequired {
        classification: StationFaultClassification,
        runner_returned: bool,
        executor_tasks_stopped: bool,
        rx_dma_stopped: bool,
        tx_owner_reset_required: bool,
    },
    ConnectedRxOverCapacityRecovered {
        classification: StationFaultClassification,
        descriptor_reloaded: bool,
        following_unit_staged: bool,
        same_ring_live: bool,
        service_result_ok: bool,
    },
}

impl StationFaultEvidence {
    pub const fn injection(self) -> StationFaultInjection {
        match self {
            Self::ConnectedTxResetRequired { .. } => {
                StationFaultInjection::ConnectedTxAfterPublication
            }
            Self::ConnectedRxOverCapacityRecovered { .. } => {
                StationFaultInjection::ConnectedRxBeforeStagingOverCapacity
            }
        }
    }

    pub const fn classification(self) -> StationFaultClassification {
        match self {
            Self::ConnectedTxResetRequired { classification, .. }
            | Self::ConnectedRxOverCapacityRecovered { classification, .. } => classification,
        }
    }

    pub const fn is_complete(self) -> bool {
        match self {
            Self::ConnectedTxResetRequired {
                classification,
                runner_returned,
                executor_tasks_stopped,
                rx_dma_stopped,
                tx_owner_reset_required,
            } => {
                matches!(
                    classification,
                    StationFaultClassification::RadioResetRequired
                ) && runner_returned
                    && executor_tasks_stopped
                    && rx_dma_stopped
                    && tx_owner_reset_required
            }
            Self::ConnectedRxOverCapacityRecovered {
                classification,
                descriptor_reloaded,
                following_unit_staged,
                same_ring_live,
                service_result_ok,
            } => {
                matches!(
                    classification,
                    StationFaultClassification::RecoverableFrameDiscard
                ) && descriptor_reloaded
                    && following_unit_staged
                    && same_ring_live
                    && service_result_ok
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionState {
    Booting,
    WaitingForNetwork,
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

/// Exact clean-stop frontiers required before another Wi-Fi role may start.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StationStopEvidence {
    pub lifecycle_owner_returned: bool,
    pub pac_reclaimed: bool,
    pub interrupt_setup_reclaimed: bool,
    /// DMA, network, executor and control owners were recovered from the
    /// exact stopped station phase rather than replaced with allocation
    /// handles or newly initialized storage.
    pub role_resources_reclaimed: bool,
    pub wifi_stopped_reconstructed: bool,
    /// The reconstructed owner was consumed by another supported Wi-Fi role.
    pub subsequent_role_materialized: bool,
    /// The subsequent role returned only after its own ISR/DMA epoch was
    /// quiescent and reconstructed the role-neutral owner again.
    pub subsequent_role_quiesced: bool,
    /// The role-local resources returned by the subsequent role were rebound
    /// to a second epoch without replacing their static storage or control
    /// domain.
    pub subsequent_role_restarted: bool,
    /// The restarted subsequent role returned its ISR/DMA and role-neutral
    /// owners through another clean quiescence edge.
    pub subsequent_role_restart_quiesced: bool,
    /// The owner returned by the subsequent role was consumed by a fresh
    /// station task without reacquiring PAC or recreating an IRQ token.
    pub station_rematerialized: bool,
    /// The rematerialized station completed scan/join/security and started a
    /// real connected runner before its stop was requested.
    pub station_connected: bool,
    /// The replacement station returned only after its task, ISR and DMA
    /// epochs were quiescent and its exact owner graph was reclaimed again.
    pub station_requiesced: bool,
}

impl StationStopEvidence {
    pub const COMPLETE: Self = Self {
        lifecycle_owner_returned: true,
        pac_reclaimed: true,
        interrupt_setup_reclaimed: true,
        role_resources_reclaimed: true,
        wifi_stopped_reconstructed: true,
        subsequent_role_materialized: true,
        subsequent_role_quiesced: true,
        subsequent_role_restarted: true,
        subsequent_role_restart_quiesced: true,
        station_rematerialized: true,
        station_connected: true,
        station_requiesced: true,
    };

    pub const fn is_complete(self) -> bool {
        self.lifecycle_owner_returned
            && self.pac_reclaimed
            && self.interrupt_setup_reclaimed
            && self.role_resources_reclaimed
            && self.wifi_stopped_reconstructed
            && self.subsequent_role_materialized
            && self.subsequent_role_quiesced
            && self.subsequent_role_restarted
            && self.subsequent_role_restart_quiesced
            && self.station_rematerialized
            && self.station_connected
            && self.station_requiesced
    }
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
pub enum EvidenceRecord {
    Transport(TransportEvidence),
    Link(LinkHealth),
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
    Accepted,
    Rejected(RejectReason),
    State(StateChange),
    /// Reliable completion acknowledgement for `CycleStationEpoch`.
    /// The envelope request ID identifies the command being completed.
    StationEpochCompleted(StationEpochEvidence),
    /// Reliable completion acknowledgement for `StopStation`.
    StationStopped(StationStopEvidence),
    /// Unsolicited, reliable station generation/link transition.
    StationLifecycle(StationLifecycleEvent),
    /// Reliable terminal frontier for a requested station fault injection.
    /// The envelope request ID matches `InjectStationFault`.
    StationFault(StationFaultEvidence),
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
