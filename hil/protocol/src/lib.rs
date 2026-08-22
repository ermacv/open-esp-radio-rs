#![no_std]
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter and the qualification manifest that selects it.

mod framing;
mod message;
mod stream_pattern;

pub use framing::{
    DecodeCounters, DecodeError, EncodeError, FrameDecoder, FrameEncoder, MAX_POSTCARD_BYTES,
    MAX_WIRE_FRAME_BYTES, evidence_crc32c, startup_artifact_crc32c,
};
pub use message::{
    Capabilities, Command, Completion, Direction, Envelope, Event, EvidenceRecord, FailureCode,
    FeatureCapabilities, Finished, FlowConfig, InitializationConfiguration, Ipv4Endpoint,
    LinkHealth, NetworkCredentials, NetworkCredentialsError, NetworkInfo, NetworkIpv4Configuration,
    NetworkSchedulerEvidence, OperationStatus, PROTOCOL_VERSION, RadioEvidence, RejectReason,
    ResultSummary, RxConsumerLedgerEvidence, RxDeliveryEvidence, RxMacOrderEvidence,
    RxRadioEvidence, RxReorderDeliveryEvidence, RxSequenceStageEvidence,
    STARTUP_ARTIFACT_CHUNK_MAX_LEN, ServiceInfo, SessionConfig, SessionLinkRequirements,
    SessionReady, SessionState, StackUsage, StackWatermark, StartupArtifactChunk,
    StartupArtifactChunkError, StartupArtifactDisposition, StartupArtifactStatus, StateChange,
    StationAttemptFailureReason, StationDisconnectReason, StationEpochEvidence,
    StationFailureStage, StationLifecycleEvent, TimebaseProbeEvidence, TimebaseProbeRequest,
    Transport, TransportEvidence, TxAggregateTimingEvidence, TxRadioEvidence,
    WIFI_MONITOR_FRAME_CHUNK_MAX_LEN, WPA2_PASSPHRASE_MAX_LEN, WPA2_PASSPHRASE_MIN_LEN,
    WPA2_SSID_MAX_LEN, WifiAccessPointEvidence, WifiAccessPointRequest,
    WifiAccessPointRequestError, WifiChannelWidth, WifiDataPlanePlacement,
    WifiMonitorCaptureRequest, WifiMonitorEvidence, WifiMonitorEvidenceSource,
    WifiMonitorFrameChunk, WifiMonitorFrameChunkError, WifiMonitorObserved, WifiMonitorPhyEvidence,
    WifiMonitorPhyFormat, WifiMonitorRequest, WifiNetworkInterface, WifiRole,
    WifiRoleFailureEvidence, WifiRoleFailureReason, WifiRoleOperation, WifiRoleTransitionEvidence,
    WifiScanEvidence, WifiScanRequest, WifiStationAccessPointRequest,
    WifiStationAccessPointStopEvidence, WireBody, WireKind,
};
pub use stream_pattern::{fill_stream_pattern, stream_pattern_byte, stream_pattern_matches};
