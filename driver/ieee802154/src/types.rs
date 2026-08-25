use crate::{Channel, FrameView};

/// Caller-assigned identifier used to correlate an operation and completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RequestId(u32);

impl RequestId {
    /// Preserve one caller-owned identifier.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the caller-owned identifier image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Microseconds in a backend-defined monotonic radio epoch.
///
/// The epoch is deliberately not wall-clock time. An adapter must use one
/// stable epoch for every timestamp it publishes in a controller instance.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RadioTimestamp(u64);

impl RadioTimestamp {
    /// Construct a timestamp from monotonic microseconds.
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// Return monotonic microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0
    }
}

/// How a backend validated the received frame check sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FcsStatus {
    /// Hardware or software reported a valid FCS.
    Valid,
    /// Hardware or software reported an invalid FCS.
    Invalid,
    /// The adapter cannot report FCS validation for this frame.
    Unavailable,
}

/// Security processing already performed before frame publication.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SecurityStatus {
    /// No security offload was applied.
    Unprocessed,
    /// Authentication/decryption completed successfully.
    Processed,
    /// Authentication/decryption failed; promiscuous policy retained the
    /// frame for diagnostics.
    Failed,
}

/// Tri-state frame-pending observation retained from an acknowledgement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FramePending {
    /// The acknowledgement carried a clear frame-pending bit.
    Clear,
    /// The acknowledgement carried a set frame-pending bit.
    Set,
    /// The backend did not expose this observation.
    Unavailable,
}

/// Backend-neutral receive metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RxMetadata {
    /// Channel on which the complete frame was received.
    pub channel: Channel,
    /// Signed RSSI normalized to dBm.
    pub rssi_dbm: i8,
    /// Link quality in the portable zero-through-255 domain.
    pub link_quality: u8,
    /// Optional start-of-frame timestamp in the controller monotonic epoch.
    pub timestamp: Option<RadioTimestamp>,
    /// FCS validation result.
    pub fcs: FcsStatus,
    /// Security processing already applied to the bytes.
    pub security: SecurityStatus,
    /// Frame-pending observation when the frame is an acknowledgement.
    pub frame_pending: FramePending,
}

/// One borrowed received MAC frame and its normalized metadata.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReceivedFrame<'frame> {
    /// MAC bytes without PHR, FCS storage or DMA metadata.
    pub frame: FrameView<'frame>,
    /// Backend-neutral receive observations.
    pub metadata: RxMetadata,
}

/// How one transmit request should acquire the channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxMode {
    /// Start transmission without a preceding clear-channel assessment.
    Direct,
    /// Perform one clear-channel assessment before transmission.
    ClearChannelAssessment,
    /// Apply bounded CSMA-CA with the supplied maximum backoff count.
    CsmaCa {
        /// Maximum number of backoffs before returning channel-busy.
        max_backoffs: u8,
    },
    /// Start at a monotonic backend timestamp without implicit CSMA-CA.
    Scheduled {
        /// Requested start time.
        at: RadioTimestamp,
    },
}

/// Whether transmission completion requires an acknowledgement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AcknowledgementPolicy {
    /// `TX_DONE` is sufficient for success.
    NotRequested,
    /// Success requires an acknowledgement within backend policy bounds.
    Required,
}

/// Borrowed portable transmission request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxRequest<'frame> {
    /// Caller-owned correlation identifier.
    pub id: RequestId,
    /// MAC bytes without platform framing.
    pub frame: FrameView<'frame>,
    /// Channel on which the frame must be transmitted.
    pub channel: Channel,
    /// Channel access mode.
    pub mode: TxMode,
    /// Acknowledgement requirement.
    pub acknowledgement: AcknowledgementPolicy,
    /// Optional requested power in dBm; `None` retains backend configuration.
    pub transmit_power_dbm: Option<i8>,
}

/// Terminal result of one accepted transmit request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxStatus {
    /// The on-air operation completed under the requested acknowledgement
    /// policy.
    Success,
    /// Channel access did not find an idle window.
    ChannelBusy,
    /// An acknowledgement was required but not received.
    NoAcknowledgement,
    /// A later command or platform shutdown cancelled the request.
    Aborted,
    /// The backend rejected bytes after accepting the portable request.
    InvalidFrame,
    /// Hardware could not complete the operation.
    HardwareFailure,
}

/// One bounded energy-detection scan request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnergyScanRequest {
    /// Caller-owned correlation identifier.
    pub id: RequestId,
    /// Channel to measure.
    pub channel: Channel,
    /// Scan duration in microseconds.
    pub duration_us: u32,
}

/// One portable radio configuration update.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Configuration {
    /// Set the local PAN identifier in host byte order.
    PanId(u16),
    /// Set the local short address in host byte order.
    ShortAddress(u16),
    /// Set the local extended address in canonical over-the-air byte order.
    ExtendedAddress([u8; 8]),
    /// Enable or disable promiscuous receive publication.
    Promiscuous(bool),
    /// Enable or disable automatic acknowledgement generation.
    AutomaticAcknowledgement(bool),
    /// Set the default transmit power in dBm.
    TransmitPowerDbm(i8),
}

/// Portable fail-closed controller fault category.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioFault {
    /// Hardware or its clock/reset owner became unavailable.
    HardwareUnavailable,
    /// An interrupt/event sequence violated backend invariants.
    InvalidEventSequence,
    /// Backend-owned packet storage was exhausted.
    StorageExhausted,
    /// Backend state can no longer be reconciled safely.
    StateLost,
}
