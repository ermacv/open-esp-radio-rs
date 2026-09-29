//! HCI commands and event of the LE PHY Update procedure.
//!
//! The Host reads a connection's PHYs, sets its default PHY preference and
//! asks for a PHY Update on one connection (Core Vol 4 Part E 7.8.47 to
//! 7.8.49); the Controller reports the result in LE PHY Update Complete
//! (7.7.65.12). A preference mask has bit 0 for LE 1M, bit 1 for LE 2M and
//! bit 2 for LE Coded. As the vendor Controller checks it
//! (`r_ble_ll_hci_chk_phy_masks`), a reserved bit is an unsupported value and
//! an empty direction without its ALL_PHYS "no preference" bit an invalid
//! parameter. This Controller also refuses LE Coded as unsupported, since it
//! runs connections on LE 1M and LE 2M only.

use bt_hci::{
    cmd::{
        Cmd, Opcode,
        le::{LeReadPhy, LeSetDefaultPhy, LeSetPhy},
    },
    event::EventKind,
    param::{ConnHandle, Error as HciError, Status},
};

use crate::HciCommandPacket;

/// PHYs this Controller runs connections on, as a preference mask.
const SUPPORTED: u8 = 0b011;

/// LE PHY Update Complete without an H4 indicator.
pub const LE_PHY_UPDATE_COMPLETE_EVENT_CAPACITY: usize = 8;
/// The longest response of a PHY command: Read PHY's Command Complete.
pub const LE_PHY_COMMAND_RESPONSE_CAPACITY: usize = 10;

/// The PHYs the Host prefers for each direction; every supported PHY when it
/// has no preference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePhyMasks {
    /// Preferred transmit PHYs.
    pub transmit: u8,
    /// Preferred receive PHYs.
    pub receive: u8,
}

/// A validated PHY command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePhyCommand {
    /// LE Read PHY.
    Read(ConnHandle),
    /// LE Set Default PHY.
    SetDefault(LePhyMasks),
    /// LE Set PHY.
    Set(ConnHandle, LePhyMasks),
}

impl LePhyCommand {
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::Read(_) => LeReadPhy::OPCODE,
            Self::SetDefault(_) => LeSetDefaultPhy::OPCODE,
            Self::Set(..) => LeSetPhy::OPCODE,
        }
    }

    pub(crate) fn decode(command: HciCommandPacket<'_>) -> Result<Self, LePhyDecodeError> {
        let opcode = command.opcode();
        let refuse = |error: HciError| {
            LePhyDecodeError::Malformed(LePhyCommandResponse::refused(opcode, error.to_status()))
        };
        let masks = |all: u8, transmit: u8, receive: u8| {
            // Reserved bits and LE Coded alike.
            if (transmit | receive) & !SUPPORTED != 0 {
                return Err(refuse(HciError::UNSUPPORTED));
            }
            let direction = |no_preference: bool, mask: u8| match (no_preference, mask) {
                (true, _) => Ok(SUPPORTED),
                (false, 0) => Err(refuse(HciError::INVALID_HCI_PARAMETERS)),
                (false, mask) => Ok(mask),
            };
            Ok(LePhyMasks {
                transmit: direction(all & 1 != 0, transmit)?,
                receive: direction(all & 2 != 0, receive)?,
            })
        };
        match *command.parameters() {
            [low, high] if opcode == LeReadPhy::OPCODE => {
                Ok(Self::Read(ConnHandle::new(u16::from_le_bytes([low, high]))))
            }
            [all, transmit, receive] if opcode == LeSetDefaultPhy::OPCODE => {
                masks(all, transmit, receive).map(Self::SetDefault)
            }
            [low, high, all, transmit, receive, _, _] if opcode == LeSetPhy::OPCODE => {
                Ok(Self::Set(
                    ConnHandle::new(u16::from_le_bytes([low, high])),
                    masks(all, transmit, receive)?,
                ))
            }
            _ if opcode == LeReadPhy::OPCODE
                || opcode == LeSetDefaultPhy::OPCODE
                || opcode == LeSetPhy::OPCODE =>
            {
                Err(refuse(HciError::INVALID_HCI_PARAMETERS))
            }
            _ => Err(LePhyDecodeError::Unsupported),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LePhyDecodeError {
    Unsupported,
    Malformed(LePhyCommandResponse),
}

/// Owned response to one PHY command: Command Complete of Read PHY and Set
/// Default PHY, Command Status of Set PHY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePhyCommandResponse {
    bytes: [u8; LE_PHY_COMMAND_RESPONSE_CAPACITY],
    len: u8,
    opcode: Opcode,
}

impl LePhyCommandResponse {
    fn complete(opcode: Opcode, status: Status, returned: &[u8]) -> Self {
        let [low, high] = opcode.to_raw().to_le_bytes();
        let mut bytes = [0; LE_PHY_COMMAND_RESPONSE_CAPACITY];
        let len = 6 + returned.len();
        bytes[..6].copy_from_slice(&[
            EventKind::CommandComplete.0,
            (len - 2) as u8,
            1,
            low,
            high,
            status.into_inner(),
        ]);
        bytes[6..len].copy_from_slice(returned);
        Self {
            bytes,
            len: len as u8,
            opcode,
        }
    }

    /// LE Read PHY of `handle` completed with `status` and the PHYs, 1 for
    /// LE 1M and 2 for LE 2M.
    pub fn read(status: Status, handle: ConnHandle, transmit: u8, receive: u8) -> Self {
        let [low, high] = handle.raw().to_le_bytes();
        Self::complete(LeReadPhy::OPCODE, status, &[low, high, transmit, receive])
    }

    /// LE Set Default PHY completed with `status`.
    pub fn set_default(status: Status) -> Self {
        Self::complete(LeSetDefaultPhy::OPCODE, status, &[])
    }

    /// Command Status of LE Set PHY.
    pub fn set(status: Status) -> Self {
        let [low, high] = LeSetPhy::OPCODE.to_raw().to_le_bytes();
        let mut bytes = [0; LE_PHY_COMMAND_RESPONSE_CAPACITY];
        bytes[..6].copy_from_slice(&[
            EventKind::CommandStatus.0,
            4,
            status.into_inner(),
            1,
            low,
            high,
        ]);
        Self {
            bytes,
            len: 6,
            opcode: LeSetPhy::OPCODE,
        }
    }

    /// The response of `opcode` refusing its command with `status`.
    pub fn refused(opcode: Opcode, status: Status) -> Self {
        if opcode == LeReadPhy::OPCODE {
            Self::read(status, ConnHandle::new(0), 0, 0)
        } else if opcode == LeSetPhy::OPCODE {
            Self::set(status)
        } else {
            Self::set_default(status)
        }
    }

    /// Opcode of the answered command.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete HCI event body without an H4 packet indicator.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// Owned LE PHY Update Complete event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePhyUpdateCompleteEvent {
    bytes: [u8; LE_PHY_UPDATE_COMPLETE_EVENT_CAPACITY],
}

impl LePhyUpdateCompleteEvent {
    /// The PHY Update of `handle` ended with `status` on the PHYs, 1 for LE
    /// 1M and 2 for LE 2M.
    pub fn new(status: Status, handle: ConnHandle, transmit: u8, receive: u8) -> Self {
        let [low, high] = handle.raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::Le.0,
                6,
                0x0c,
                status.into_inner(),
                low,
                high,
                transmit,
                receive,
            ],
        }
    }

    /// Complete HCI event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests;
