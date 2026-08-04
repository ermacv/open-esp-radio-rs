use core::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const PROTOCOL_VERSION: u16 = 4;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub transport: Transport,
    pub direction: Direction,
    pub completion: Completion,
    pub peer: Option<Ipv4Endpoint>,
    pub target_rx: Option<FlowConfig>,
    pub target_tx: Option<FlowConfig>,
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
    ProvisionNetwork(NetworkCredentials),
    Configure(SessionConfig),
    Arm,
    Start,
    /// Request one connected STA teardown/reassociation cycle. This is a HIL
    /// lifecycle operation, not a transport-session stop and not evidence of
    /// peer link loss.
    CycleStationEpoch,
    Stop,
    Abort,
    GetLastResult,
    AcknowledgeResult,
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
    NetworkReady(NetworkInfo),
    ServiceReady(ServiceInfo),
    Evidence(EvidenceRecord),
    Finished(Finished),
    Failed(FailureCode),
    StartupArtifactReady(StartupArtifactStatus),
    StartupArtifact(StartupArtifactChunk),
}
