//! Retained HCI packet ownership and complete packet validation.
//!
//! The asynchronous in-process Host/Controller transport is
//! `oer-bluetooth-hci-transport`; this module owns its synchronous parts.

use bt_hci::PacketKind;

mod packet;
mod queue;

pub use queue::{ControllerToHostQueue, ControllerToHostQueueError};

/// Validate one complete Controller-to-Host packet body of `kind`.
///
/// Commands travel the other direction; every other packet must decode
/// through `bt-hci` and end exactly at the length its header declares.
pub fn validate_controller_to_host_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostQueueError> {
    packet::validate_complete_packet(kind, bytes)
}

/// Maximum packet body accepted by the in-process HCI Host contract.
///
/// The packet indicator used by UART/H4 is not retained because the direct
/// in-process boundary carries [`bt_hci::PacketKind`] separately. Future ISO or larger
/// ACL profiles must introduce a separately reviewed storage profile instead
/// of silently widening every controller allocation.
pub const INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY: usize = 258;

#[cfg(test)]
mod tests;
