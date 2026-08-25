#![no_std]
#![forbid(unsafe_code)]

//! Executor-neutral transport and storage at the Bluetooth Host Controller
//! Interface.
//!
//! [`InProcessHciChannel`] splits into a Host transport accepted by
//! `bt_hci::ExternalController` and one affine Controller-worker endpoint. It
//! carries HCI packet bodies with a separate typed packet kind, so no UART/H4
//! framing exists inside the process. Both directions have statically bounded
//! storage, wake-driven backpressure and cancellation-safe waits.
//! [`LeControllerBootstrap`] implements a closed software-only HCI command
//! subset for Host initialization and rejects Link-Layer commands.
//! [`LeControllerBootstrapWorker`] is its sole executor-neutral endpoint owner;
//! it preserves accepted responses across shutdown and backpressure. This crate
//! contains no Link Layer, radio, MMIO, interrupt, executor, allocator, or
//! readiness substitute.

#[cfg(test)]
extern crate std;

mod bootstrap;
mod channel;
mod worker;

pub use bootstrap::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, BootstrapCommand, BootstrapCommandCompleteEvent,
    BootstrapConfigError, BootstrapHostBuffers, BootstrapPhase, LeControllerBootstrap,
    LeControllerBootstrapConfig,
};
pub use bt_hci;
pub use channel::{
    HciChannelError, HciCommandPacket, HostToControllerFrame, InProcessHciChannel,
    InProcessHciControllerEndpoint, InProcessHciHostTransport,
};
pub use worker::{
    BootstrapWorkerError, BootstrapWorkerExit, HciCommandDispatcher, HciCommandWorker,
    HciControllerResponse, LeControllerBootstrapWorker,
};

use bt_hci::{ControllerToHostPacket, FromHciBytesError, PacketKind};

/// Maximum packet body accepted by the pinned `bt-hci` 0.9 Host contract.
///
/// The packet indicator used by UART/H4 is not retained because the direct
/// in-process boundary carries [`PacketKind`] separately. Future ISO or larger
/// ACL profiles must introduce a separately reviewed storage profile instead
/// of silently widening every controller allocation.
pub const INITIAL_CONTROLLER_TO_HOST_PACKET_CAPACITY: usize = 258;

/// A failed publication or receive operation on the bounded HCI queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerToHostQueueError {
    /// Every statically allocated queue slot is occupied.
    Full,
    /// No packet is available to receive.
    Empty,
    /// A Host-to-Controller command cannot enter this directional queue.
    InvalidDirection,
    /// The packet body exceeds this queue profile's fixed storage.
    PacketTooLong {
        /// Supplied packet length.
        length: usize,
        /// Maximum packet length retained by this queue.
        capacity: usize,
    },
    /// The destination cannot retain the complete oldest packet.
    DestinationTooSmall {
        /// Required destination length.
        required: usize,
        /// Supplied destination length.
        available: usize,
    },
    /// `bt-hci` rejected the packet header or declared payload.
    InvalidPacket(FromHciBytesError),
    /// A valid packet was followed by bytes outside its declared HCI length.
    TrailingBytes,
    /// Retained storage no longer decodes to the packet admitted at publish.
    ///
    /// Safe code cannot create this state. The variant keeps future hardware
    /// adapters fail-closed if a storage invariant is ever violated.
    CorruptRetainedPacket,
}

#[derive(Clone, Copy)]
struct PacketSlot<const PACKET_CAPACITY: usize> {
    kind: PacketKind,
    length: usize,
    bytes: [u8; PACKET_CAPACITY],
}

impl<const PACKET_CAPACITY: usize> PacketSlot<PACKET_CAPACITY> {
    const EMPTY: Self = Self {
        kind: PacketKind::Event,
        length: 0,
        bytes: [0; PACKET_CAPACITY],
    };
}

/// Fixed-capacity FIFO from the sole Controller owner to one HCI Host reader.
///
/// Publication validates a complete `bt-hci` packet before changing queue
/// state. A full queue never overwrites an older event or ACL packet. Receive
/// similarly retains the oldest slot if the caller's buffer is too small, so
/// cancellation or retry cannot silently consume controller state.
pub struct ControllerToHostQueue<const DEPTH: usize, const PACKET_CAPACITY: usize> {
    slots: [PacketSlot<PACKET_CAPACITY>; DEPTH],
    head: usize,
    length: usize,
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize>
    ControllerToHostQueue<DEPTH, PACKET_CAPACITY>
{
    /// Construct an empty queue with no allocator or runtime registration.
    pub const fn new() -> Self {
        assert!(DEPTH > 0, "an HCI queue needs at least one packet slot");
        assert!(
            PACKET_CAPACITY > 0,
            "an HCI queue packet slot must retain at least one byte"
        );
        Self {
            slots: [PacketSlot::EMPTY; DEPTH],
            head: 0,
            length: 0,
        }
    }

    /// Number of packets currently retained for the Host.
    pub const fn len(&self) -> usize {
        self.length
    }

    /// Whether no packet is currently retained for the Host.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Number of statically allocated packet slots.
    pub const fn capacity(&self) -> usize {
        DEPTH
    }

    /// Length of the oldest packet without borrowing its contents.
    pub const fn front_len(&self) -> Option<usize> {
        if self.length == 0 {
            None
        } else {
            Some(self.slots[self.head].length)
        }
    }

    /// Validate and publish one complete packet emitted by the Controller.
    pub fn publish(
        &mut self,
        kind: PacketKind,
        bytes: &[u8],
    ) -> Result<(), ControllerToHostQueueError> {
        if kind == PacketKind::Cmd {
            return Err(ControllerToHostQueueError::InvalidDirection);
        }
        if bytes.len() > PACKET_CAPACITY {
            return Err(ControllerToHostQueueError::PacketTooLong {
                length: bytes.len(),
                capacity: PACKET_CAPACITY,
            });
        }
        validate_complete_packet(kind, bytes)?;
        if self.length == DEPTH {
            return Err(ControllerToHostQueueError::Full);
        }

        let tail = (self.head + self.length) % DEPTH;
        let slot = &mut self.slots[tail];
        slot.kind = kind;
        slot.length = bytes.len();
        slot.bytes[..bytes.len()].copy_from_slice(bytes);
        self.length += 1;
        Ok(())
    }

    /// Copy and decode the oldest packet into caller-owned Host read storage.
    ///
    /// The slot is consumed only after the complete packet has been copied and
    /// decoded. Cleared slot bytes cannot retain an earlier event or key-bearing
    /// control payload across a later controller epoch.
    pub fn receive<'buffer>(
        &mut self,
        buffer: &'buffer mut [u8],
    ) -> Result<ControllerToHostPacket<'buffer>, ControllerToHostQueueError> {
        if self.length == 0 {
            return Err(ControllerToHostQueueError::Empty);
        }

        let slot = &mut self.slots[self.head];
        if buffer.len() < slot.length {
            return Err(ControllerToHostQueueError::DestinationTooSmall {
                required: slot.length,
                available: buffer.len(),
            });
        }

        let packet_length = slot.length;
        let kind = slot.kind;
        buffer[..packet_length].copy_from_slice(&slot.bytes[..packet_length]);
        let packet = decode_complete_packet(kind, &buffer[..packet_length])
            .map_err(|_| ControllerToHostQueueError::CorruptRetainedPacket)?;

        slot.bytes[..packet_length].fill(0);
        slot.length = 0;
        self.head = (self.head + 1) % DEPTH;
        self.length -= 1;
        Ok(packet)
    }
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize> Default
    for ControllerToHostQueue<DEPTH, PACKET_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

fn decode_complete_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<ControllerToHostPacket<'_>, ControllerToHostQueueError> {
    validate_declared_length(kind, bytes)?;
    let (packet, remaining) = ControllerToHostPacket::from_hci_bytes_with_kind(kind, bytes)
        .map_err(ControllerToHostQueueError::InvalidPacket)?;
    if remaining.is_empty() {
        Ok(packet)
    } else {
        Err(ControllerToHostQueueError::TrailingBytes)
    }
}

fn validate_declared_length(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostQueueError> {
    let declared = match kind {
        PacketKind::Cmd => return Err(ControllerToHostQueueError::InvalidDirection),
        PacketKind::Event => {
            let Some(length) = bytes.get(1) else {
                return Err(ControllerToHostQueueError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            2 + usize::from(*length)
        }
        PacketKind::AclData => {
            let Some(length) = bytes.get(2..4) else {
                return Err(ControllerToHostQueueError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            4 + usize::from(u16::from_le_bytes([length[0], length[1]]))
        }
        PacketKind::SyncData => {
            let Some(length) = bytes.get(2) else {
                return Err(ControllerToHostQueueError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            3 + usize::from(*length)
        }
        PacketKind::IsoData => {
            let Some(length) = bytes.get(2..4) else {
                return Err(ControllerToHostQueueError::InvalidPacket(
                    FromHciBytesError::InvalidSize,
                ));
            };
            let image = u16::from_le_bytes([length[0], length[1]]);
            4 + usize::from(image & 0x3fff)
        }
    };

    if bytes.len() < declared {
        Err(ControllerToHostQueueError::InvalidPacket(
            FromHciBytesError::InvalidSize,
        ))
    } else if bytes.len() > declared {
        Err(ControllerToHostQueueError::TrailingBytes)
    } else {
        Ok(())
    }
}

fn validate_complete_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostQueueError> {
    decode_complete_packet(kind, bytes).map(|_| ())
}

#[cfg(test)]
mod tests {
    use bt_hci::{ControllerToHostPacket, PacketKind, controller::ExternalController};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use super::{ControllerToHostQueue, ControllerToHostQueueError, InProcessHciHostTransport};

    const RESET_COMMAND_COMPLETE: [u8; 6] = [0x0e, 0x04, 0x01, 0x03, 0x0c, 0x00];
    const HARDWARE_ERROR: [u8; 3] = [0x10, 0x01, 0x42];

    #[test]
    fn queue_preserves_fifo_and_decodes_through_bt_hci() {
        let mut queue = ControllerToHostQueue::<2, 16>::new();
        queue
            .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .unwrap();
        queue.publish(PacketKind::Event, &HARDWARE_ERROR).unwrap();
        assert_eq!(queue.len(), 2);

        let mut buffer = [0; 16];
        let first = queue.receive(&mut buffer).unwrap();
        assert!(matches!(first, ControllerToHostPacket::Event(_)));
        assert_eq!(first.kind(), PacketKind::Event);
        assert_eq!(queue.front_len(), Some(HARDWARE_ERROR.len()));

        let second = queue.receive(&mut buffer).unwrap();
        assert!(matches!(second, ControllerToHostPacket::Event(_)));
        assert!(queue.is_empty());
    }

    #[test]
    fn full_queue_never_overwrites_the_oldest_packet() {
        let mut queue = ControllerToHostQueue::<1, 16>::new();
        queue.publish(PacketKind::Event, &HARDWARE_ERROR).unwrap();

        assert_eq!(
            queue.publish(PacketKind::Event, &RESET_COMMAND_COMPLETE),
            Err(ControllerToHostQueueError::Full)
        );

        let mut buffer = [0; 16];
        let packet = queue.receive(&mut buffer).unwrap();
        let ControllerToHostPacket::Event(event) = packet else {
            panic!("the retained packet changed kind");
        };
        assert_eq!(event.data, &[0x42]);
    }

    #[test]
    fn short_receive_buffer_retains_the_complete_oldest_packet() {
        let mut queue = ControllerToHostQueue::<1, 16>::new();
        queue
            .publish(PacketKind::Event, &RESET_COMMAND_COMPLETE)
            .unwrap();
        let mut short = [0; 5];

        assert!(matches!(
            queue.receive(&mut short),
            Err(ControllerToHostQueueError::DestinationTooSmall {
                required: 6,
                available: 5,
            })
        ));
        assert_eq!(queue.front_len(), Some(RESET_COMMAND_COMPLETE.len()));

        let mut complete = [0; 6];
        assert!(queue.receive(&mut complete).is_ok());
        assert!(queue.is_empty());
    }

    #[test]
    fn invalid_direction_length_and_trailing_bytes_fail_before_publication() {
        let mut queue = ControllerToHostQueue::<1, 6>::new();
        assert_eq!(
            queue.publish(PacketKind::Cmd, &[0x03, 0x0c, 0x00]),
            Err(ControllerToHostQueueError::InvalidDirection)
        );
        assert_eq!(
            queue.publish(PacketKind::Event, &[0x10, 0x00, 0xff]),
            Err(ControllerToHostQueueError::TrailingBytes)
        );
        assert_eq!(
            queue.publish(PacketKind::Event, &[0; 7]),
            Err(ControllerToHostQueueError::PacketTooLong {
                length: 7,
                capacity: 6,
            })
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn every_controller_packet_header_rejects_declared_length_mismatch() {
        let mut queue = ControllerToHostQueue::<1, 16>::new();
        for (kind, bytes) in [
            (PacketKind::AclData, &[0x01, 0x00, 0x00, 0x00, 0xaa][..]),
            (PacketKind::SyncData, &[0x01, 0x00, 0x00, 0xbb][..]),
            (PacketKind::IsoData, &[0x01, 0x00, 0x00, 0x00, 0xcc][..]),
        ] {
            assert_eq!(
                queue.publish(kind, bytes),
                Err(ControllerToHostQueueError::TrailingBytes)
            );
        }
        assert_eq!(
            queue.publish(PacketKind::AclData, &[0x01, 0x00, 0x01]),
            Err(ControllerToHostQueueError::InvalidPacket(
                bt_hci::FromHciBytesError::InvalidSize
            ))
        );
        assert!(queue.is_empty());
    }

    fn requires_trouble_controller<C: trouble_host::Controller>() {}

    #[test]
    fn pinned_bt_hci_release_matches_trouble_controller_contract() {
        type ContractTransport = InProcessHciHostTransport<'static, NoopRawMutex, 1, 1, 16>;
        requires_trouble_controller::<ExternalController<ContractTransport, 1>>();
    }
}
