//! Backend observations.

use crate::{ConnectionId, EventId, RadioInstant};

/// One PDU received during an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedPdu<'pdu> {
    /// The complete PDU: two-byte header and payload.
    pub pdu: &'pdu [u8],
    /// Receive strength.
    pub rssi_dbm: i8,
    /// When the backend captured the packet, when it can project it.
    pub captured_at: Option<RadioInstant>,
}

/// How a scheduled event ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventResult {
    /// The radio executed the event.
    Executed {
        /// The anchor a connection event captured, if any.
        anchor: Option<RadioInstant>,
    },
    /// The event left the schedule without executing: it was cancelled,
    /// skipped or its window passed while the radio was stopped.
    NotExecuted,
}

/// What a Direct Test Mode receiver event returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestReport {
    /// No packet was returned.
    Nothing,
    /// One packet arrived with a valid CRC.
    Received {
        /// Receive strength.
        rssi_dbm: i8,
    },
    /// One packet arrived and failed its check.
    Failed,
}

/// A backend fault. The backend stops scheduling until it is restarted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioFault {
    /// Hardware reported a result the backend cannot interpret.
    UnsupportedHardwareResult,
    /// A receive or transmit structure was found inconsistent.
    MemoryInconsistency,
    /// Hardware executed events out of their scheduled order.
    OutOfOrderCompletion,
}

/// One observation from the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioOutcome<'pdu> {
    /// A PDU received during the event; any number precede its end.
    Received {
        /// The event.
        id: EventId,
        /// The PDU.
        pdu: ReceivedPdu<'pdu>,
    },
    /// The event ended.
    EventEnded {
        /// The event.
        id: EventId,
        /// How it ended.
        result: EventResult,
    },
    /// The peer acknowledged the connection's queued PDU.
    TransmitAcknowledged(ConnectionId),
    /// A Direct Test Mode receiver event's result, before its end.
    TestReport {
        /// The event.
        id: EventId,
        /// The result.
        report: TestReport,
    },
    /// The backend faulted.
    Fault(RadioFault),
}
