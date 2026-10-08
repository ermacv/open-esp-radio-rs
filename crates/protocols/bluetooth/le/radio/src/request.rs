//! Caller requests and long-lived role configuration.

use oer_radio_coex::CoexPriority;

use crate::{
    AdvertisingChannels, AdvertisingPdu, DataChannel, DataPdu, LeInstant, LePhy, LeWindow,
    RadioDuration, TestChannel, TestPayloadType, channel::AdvertisingChannel,
};

/// Caller-assigned identifier correlating one event with its outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EventId(u32);

impl oer_radio_port::Correlation for EventId {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn raw(self) -> u32 {
        self.0
    }
}

impl EventId {
    /// Preserve one caller-owned identifier.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The caller-owned identifier.
    pub const fn get(self) -> u32 {
        self.0
    }
}

macro_rules! role_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(u8);

        impl $name {
            /// Preserve one caller-chosen index.
            pub const fn new(index: u8) -> Self {
                Self(index)
            }

            /// The caller-chosen index.
            pub const fn index(self) -> u8 {
                self.0
            }
        }
    };
}

role_id!(
    /// One configured advertising set.
    AdvertisingSetId
);
role_id!(
    /// One configured scanner.
    ScannerId
);
role_id!(
    /// One open connection.
    ConnectionId
);

/// Requested transmit power in dBm; the backend rounds to what it supports.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxPower(i8);

impl TxPower {
    /// Request `dbm`.
    pub const fn from_dbm(dbm: i8) -> Self {
        Self(dbm)
    }

    /// The requested power.
    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Access Address octets in air order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AccessAddress(pub [u8; 4]);

/// CRC initialization octets in air order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CrcInit(pub [u8; 3]);

/// What an advertising set listens for after each of its PDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisingReception<'pdu> {
    /// The set only transmits.
    None,
    /// The set answers `SCAN_REQ` with this `SCAN_RSP` and reports every
    /// reception.
    ScanResponse(AdvertisingPdu<'pdu>),
    /// The set reports every reception without answering.
    Report,
}

/// Configuration of one legacy advertising set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingConfiguration<'pdu> {
    /// The set.
    pub set: AdvertisingSetId,
    /// The advertising PDU.
    pub pdu: AdvertisingPdu<'pdu>,
    /// What the set listens for.
    pub reception: AdvertisingReception<'pdu>,
    /// Transmit power.
    pub tx_power: TxPower,
    /// PHY of the advertising PDUs and their responses.
    pub phy: LePhy,
}

/// How urgently one event needs the shared antenna.
///
/// A radio that shares the antenna with other protocols raises the event's
/// coexistence priority with the level. The Controller core derives it from
/// the role's state: an advertising set periodically, a connection when it
/// is new, has missed receptions or runs a control procedure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub enum CoexistenceLevel {
    /// The role's ordinary priority.
    #[default]
    Baseline,
    /// Above the ordinary priority.
    Elevated,
    /// The highest priority of the role.
    Critical,
}

impl CoexistenceLevel {
    /// The portable priority of this level: `Baseline` is the ordinary
    /// [`CoexPriority::Normal`].
    pub const fn priority(self) -> CoexPriority {
        match self {
            Self::Baseline => CoexPriority::Normal,
            Self::Elevated => CoexPriority::Elevated,
            Self::Critical => CoexPriority::Critical,
        }
    }

    /// The level of a portable priority, or `None` for
    /// [`CoexPriority::Idle`]: an event always runs an operation.
    pub const fn from_priority(priority: CoexPriority) -> Option<Self> {
        match priority {
            CoexPriority::Idle => None,
            CoexPriority::Normal => Some(Self::Baseline),
            CoexPriority::Elevated => Some(Self::Elevated),
            CoexPriority::Critical => Some(Self::Critical),
        }
    }
}

impl From<CoexistenceLevel> for CoexPriority {
    fn from(level: CoexistenceLevel) -> Self {
        level.priority()
    }
}

/// [`CoexPriority::Idle`], which no Bluetooth LE event requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdlePriority;

impl TryFrom<CoexPriority> for CoexistenceLevel {
    type Error = IdlePriority;

    fn try_from(priority: CoexPriority) -> Result<Self, Self::Error> {
        Self::from_priority(priority).ok_or(IdlePriority)
    }
}

/// One advertising event of a configured set.
///
/// Channel `i` in index order has its anchor at `anchor + i *
/// channel_spacing`. The backend reserves each channel for one spacing,
/// starting its preparation lead before the channel's anchor, so a spacing
/// covers the lead, the packet and any response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingEvent {
    /// Correlation.
    pub id: EventId,
    /// The set.
    pub set: AdvertisingSetId,
    /// Air anchor of the first channel.
    pub anchor: LeInstant,
    /// Channels used in index order.
    pub channels: AdvertisingChannels,
    /// Time between the anchors of consecutive channels.
    pub channel_spacing: RadioDuration,
    /// Coexistence urgency of the event.
    pub coexistence: CoexistenceLevel,
}

impl AdvertisingEvent {
    /// The channel and its anchor at `position` in channel order. An absent
    /// position is `Ok(None)`; invalid spacing or anchor arithmetic is an error.
    pub fn channel_anchor(
        &self,
        position: usize,
    ) -> Result<Option<(AdvertisingChannel, LeInstant)>, TimingError> {
        let Some(channel) = self.channels.iter().nth(position) else {
            return Ok(None);
        };
        // A present position is at most two in the three-channel wire set.
        let offset = self
            .channel_spacing
            .checked_mul(position as u64)
            .ok_or(TimingError::DurationOverflow)?;
        let anchor = self
            .anchor
            .checked_add(offset)
            .ok_or(TimingError::BeyondEpoch)?;
        Ok(Some((channel, anchor)))
    }
}

/// Configuration of one passive scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScannerConfiguration {
    /// The scanner.
    pub scanner: ScannerId,
    /// Whether the scanner requests scan responses.
    pub scan_type: ScanType,
    /// Which advertisers the scanner receives from.
    pub filter_policy: ScanFilterPolicy,
    /// Transmit power retained by the scanner profile.
    pub tx_power: TxPower,
    /// PHY listened on.
    pub phy: LePhy,
}

/// Which advertisers a scanner receives from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanFilterPolicy {
    /// Every advertiser.
    AcceptAll,
    /// Only the devices of the filter accept list; the backend filters in
    /// hardware, so other advertisers are neither reported nor scanned.
    AcceptListOnly,
}

/// One device of the filter accept list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptListDevice {
    /// The address is random rather than public.
    pub random: bool,
    /// The address, least significant octet first.
    pub address: [u8; 6],
}

/// One change of the filter accept list. The Controller changes the list only
/// while no role filters against it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptListChange {
    /// Add a device; a device already listed stays listed.
    Add(AcceptListDevice),
    /// Remove a listed device.
    Remove(AcceptListDevice),
    /// Remove every device.
    Clear,
}

/// How a scanner treats scannable advertising.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanType {
    /// Only listen.
    Passive,
    /// Send `SCAN_REQ` to scannable advertisers and receive their `SCAN_RSP`.
    Active,
}

/// One scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanWindow {
    /// Correlation.
    pub id: EventId,
    /// The scanner.
    pub scanner: ScannerId,
    /// Channel listened on.
    pub channel: AdvertisingChannel,
    /// Air window of the listening.
    pub window: LeWindow,
}

/// Configuration of one peripheral connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionConfiguration {
    /// The connection.
    pub connection: ConnectionId,
    /// Access Address.
    pub access_address: AccessAddress,
    /// CRC initialization.
    pub crc_init: CrcInit,
    /// The on-air start of the connection indication; the first reference
    /// point until a valid reception replaces it.
    pub created_at: LeInstant,
    /// Transmit power.
    pub tx_power: TxPower,
    /// PHY of the connection in both directions.
    pub phy: LePhy,
}

/// How long the peripheral listens for the central in one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionEventTiming {
    /// The first event: the transmit window widened by the timing guard on
    /// both sides.
    First {
        /// Transmit window width.
        transmit_window: RadioDuration,
        /// Timing uncertainty on each side.
        timing_guard: RadioDuration,
    },
    /// A later event with its complete widened receive wait.
    Recurring {
        /// Total time to wait for the central.
        receive_wait: RadioDuration,
        /// Window widening on each side of the anchor: the clock drift since
        /// the last received anchor plus the backend's jitter.
        widening: RadioDuration,
    },
}

/// One connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionEvent {
    /// Correlation.
    pub id: EventId,
    /// The connection.
    pub connection: ConnectionId,
    /// Data channel of the event.
    pub channel: DataChannel,
    /// Air window: its start is the earliest anchor, its duration the air
    /// time the event reserves.
    pub window: LeWindow,
    /// Connection interval. A backend may let the event continue past its
    /// air window while it stays within the interval.
    pub interval: RadioDuration,
    /// Receive wait of the event.
    pub timing: ConnectionEventTiming,
    /// Scheduling priority, 0 to 15.
    pub priority: u8,
    /// Coexistence urgency of the event.
    pub coexistence: CoexistenceLevel,
}

/// One Direct Test Mode transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestTransmit<'payload> {
    /// Correlation.
    pub id: EventId,
    /// RF channel.
    pub channel: TestChannel,
    /// PHY.
    pub phy: TestPhy,
    /// Air window of the packet.
    pub window: LeWindow,
    /// Transmit power.
    pub tx_power: TxPower,
    /// LE Test packet payload type.
    pub payload_type: TestPayloadType,
    /// Payload.
    pub payload: &'payload [u8],
}

/// One Direct Test Mode receiver event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestReceive {
    /// Correlation.
    pub id: EventId,
    /// RF channel.
    pub channel: TestChannel,
    /// PHY.
    pub phy: TestPhy,
    /// Air window of the listening.
    pub window: LeWindow,
    /// Whether the receiver ran before in the same test.
    pub recurring: bool,
    /// Transmit power retained by the test profile.
    pub tx_power: TxPower,
}

/// PHY of a Direct Test Mode event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TestPhy {
    /// LE 1M.
    Le1M,
    /// LE 2M.
    Le2M,
    /// LE Coded with S=8 coding.
    LeCodedS8,
    /// LE Coded with S=2 coding; a receiver accepts either coding.
    LeCodedS2,
}

/// The backend's timing, which the planner uses to keep reservations apart.
///
/// A backend reserves `[anchor - preparation_lead, window end)` for every
/// event and refuses an event whose reservation starts less than
/// `admission_guard` after the current time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioTiming {
    /// Time the backend prepares the radio before an anchor.
    pub preparation_lead: RadioDuration,
    /// Minimum time from a request to the start of its reservation.
    pub admission_guard: RadioDuration,
    /// How the backend listens around connection anchors.
    pub connection: ConnectionAllowances,
}

/// The backend's allowances around a connection anchor, which the planner
/// adds to the Link Layer's own window widening.
///
/// For an anchor `A`, a widening `W` (the specification's clock drift plus
/// `widening_jitter`) and an outstanding transmit window `T`, a recurring
/// event listens for `receive_guard + 2W + T + receive_tail` from
/// `A - receive_guard - W` and occupies the air from
/// `A - receive_guard - W - boundary_guard` to `A + W + T + event_length`.
/// The first event listens across `T` widened by `first_event_guard` on each
/// side and occupies the air from `A - first_event_guard - boundary_guard`
/// to `A + T + first_event_length`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionAllowances {
    /// Worst-case accuracy of the local sleep clock, in parts per million.
    pub local_sleep_clock_ppm: u16,
    /// Timing jitter added to every window widening.
    pub widening_jitter: RadioDuration,
    /// Fixed listening before the widened anchor of a recurring event.
    pub receive_guard: RadioDuration,
    /// Listening kept after the widened receive window.
    pub receive_tail: RadioDuration,
    /// Guard before an event's air window.
    pub boundary_guard: RadioDuration,
    /// Uncertainty on each side of the first event's transmit window.
    pub first_event_guard: RadioDuration,
    /// Air time a recurring event keeps after its widened anchor window.
    pub event_length: RadioDuration,
    /// Air time the first event keeps after its transmit window.
    pub first_event_length: RadioDuration,
}

/// Why physical radio geometry cannot be represented in the current epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimingError {
    /// An endpoint precedes the beginning of the epoch.
    BeforeEpoch,
    /// An endpoint exceeds the end of the epoch.
    BeyondEpoch,
    /// A duration sum or product exceeds the portable duration range.
    DurationOverflow,
    /// An elapsed span was requested in reverse order.
    ReversedTime,
    /// The resulting window is empty or exceeds the epoch.
    Window(crate::WindowError),
}

impl RadioTiming {
    /// Add the preparation lead to a checked air window's reservation.
    pub fn reservation(&self, window: LeWindow) -> Result<LeWindow, TimingError> {
        let start = window
            .start()
            .checked_sub(self.preparation_lead)
            .ok_or(TimingError::BeforeEpoch)?;
        let duration = window
            .duration()
            .checked_add(self.preparation_lead)
            .ok_or(TimingError::DurationOverflow)?;
        LeWindow::new(start, duration).map_err(TimingError::Window)
    }
}

/// One request to the radio backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioRequest<'data> {
    /// Configure an advertising set; its events may follow.
    ConfigureAdvertising(AdvertisingConfiguration<'data>),
    /// Schedule one advertising event.
    Advertise(AdvertisingEvent),
    /// Release an advertising set without pending events.
    RemoveAdvertising(AdvertisingSetId),
    /// Configure a scanner.
    ConfigureScanner(ScannerConfiguration),
    /// Schedule one scan window.
    Scan(ScanWindow),
    /// Release a scanner without pending windows.
    RemoveScanner(ScannerId),
    /// Open a connection.
    OpenConnection(ConnectionConfiguration),
    /// Schedule one connection event.
    ConnectionEvent(ConnectionEvent),
    /// Queue one PDU on a connection.
    Transmit {
        /// The connection.
        connection: ConnectionId,
        /// The PDU.
        pdu: DataPdu<'data>,
    },
    /// Close a connection without pending events.
    CloseConnection(ConnectionId),
    /// Schedule one Direct Test Mode transmitter event.
    TestTransmit(TestTransmit<'data>),
    /// Schedule one Direct Test Mode receiver event.
    TestReceive(TestReceive),
    /// End Direct Test Mode after its last event ended.
    EndTest,
    /// Withdraw a scheduled event. Its outcome still follows.
    Cancel(EventId),
    /// Change the filter accept list.
    FilterAcceptList(AcceptListChange),
}

/// Why the backend refused a request. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    /// No instance of the role is free.
    NoInstance,
    /// The set, scanner or connection is not configured.
    Unknown,
    /// The set, scanner or connection is already configured.
    AlreadyConfigured,
    /// The set, scanner or connection has a pending event or packet.
    Busy,
    /// The window overlaps a scheduled event.
    Overlap,
    /// The window starts too soon for the backend to prepare it.
    TooLate,
    /// The window lies beyond what the backend can schedule.
    TooFar,
    /// The backend does not implement this form of the request.
    Unsupported,
    /// No event with this identifier is scheduled.
    UnknownEvent,
    /// The filter accept list has no free entry.
    ListFull,
    /// The filter accept list does not hold the device to remove.
    NotListed,
}

#[cfg(test)]
mod tests {
    use super::{
        AdvertisingEvent, AdvertisingSetId, CoexPriority, CoexistenceLevel, ConnectionAllowances,
        EventId, IdlePriority, RadioTiming,
    };
    use crate::{AdvertisingChannel, AdvertisingChannels, LeInstant, LeWindow, RadioDuration};

    /// Every level keeps its order as a portable priority and returns from
    /// it; the idle priority has no event level.
    #[test]
    fn coexistence_levels_round_trip_through_portable_priorities() {
        let levels = [
            CoexistenceLevel::Baseline,
            CoexistenceLevel::Elevated,
            CoexistenceLevel::Critical,
        ];
        let priorities = levels.map(CoexPriority::from);
        assert!(priorities.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(priorities.map(CoexistenceLevel::try_from), levels.map(Ok));
        assert_eq!(
            CoexistenceLevel::try_from(CoexPriority::Idle),
            Err(IdlePriority)
        );
        assert_eq!(
            CoexPriority::from(CoexistenceLevel::default()),
            CoexPriority::default()
        );
    }

    #[test]
    fn advertising_channels_follow_one_spacing_apart() {
        let event = AdvertisingEvent {
            id: EventId::new(1),
            set: AdvertisingSetId::new(0),
            anchor: LeInstant::from_micros(1_000),
            channels: AdvertisingChannels::new(false, true, true).unwrap(),
            channel_spacing: RadioDuration::from_micros(400),
            coexistence: super::CoexistenceLevel::Baseline,
        };
        assert_eq!(
            event.channel_anchor(1),
            Ok(Some((
                AdvertisingChannel::Channel39,
                LeInstant::from_micros(1_400)
            )))
        );
        assert_eq!(event.channel_anchor(2), Ok(None));
    }

    #[test]
    fn a_reservation_adds_the_preparation_lead() {
        let timing = RadioTiming {
            preparation_lead: RadioDuration::from_micros(300),
            admission_guard: RadioDuration::from_micros(100),
            connection: ConnectionAllowances {
                local_sleep_clock_ppm: 500,
                widening_jitter: RadioDuration::from_micros(0),
                receive_guard: RadioDuration::from_micros(0),
                receive_tail: RadioDuration::from_micros(0),
                boundary_guard: RadioDuration::from_micros(0),
                first_event_guard: RadioDuration::from_micros(0),
                event_length: RadioDuration::from_micros(0),
                first_event_length: RadioDuration::from_micros(0),
            },
        };
        let air = LeWindow::new(
            LeInstant::from_micros(1_000),
            RadioDuration::from_micros(50),
        )
        .unwrap();
        let reserved = timing.reservation(air).unwrap();
        assert_eq!(reserved.start(), LeInstant::from_micros(700));
        assert_eq!(reserved.end(), LeInstant::from_micros(1_050));
        let early =
            LeWindow::new(LeInstant::from_micros(10), RadioDuration::from_micros(5)).unwrap();
        assert_eq!(
            timing.reservation(early),
            Err(super::TimingError::BeforeEpoch)
        );
    }
}
