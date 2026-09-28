//! Trace points of the connected station.
//!
//! Every event the station records into [`oer_trace`] is defined here with
//! its channel, so the host decodes a drained record with the same type. The
//! channels are the ieee80211 domain's indices; one channel gates one event,
//! so the host can leave the high-rate network TX reports off while it
//! records beacons and control.
//!
//! | index | event                   | rate under saturated traffic |
//! |-------|-------------------------|------------------------------|
//! | 0     | [`BeaconDispatch`]      | one per received beacon      |
//! | 1     | [`ControlMailbox`]      | one per control event        |
//! | 2     | [`PowerInputTrace`]     | TBTT, timers and coexistence |
//! | 3     | [`NetworkTxPowerTrace`] | one per network transaction  |
//! | 4     | [`BeaconMonitorTrace`]  | one per beacon or probe      |
//! | 5     | [`ControlExit`]         | one per connected epoch      |

use core::fmt;

use oer_trace::{Channel, Domain, Event, Kind};

use crate::connected_control::ConnectedDisconnectReason;

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Ieee80211, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Ieee80211, index)
}

/// What RX dispatch did with a received beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BeaconVerdict {
    /// Published to control as the associated BSS's beacon.
    Published = 0,
    /// Its management body could not be copied out of the RX segment.
    Unextracted = 1,
    /// Not the associated BSS's beacon, or malformed.
    Rejected = 2,
    /// Its RX metadata was unavailable.
    NoMetadata = 3,
}

impl BeaconVerdict {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Published,
            1 => Self::Unextracted,
            2 => Self::Rejected,
            3 => Self::NoMetadata,
            _ => return None,
        })
    }
}

/// One beacon RX dispatch classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconDispatch {
    pub verdict: BeaconVerdict,
    /// The beacon's RSSI, when its metadata carried one.
    pub rssi_dbm: Option<i8>,
    /// The low word of the access point TSF in a published beacon.
    pub timestamp_tsf_low: u32,
}

impl Event for BeaconDispatch {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        let rssi = match self.rssi_dbm {
            Some(rssi) => 0x100 | u32::from(rssi as u8),
            None => 0,
        };
        [(self.verdict as u32) | (rssi << 8), self.timestamp_tsf_low]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 17 != 0 {
            return None;
        }
        let rssi = (words[0] >> 8) & 0x1ff;
        Some(Self {
            verdict: BeaconVerdict::from_raw(words[0] & 0xff)?,
            rssi_dbm: (rssi & 0x100 != 0).then_some(rssi as u8 as i8),
            timestamp_tsf_low: words[1],
        })
    }
}

impl fmt::Display for BeaconDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "beacon {:?}", self.verdict)?;
        if let Some(rssi) = self.rssi_dbm {
            write!(f, " rssi={rssi}dBm")?;
        }
        write!(f, " tsf_low={}", self.timestamp_tsf_low)
    }
}

/// The kind of a control event, without its payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControlEventKind {
    Beacon = 0,
    ProbeResponse = 1,
    Trigger = 2,
    Ndpa = 3,
    BlockAck = 4,
    IndividualTwt = 5,
    PeerDisconnect = 6,
    UnprotectedDisconnect = 7,
    SaQuery = 8,
    PowerSaveData = 9,
}

impl ControlEventKind {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Beacon,
            1 => Self::ProbeResponse,
            2 => Self::Trigger,
            3 => Self::Ndpa,
            4 => Self::BlockAck,
            5 => Self::IndividualTwt,
            6 => Self::PeerDisconnect,
            7 => Self::UnprotectedDisconnect,
            8 => Self::SaQuery,
            9 => Self::PowerSaveData,
            _ => return None,
        })
    }
}

impl From<&crate::connected_rx::ConnectedRxControlEvent> for ControlEventKind {
    fn from(event: &crate::connected_rx::ConnectedRxControlEvent) -> Self {
        use crate::connected_rx::ConnectedRxControlEvent as Event;
        match event {
            Event::Beacon(_) => Self::Beacon,
            Event::ProbeResponse => Self::ProbeResponse,
            Event::Trigger { .. } => Self::Trigger,
            Event::Ndpa { .. } => Self::Ndpa,
            Event::BlockAck(_) => Self::BlockAck,
            Event::IndividualTwt(_) => Self::IndividualTwt,
            Event::PeerDisconnect(_) => Self::PeerDisconnect,
            Event::UnprotectedDisconnect(_) => Self::UnprotectedDisconnect,
            Event::SaQuery(_) => Self::SaQuery,
            Event::PowerSaveData(_) => Self::PowerSaveData,
        }
    }
}

/// What happened to a control event between RX and the control core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MailboxOp {
    /// RX published it to the control mailbox.
    Published = 0,
    /// A full mailbox refused it.
    Overflowed = 1,
    /// The control core applied it.
    Applied = 2,
}

impl MailboxOp {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Published,
            1 => Self::Overflowed,
            2 => Self::Applied,
            _ => return None,
        })
    }
}

/// A control event crossing the mailbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlMailbox {
    pub event: ControlEventKind,
    pub op: MailboxOp,
}

impl Event for ControlMailbox {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        [self.event as u32, self.op as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            event: ControlEventKind::from_raw(words[0])?,
            op: MailboxOp::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for ControlMailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "control {:?} {:?}", self.event, self.op)
    }
}

/// A power input other than network TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PowerInputKind {
    Start = 0,
    Null = 1,
    Tbtt = 2,
    CoexPhase = 3,
    Preemption = 4,
    Timer = 5,
}

impl PowerInputKind {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Start,
            1 => Self::Null,
            2 => Self::Tbtt,
            3 => Self::CoexPhase,
            4 => Self::Preemption,
            5 => Self::Timer,
            _ => return None,
        })
    }
}

/// The control core performed a power input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerInputTrace {
    pub input: PowerInputKind,
    /// Whether a control event waited, and so takes the next step.
    pub control_event_waiting: bool,
}

impl Event for PowerInputTrace {
    const KIND: Kind = kind(3);
    const CHANNEL: Channel = channel(2);

    fn encode(&self) -> [u32; 2] {
        [self.input as u32, u32::from(self.control_event_waiting)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            input: PowerInputKind::from_raw(words[0])?,
            control_event_waiting: match words[1] {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl fmt::Display for PowerInputTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "power {:?}", self.input)?;
        if self.control_event_waiting {
            f.write_str(" (event waits)")?;
        }
        Ok(())
    }
}

/// The control core gave power management a network TX input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkTxPowerTrace {
    /// A queued frame was offered before it left, rather than reported
    /// after it started.
    pub offer: bool,
    pub control_event_waiting: bool,
}

impl Event for NetworkTxPowerTrace {
    const KIND: Kind = kind(4);
    const CHANNEL: Channel = channel(3);

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.offer), u32::from(self.control_event_waiting)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let flag = |word: u32| match word {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        Some(Self {
            offer: flag(words[0])?,
            control_event_waiting: flag(words[1])?,
        })
    }
}

impl fmt::Display for NetworkTxPowerTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.offer {
            "power network offer"
        } else {
            "power network report"
        })?;
        if self.control_event_waiting {
            f.write_str(" (event waits)")?;
        }
        Ok(())
    }
}

/// A beacon monitor transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BeaconMonitorOp {
    /// A beacon refreshed the deadline.
    Refreshed = 0,
    /// The deadline passed and an active probe starts.
    ProbeStarted = 1,
    /// The access point answered a probe.
    ProbeAnswered = 2,
    /// The probes went unanswered; the link is lost.
    Lost = 3,
}

impl BeaconMonitorOp {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Refreshed,
            1 => Self::ProbeStarted,
            2 => Self::ProbeAnswered,
            3 => Self::Lost,
            _ => return None,
        })
    }
}

/// The beacon monitor changed state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconMonitorTrace {
    pub op: BeaconMonitorOp,
    /// The low word of the monitor's deadline afterwards, in microseconds.
    pub deadline_micros_low: u32,
}

impl Event for BeaconMonitorTrace {
    const KIND: Kind = kind(5);
    const CHANNEL: Channel = channel(4);

    fn encode(&self) -> [u32; 2] {
        [self.op as u32, self.deadline_micros_low]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            op: BeaconMonitorOp::from_raw(words[0])?,
            deadline_micros_low: words[1],
        })
    }
}

impl fmt::Display for BeaconMonitorTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "beacon monitor {:?} deadline_low={}us",
            self.op, self.deadline_micros_low
        )
    }
}

/// A connected epoch ended; recording freezes a window after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlExit {
    pub reason: ConnectedDisconnectReason,
}

impl ControlExit {
    /// Entries still recorded after the exit: the teardown that follows.
    pub const POST_TRIGGER_ENTRIES: u32 = 64;
}

impl Event for ControlExit {
    const KIND: Kind = kind(6);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        use ConnectedDisconnectReason as Reason;
        match self.reason {
            Reason::BeaconLoss => [0, 0],
            Reason::PeerDeauthentication { reason_code } => [1, u32::from(reason_code)],
            Reason::PeerDisassociation { reason_code } => [2, u32::from(reason_code)],
            Reason::ControlMailboxOverflow => [3, 0],
            Reason::ActiveStateRestoreFailed => [4, 0],
            Reason::GroupKeyHandshakeFailed => [5, 0],
            Reason::SaQueryTimeout => [6, 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        use ConnectedDisconnectReason as Reason;
        let reason_code = u16::try_from(words[1]).ok()?;
        let reason = match (words[0], reason_code) {
            (0, 0) => Reason::BeaconLoss,
            (1, reason_code) => Reason::PeerDeauthentication { reason_code },
            (2, reason_code) => Reason::PeerDisassociation { reason_code },
            (3, 0) => Reason::ControlMailboxOverflow,
            (4, 0) => Reason::ActiveStateRestoreFailed,
            (5, 0) => Reason::GroupKeyHandshakeFailed,
            (6, 0) => Reason::SaQueryTimeout,
            _ => return None,
        };
        Some(Self { reason })
    }
}

impl fmt::Display for ControlExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connected exit {:?}", self.reason)
    }
}

#[cfg(test)]
mod tests;
