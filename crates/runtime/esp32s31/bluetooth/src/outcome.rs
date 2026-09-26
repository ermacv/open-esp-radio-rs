//! Owned copies of the portable outcomes.

use oer_bluetooth_radio::{
    ConnectionId, EventId, EventResult, RadioFault, RadioInstant, RadioOutcome, ReceivedPdu,
    TestReport,
};

/// The longest Link Layer PDU: a two-byte header and a 251-byte payload.
pub const MAX_PDU_BYTES: usize = 2 + 251;

/// A received PDU copied out of its receive chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BluetoothReceivedPdu {
    bytes: [u8; MAX_PDU_BYTES],
    len: u8,
    /// Receive strength.
    pub rssi_dbm: i8,
    /// When the backend captured the packet, when it can project it.
    pub captured_at: Option<RadioInstant>,
}

impl BluetoothReceivedPdu {
    #[cfg(any(target_arch = "riscv32", test))]
    fn copy(pdu: ReceivedPdu<'_>) -> Option<Self> {
        let len = u8::try_from(pdu.pdu.len())
            .ok()
            .filter(|len| usize::from(*len) <= MAX_PDU_BYTES)?;
        let mut bytes = [0; MAX_PDU_BYTES];
        bytes[..usize::from(len)].copy_from_slice(pdu.pdu);
        Some(Self {
            bytes,
            len,
            rssi_dbm: pdu.rssi_dbm,
            captured_at: pdu.captured_at,
        })
    }

    /// The complete PDU: two-byte header and payload.
    pub fn pdu(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    /// Lend the PDU as a portable value.
    pub fn portable(&self) -> ReceivedPdu<'_> {
        ReceivedPdu {
            pdu: self.pdu(),
            rssi_dbm: self.rssi_dbm,
            captured_at: self.captured_at,
        }
    }
}

/// An owned portable [`RadioOutcome`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the no-alloc outcome queue stores received PDUs inline"
)]
pub enum BluetoothOutcome {
    /// A PDU received during the event.
    Received {
        /// The event.
        id: EventId,
        /// The PDU.
        pdu: BluetoothReceivedPdu,
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
    /// A Direct Test Mode receiver event's result.
    TestReport {
        /// The event.
        id: EventId,
        /// The result.
        report: TestReport,
    },
    /// The backend faulted.
    Fault(RadioFault),
}

impl BluetoothOutcome {
    /// Copy `outcome`. A PDU longer than any Link Layer PDU becomes a
    /// memory-inconsistency fault.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn copy(outcome: RadioOutcome<'_>) -> Self {
        match outcome {
            RadioOutcome::Received { id, pdu } => match BluetoothReceivedPdu::copy(pdu) {
                Some(pdu) => Self::Received { id, pdu },
                None => Self::Fault(RadioFault::MemoryInconsistency),
            },
            RadioOutcome::EventEnded { id, result } => Self::EventEnded { id, result },
            RadioOutcome::TransmitAcknowledged(connection) => {
                Self::TransmitAcknowledged(connection)
            }
            RadioOutcome::TestReport { id, report } => Self::TestReport { id, report },
            RadioOutcome::Fault(fault) => Self::Fault(fault),
        }
    }

    /// Lend the outcome as a portable value.
    pub fn portable(&self) -> RadioOutcome<'_> {
        match self {
            Self::Received { id, pdu } => RadioOutcome::Received {
                id: *id,
                pdu: pdu.portable(),
            },
            Self::EventEnded { id, result } => RadioOutcome::EventEnded {
                id: *id,
                result: *result,
            },
            Self::TransmitAcknowledged(connection) => {
                RadioOutcome::TransmitAcknowledged(*connection)
            }
            Self::TestReport { id, report } => RadioOutcome::TestReport {
                id: *id,
                report: *report,
            },
            Self::Fault(fault) => RadioOutcome::Fault(*fault),
        }
    }
}
