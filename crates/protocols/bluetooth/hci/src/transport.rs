//! Bounded HCI packet transport and retained packet ownership.

mod in_process;
mod packet;
mod queue;

pub use in_process::{HciChannelError, HciEpochBound, HciEpochIdentity, InProcessHciHostTransport};
pub(crate) use in_process::{
    HciClassifiedCommandIntake, InProcessHciChannel, InProcessHciControllerEndpoint,
};
pub use queue::{ControllerToHostQueue, ControllerToHostQueueError};

/// Maximum packet body accepted by the in-process HCI Host contract.
///
/// The packet indicator used by UART/H4 is not retained because the direct
/// in-process boundary carries [`bt_hci::PacketKind`] separately. Future ISO or larger
/// ACL profiles must introduce a separately reviewed storage profile instead
/// of silently widening every controller allocation.
pub const INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY: usize = 258;

#[cfg(test)]
mod tests;
