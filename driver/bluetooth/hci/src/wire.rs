//! Borrowed HCI command and Host data packet representations.
//!
//! Packet kinds are separate from packet bodies; these types add no UART/H4
//! framing and carry no Controller epoch or command/response authority.

use bt_hci::{
    PacketKind,
    cmd::{Opcode, OpcodeGroup},
    data::{AclPacket, IsoPacket, SyncPacket},
};

/// A generic HCI command decoded at the Controller side of the boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HciCommandPacket<'packet> {
    opcode: Opcode,
    parameters: &'packet [u8],
}

impl HciCommandPacket<'_> {
    /// Command opcode, including its OGF and OCF fields.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete command parameter bytes following the three-byte HCI header.
    pub const fn parameters(&self) -> &[u8] {
        self.parameters
    }

    #[cfg(test)]
    pub(crate) const fn for_test(opcode: Opcode, parameters: &[u8]) -> HciCommandPacket<'_> {
        HciCommandPacket { opcode, parameters }
    }
}

/// One complete packet emitted by the Host and consumed by the Controller.
#[derive(Debug)]
pub enum HostToControllerFrame<'packet> {
    /// A generic HCI command for dispatch by opcode.
    Command(HciCommandPacket<'packet>),
    /// Host ACL data.
    Acl(AclPacket<'packet>),
    /// Host synchronous data.
    Sync(SyncPacket<'packet>),
    /// Host isochronous data.
    Iso(IsoPacket<'packet>),
}

impl HostToControllerFrame<'_> {
    /// Packet kind carried separately from the in-process packet body.
    pub const fn kind(&self) -> PacketKind {
        match self {
            Self::Command(_) => PacketKind::Cmd,
            Self::Acl(_) => PacketKind::AclData,
            Self::Sync(_) => PacketKind::SyncData,
            Self::Iso(_) => PacketKind::IsoData,
        }
    }
}

/// Decode a command body after the transport has validated its full length.
pub(crate) fn command_from_validated_bytes(bytes: &[u8]) -> HciCommandPacket<'_> {
    let raw = u16::from_le_bytes([bytes[0], bytes[1]]);
    HciCommandPacket {
        opcode: Opcode::new(OpcodeGroup::new((raw >> 10) as u8), raw & 0x03ff),
        parameters: &bytes[3..],
    }
}
