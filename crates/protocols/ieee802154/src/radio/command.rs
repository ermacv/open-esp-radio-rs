//! Caller-owned command values; frame bytes are borrowed only for admission.

use super::{RadioTimestamp, RequestId, channel::Channel};
use crate::mac::frame::FrameView;

/// Portable Host-to-radio operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioCommand<'frame> {
    /// Acquire the radio and enter sleep.
    Enable {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Release an enabled, non-busy radio.
    Disable {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Leave receive mode and enter sleep.
    Sleep {
        /// Caller-owned correlation identifier.
        id: RequestId,
    },
    /// Enter receive mode on one channel.
    Receive {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Requested receive channel.
        channel: Channel,
    },
    /// Apply one portable address/filter/power setting.
    Configure {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Complete setting update.
        configuration: Configuration,
    },
    /// Transfer a borrowed frame to the backend for the duration of command
    /// admission. A hardware adapter must copy or otherwise retain the bytes
    /// under its own explicit ownership before returning from admission.
    Transmit(TxRequest<'frame>),
    /// Perform one bounded energy scan.
    EnergyScan(EnergyScanRequest),
    /// Perform one standalone clear-channel assessment.
    ClearChannelAssessment {
        /// Caller-owned correlation identifier.
        id: RequestId,
        /// Channel to assess.
        channel: Channel,
    },
}

impl RadioCommand<'_> {
    /// Return the caller-owned correlation identifier.
    pub const fn id(self) -> RequestId {
        match self {
            Self::Enable { id }
            | Self::Disable { id }
            | Self::Sleep { id }
            | Self::Receive { id, .. }
            | Self::Configure { id, .. }
            | Self::ClearChannelAssessment { id, .. } => id,
            Self::Transmit(request) => request.id,
            Self::EnergyScan(request) => request.id,
        }
    }

    /// Return the finite operation kind without retaining frame bytes.
    pub const fn kind(self) -> CommandKind {
        match self {
            Self::Enable { .. } => CommandKind::Enable,
            Self::Disable { .. } => CommandKind::Disable,
            Self::Sleep { .. } => CommandKind::Sleep,
            Self::Receive { .. } => CommandKind::Receive,
            Self::Configure { .. } => CommandKind::Configure,
            Self::Transmit(_) => CommandKind::Transmit,
            Self::EnergyScan(_) => CommandKind::EnergyScan,
            Self::ClearChannelAssessment { .. } => CommandKind::ClearChannelAssessment,
        }
    }
}

/// Frame-free command discriminator suitable for bounded mailboxes and logs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandKind {
    /// Enable operation.
    Enable,
    /// Disable operation.
    Disable,
    /// Sleep operation.
    Sleep,
    /// Receive operation.
    Receive,
    /// Configuration operation.
    Configure,
    /// Transmit operation.
    Transmit,
    /// Energy-scan operation.
    EnergyScan,
    /// Standalone CCA operation.
    ClearChannelAssessment,
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
    /// Optional requested power in dBm; `None` retains backend configuration.
    pub transmit_power_dbm: Option<i8>,
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
