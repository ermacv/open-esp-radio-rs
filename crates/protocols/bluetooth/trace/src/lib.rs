#![no_std]
#![forbid(unsafe_code)]

//! Trace points of the Bluetooth LE radio role.
//!
//! Every event the role records into [`oer_trace`] is defined here with its
//! channel, so the host decodes a drained record with the same type. The
//! channels are the bluetooth domain's indices.
//!
//! | index | event              | rate                          |
//! |-------|--------------------|-------------------------------|
//! | 0     | [`ScanWindowTrace`] | one per requested scan window |
//! | 1     | [`ScanEventTrace`]  | one per ended scan event      |

use core::fmt;

use oer_trace::{Channel, Domain, Event, Kind};

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Bluetooth, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Bluetooth, index)
}

/// How the role answered a scan window request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ScanWindowVerdict {
    /// The scheduler item was submitted.
    Submitted = 0,
    /// The scanner still owns an event.
    Busy = 1,
    /// The window starts before the admission guard allows.
    TooLate = 2,
    /// The window ends beyond the scheduler's horizon.
    TooFar = 3,
    /// The window overlaps a listed or pending reservation.
    Overlap = 4,
    /// The scheduler list had no room.
    NoCapacity = 5,
    /// The scanner memory refused to prepare the event.
    Unprepared = 6,
}

impl ScanWindowVerdict {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Submitted,
            1 => Self::Busy,
            2 => Self::TooLate,
            3 => Self::TooFar,
            4 => Self::Overlap,
            5 => Self::NoCapacity,
            6 => Self::Unprepared,
            _ => return None,
        })
    }
}

/// One scan window request of the Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanWindowTrace {
    pub verdict: ScanWindowVerdict,
    /// Primary advertising channel, 37 to 39.
    pub channel: u8,
    pub duration_micros: u32,
}

impl Event for ScanWindowTrace {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        [
            self.verdict as u32 | u32::from(self.channel) << 8,
            self.duration_micros,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 16 != 0 {
            return None;
        }
        Some(Self {
            verdict: ScanWindowVerdict::from_raw(words[0] & 0xff)?,
            channel: (words[0] >> 8) as u8,
            duration_micros: words[1],
        })
    }
}

impl fmt::Display for ScanWindowTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "scan window {:?} channel={} duration={}us",
            self.verdict, self.channel, self.duration_micros
        )
    }
}

/// One ended scan event and what its receive chain held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanEventTrace {
    /// The receive chain of the event was available.
    pub source: bool,
    /// The chain was inconsistent.
    pub fault: bool,
    /// PDUs published to the Controller.
    pub received: u16,
    /// PDUs the receive chain discarded.
    pub discarded: u16,
}

impl Event for ScanEventTrace {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        [
            u32::from(self.source) | u32::from(self.fault) << 1,
            u32::from(self.received) | u32::from(self.discarded) << 16,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 2 != 0 {
            return None;
        }
        Some(Self {
            source: words[0] & 1 != 0,
            fault: words[0] & 2 != 0,
            received: words[1] as u16,
            discarded: (words[1] >> 16) as u16,
        })
    }
}

impl fmt::Display for ScanEventTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "scan event source={} fault={} received={} discarded={}",
            self.source, self.fault, self.received, self.discarded
        )
    }
}

oer_trace::event_set!(pub RadioRoleTrace: ScanWindowTrace, ScanEventTrace);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_survive_their_encoding() {
        let window = ScanWindowTrace {
            verdict: ScanWindowVerdict::Unprepared,
            channel: 39,
            duration_micros: 30_000,
        };
        assert_eq!(ScanWindowTrace::decode(window.encode()), Some(window));
        let event = ScanEventTrace {
            source: true,
            fault: false,
            received: 3,
            discarded: 65535,
        };
        assert_eq!(ScanEventTrace::decode(event.encode()), Some(event));
        assert_eq!(ScanWindowTrace::decode([5, 0]), None);
        assert_eq!(ScanEventTrace::decode([4, 0]), None);
    }
}
