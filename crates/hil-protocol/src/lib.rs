#![no_std]
//! Target-neutral control and evidence protocol for hardware-in-the-loop tests.
//!
//! The wire types deliberately contain no board paths, expected image hashes,
//! vendor ABI versions, or target-specific register layouts. Those belong to
//! the firmware adapter and the qualification manifest that selects it.

mod framing;
mod message;

pub use framing::{
    DecodeCounters, DecodeError, EncodeError, FrameDecoder, FrameEncoder, MAX_POSTCARD_BYTES,
    MAX_WIRE_FRAME_BYTES, evidence_crc32c, startup_artifact_crc32c,
};
pub use message::{
    Capabilities, Command, Completion, Direction, Envelope, Event, EvidenceRecord, FailureCode,
    FeatureCapabilities, Finished, FlowConfig, Ipv4Endpoint, LinkHealth, NetworkCredentials,
    NetworkCredentialsError, NetworkInfo, PROTOCOL_VERSION, RejectReason, ResultSummary,
    STARTUP_ARTIFACT_CHUNK_MAX_LEN, ServiceInfo, SessionConfig, SessionState, StartupArtifactChunk,
    StartupArtifactChunkError, StartupArtifactDisposition, StartupArtifactStatus, StateChange,
    Transport, TransportEvidence, WPA2_PASSPHRASE_MAX_LEN, WPA2_PASSPHRASE_MIN_LEN,
    WPA2_SSID_MAX_LEN,
};
