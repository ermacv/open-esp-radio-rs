//! Shared HCI packet slots and complete Controller packet validation.

use super::ControllerToHostQueueError;
use bt_hci::{ControllerToHostPacket, FromHciBytesError, PacketKind};

#[derive(Clone, Copy)]
pub(super) struct PacketSlot<const PACKET_CAPACITY: usize> {
    pub(super) kind: PacketKind,
    pub(super) length: usize,
    pub(super) bytes: [u8; PACKET_CAPACITY],
}

impl<const PACKET_CAPACITY: usize> PacketSlot<PACKET_CAPACITY> {
    pub(super) const EMPTY: Self = Self {
        kind: PacketKind::Event,
        length: 0,
        bytes: [0; PACKET_CAPACITY],
    };
}

pub(super) fn decode_complete_packet(
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

pub(super) fn validate_complete_packet(
    kind: PacketKind,
    bytes: &[u8],
) -> Result<(), ControllerToHostQueueError> {
    decode_complete_packet(kind, bytes).map(|_| ())
}
