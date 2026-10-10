//! Backend observations.

use oer_radio_port::LifecycleEvent;

use crate::{ConnectionId, EventId, LeInstant};

/// One PDU received during an event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedPdu<'pdu> {
    /// The complete PDU: two-byte header and payload.
    pub pdu: &'pdu [u8],
    /// Receive strength.
    pub rssi_dbm: i8,
    /// The on-air start, or `None` when the hardware captured none.
    pub captured_at: Option<LeInstant>,
}

/// How a scheduled event ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventResult {
    /// The radio executed the event.
    Executed {
        /// The on-air start of the central's first packet of a connection
        /// event, if one arrived.
        anchor: Option<LeInstant>,
    },
    /// The event left the schedule without executing: it was cancelled,
    /// skipped or its window passed while the radio was stopped.
    NotExecuted,
    /// The radio withdrew the event after publishing it to its hardware,
    /// which left no completion status: the hardware may or may not have
    /// executed it. The hardware no longer reaches the event's memory, so
    /// its resources are returned; it establishes no anchor.
    Aborted,
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
    /// The peer acknowledged the connection's queued PDU, right before the
    /// end of the event that carried it.
    TransmitAcknowledged(ConnectionId),
    /// A Direct Test Mode receiver event's result, before its end.
    TestReport {
        /// The event.
        id: EventId,
        /// The result.
        report: TestReport,
    },
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
}
