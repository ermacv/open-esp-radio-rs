//! Caller requests and long-lived role configuration.

use crate::{
    AdvertisingChannels, AdvertisingPdu, DataChannel, DataPdu, RadioDuration, RadioInstant,
    RadioWindow, TestChannel, TestPayloadType, channel::AdvertisingChannel,
};

/// Caller-assigned identifier correlating one event with its outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct EventId(u32);

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

/// Configuration of one legacy advertising set.
///
/// Without a scan response the set advertises non-connectably; with one it
/// answers scan requests and accepts connection indications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingConfiguration<'pdu> {
    /// The set.
    pub set: AdvertisingSetId,
    /// The advertising PDU.
    pub pdu: AdvertisingPdu<'pdu>,
    /// The scan response of a response-capable set.
    pub scan_response: Option<AdvertisingPdu<'pdu>>,
    /// Transmit power.
    pub tx_power: TxPower,
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
    pub anchor: RadioInstant,
    /// Channels used in index order.
    pub channels: AdvertisingChannels,
    /// Time between the anchors of consecutive channels.
    pub channel_spacing: RadioDuration,
}

impl AdvertisingEvent {
    /// The channel and its anchor at `position` in channel order.
    pub fn channel_anchor(&self, position: usize) -> Option<(AdvertisingChannel, RadioInstant)> {
        let channel = self.channels.iter().nth(position)?;
        let offset = u64::from(self.channel_spacing.as_micros()).checked_mul(position as u64)?;
        let anchor = RadioInstant::from_micros(self.anchor.as_micros().checked_add(offset)?);
        Some((channel, anchor))
    }
}

/// Configuration of one passive scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScannerConfiguration {
    /// The scanner.
    pub scanner: ScannerId,
    /// Transmit power retained by the scanner profile.
    pub tx_power: TxPower,
}

/// One passive scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanWindow {
    /// Correlation.
    pub id: EventId,
    /// The scanner.
    pub scanner: ScannerId,
    /// Channel listened on.
    pub channel: AdvertisingChannel,
    /// Air window of the listening.
    pub window: RadioWindow,
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
    /// When the connection indication was received; the first reference
    /// point until a valid reception replaces it.
    pub created_at: RadioInstant,
    /// Transmit power.
    pub tx_power: TxPower,
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
    /// Air window: its start is the earliest anchor, its duration the event
    /// length.
    pub window: RadioWindow,
    /// Receive wait of the event.
    pub timing: ConnectionEventTiming,
    /// Scheduling priority, 0 to 15.
    pub priority: u8,
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
    pub window: RadioWindow,
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
    pub window: RadioWindow,
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
}

impl RadioTiming {
    /// The reservation of an air window, or `None` before the epoch.
    pub fn reservation(&self, window: RadioWindow) -> Option<RadioWindow> {
        let start = window
            .start()
            .as_micros()
            .checked_sub(u64::from(self.preparation_lead.as_micros()))?;
        let duration = window
            .duration()
            .as_micros()
            .checked_add(self.preparation_lead.as_micros())?;
        RadioWindow::new(
            RadioInstant::from_micros(start),
            RadioDuration::from_micros(duration),
        )
        .ok()
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
    /// The backend is stopped or faulted.
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::{AdvertisingEvent, AdvertisingSetId, EventId, RadioTiming};
    use crate::{
        AdvertisingChannel, AdvertisingChannels, RadioDuration, RadioInstant, RadioWindow,
    };

    #[test]
    fn advertising_channels_follow_one_spacing_apart() {
        let event = AdvertisingEvent {
            id: EventId::new(1),
            set: AdvertisingSetId::new(0),
            anchor: RadioInstant::from_micros(1_000),
            channels: AdvertisingChannels::new(false, true, true).unwrap(),
            channel_spacing: RadioDuration::from_micros(400),
        };
        assert_eq!(
            event.channel_anchor(1),
            Some((
                AdvertisingChannel::Channel39,
                RadioInstant::from_micros(1_400)
            ))
        );
        assert!(event.channel_anchor(2).is_none());
    }

    #[test]
    fn a_reservation_adds_the_preparation_lead() {
        let timing = RadioTiming {
            preparation_lead: RadioDuration::from_micros(300),
            admission_guard: RadioDuration::from_micros(100),
        };
        let air = RadioWindow::new(
            RadioInstant::from_micros(1_000),
            RadioDuration::from_micros(50),
        )
        .unwrap();
        let reserved = timing.reservation(air).unwrap();
        assert_eq!(reserved.start(), RadioInstant::from_micros(700));
        assert_eq!(reserved.end(), RadioInstant::from_micros(1_050));
        let early =
            RadioWindow::new(RadioInstant::from_micros(10), RadioDuration::from_micros(5)).unwrap();
        assert_eq!(timing.reservation(early), None);
    }
}
