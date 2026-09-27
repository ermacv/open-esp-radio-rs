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
    /// Sleep, then receive in a window that opens at a monotonic radio time.
    ScheduledReceive(ScheduledReceiveRequest),
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
            Self::ScheduledReceive(request) => request.id,
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
            Self::ScheduledReceive(_) => CommandKind::ScheduledReceive,
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
    /// Scheduled receive window.
    ScheduledReceive,
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
    /// Start at a monotonic backend timestamp without implicit CSMA-CA,
    /// optionally after one clear-channel assessment that ends at that time.
    Scheduled {
        /// Requested start time.
        at: RadioTimestamp,
        /// Assess the channel first (ESP-IDF `esp_ieee802154_transmit_at`
        /// with `cca`, as OpenThread's `mCsmaCaEnabled` asks).
        cca: bool,
    },
}

/// Who secures a frame whose security-enabled bit is set, as the transmit
/// information of OpenThread's `otRadioFrame` (`mIsSecurityProcessed`,
/// `mIsARetx`) tells a radio that claims `OT_RADIO_CAPS_TRANSMIT_SEC`.
///
/// A radio without MAC keys transmits every frame as given.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TxSecurity {
    /// The radio secures the frame with its MAC keys and writes a new frame
    /// counter and, in key identifier mode 1, its key index.
    #[default]
    Radio,
    /// The frame retransmits one the radio secured: the radio secures it
    /// again under the frame counter and key index it already carries.
    Retransmission,
    /// The upper layer already secured the frame; the radio transmits it
    /// as given.
    Processed,
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
    /// Retransmissions after an attempt without acknowledgement or channel
    /// access (`macMaxFrameRetries`); zero sends the frame once.
    pub max_frame_retries: u8,
    /// Who secures the frame when its security-enabled bit is set.
    pub security: TxSecurity,
}

/// One scheduled receive window (`otPlatRadioReceiveAt`).
///
/// The radio sleeps until the window opens, receives on `channel` from
/// `start` and, for a nonzero `duration_us`, stops at its end - finishing a
/// frame whose reception has begun - and sleeps again. A window of zero
/// duration receives until the next command. A backend may end the window
/// with its first received frame, as ESP-IDF's driver does; a window that
/// already ended when admitted ends at once. Either end is reported by
/// [`RadioEvent::ScheduledReceiveDone`](crate::RadioEvent::ScheduledReceiveDone).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScheduledReceiveRequest {
    /// Caller-owned correlation identifier.
    pub id: RequestId,
    /// Receive channel.
    pub channel: Channel,
    /// Time the receiver is on, in the radio's monotonic epoch.
    pub start: RadioTimestamp,
    /// Window length in microseconds; zero leaves the window open.
    pub duration_us: u32,
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
    /// Set the transmit power of one channel in dBm
    /// (`esp_ieee802154_set_power_with_channel`).
    ChannelTransmitPowerDbm {
        /// The channel whose power changes.
        channel: Channel,
        /// Transmit power in dBm.
        power_dbm: i8,
    },
    /// Set the energy threshold of clear-channel assessment in dBm
    /// (`esp_ieee802154_set_cca_threshold`,
    /// `otPlatRadioSetCcaEnergyDetectThreshold`).
    CcaThresholdDbm(i8),
    /// Set how clear-channel assessment decides that the channel is busy
    /// (`esp_ieee802154_set_cca_mode`).
    CcaMode(CcaMode),
    /// Act as the PAN coordinator: accept data and command frames without a
    /// destination address (`esp_ieee802154_set_coordinator`).
    PanCoordinator(bool),
}

/// How clear-channel assessment decides that the channel is busy
/// (IEEE 802.15.4 CCA modes, `esp_ieee802154_cca_mode_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CcaMode {
    /// Busy when a carrier is detected (mode 2).
    Carrier,
    /// Busy when the energy is above the threshold (mode 1).
    EnergyDetection,
    /// Busy when a carrier is detected or the energy is above the threshold.
    CarrierOrEnergyDetection,
    /// Busy when a carrier is detected and the energy is above the threshold
    /// (mode 3).
    CarrierAndEnergyDetection,
}
