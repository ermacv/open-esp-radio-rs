//! Bounded receive accounting for the ESP32-S31 DTM recycle callback.
//!
//! This is Lower Link Layer state above the controller-memory parser. Each
//! call consumes exactly one lower returned-packet observation and contains no
//! loop, allocation, MMIO, waker or RTOS dependency. The session itself is
//! retained by the affine event chain, so an observation cannot be accounted
//! twice or applied to another graph.

#![forbid(unsafe_code)]

#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_bluetooth_memory::DtmRxResultProjection;

use oer_esp32s31_bluetooth_memory::{DtmRxResultProjectionError, DtmRxRssi};

/// Pure receive-count state retained across one active DTM receiver test.
#[derive(Debug, Eq, PartialEq)]
pub struct DtmReceiverSession {
    received_packet_count: u16,
    last_rssi: Option<DtmRxRssi>,
}

impl DtmReceiverSession {
    /// Construct an empty semantic session.
    ///
    /// The vendor environment stores `0x7f` before the first accepted packet.
    /// Rust retains absence as `None` instead of exposing that storage sentinel
    /// as a measured RSSI sample.
    pub const fn new() -> Self {
        Self {
            received_packet_count: 0,
            last_rssi: None,
        }
    }

    /// Account one semantic projection retained by its lower packet owner.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn account_projection(
        &mut self,
        projection: Result<DtmRxResultProjection, DtmRxResultProjectionError>,
    ) -> DtmRxCompletionOutcome {
        match projection {
            Ok(result) => {
                self.last_rssi = Some(result.rssi());
                self.received_packet_count = self.received_packet_count.wrapping_add(1);
                DtmRxCompletionOutcome::Counted {
                    received_packet_count: self.received_packet_count,
                    rssi: result.rssi(),
                }
            }
            Err(error) => DtmRxCompletionOutcome::NotCounted { error },
        }
    }

    /// Count serialized as the two-byte LE Test End return parameter.
    pub const fn received_packet_count(&self) -> u16 {
        self.received_packet_count
    }

    /// Return the last accepted signed RSSI, if one packet has been counted.
    pub const fn last_rssi(&self) -> Option<DtmRxRssi> {
        self.last_rssi
    }
}

impl Default for DtmReceiverSession {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of one completed lower RX drain/rotation transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the DTM RX completion outcome must reach the test owner"]
pub enum DtmRxCompletionOutcome {
    /// The scheduler event returned no completed packet.
    NoReturnedPacket,
    /// The result word updated the signed RSSI and wrapping packet count.
    Counted {
        /// Count after this buffer was accepted.
        received_packet_count: u16,
        /// Signed controller RSSI copied from packet-buffer offset `+0x0f`.
        rssi: DtmRxRssi,
    },
    /// The low 24 bits prevented this buffer from changing DTM result state.
    NotCounted {
        /// Exact positional validation failure.
        error: DtmRxResultProjectionError,
    },
}

#[cfg(test)]
mod tests;
