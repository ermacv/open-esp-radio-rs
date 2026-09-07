//! Backend-to-caller observations, completion categories and receive metadata.

use super::{RadioTimestamp, RequestId, channel::Channel};
use crate::mac::frame::FrameView;

/// Backend-to-Host observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioEvent<'frame> {
    /// One frame was received while the state machine owns receive mode.
    Received(ReceivedFrame<'frame>),
    /// Terminal transmit completion.
    TransmitDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Portable completion category.
        status: TxStatus,
        /// Optional received acknowledgement MAC bytes and metadata.
        acknowledgement: Option<ReceivedFrame<'frame>>,
    },
    /// Terminal energy scan completion.
    EnergyScanDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Maximum observed RSSI normalized to dBm.
        maximum_rssi_dbm: i8,
    },
    /// Terminal standalone CCA completion.
    ClearChannelAssessmentDone {
        /// Correlation identifier from the accepted request.
        id: RequestId,
        /// Whether the assessment found the channel idle.
        idle: bool,
    },
    /// Fail-closed backend fault. A valid fault disables the state machine.
    Fault {
        /// Active operation identifier, or `None` outside an operation.
        id: Option<RequestId>,
        /// Portable fault category.
        fault: RadioFault,
    },
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
