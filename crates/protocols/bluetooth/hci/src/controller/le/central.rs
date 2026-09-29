//! HCI commands of the legacy LE initiator.
//!
//! LE Create Connection starts scanning for one advertiser, or for the filter
//! accept list, and connects to the first one found (Core Vol 4 Part E
//! 7.8.12); LE Create Connection Cancel stops it (7.8.13). This Controller
//! connects from its public or static random address to a public or random
//! peer address; the address types that need a resolving list are refused as
//! unsupported until the Controller resolves private addresses.

use bt_hci::{
    cmd::{
        Cmd, Opcode,
        le::{LeCreateConn, LeCreateConnCancel},
    },
    event::EventKind,
    param::{Error as HciError, Status},
};

use crate::HciCommandPacket;

/// The longest response of an initiator command.
pub const LE_CENTRAL_COMMAND_RESPONSE_CAPACITY: usize = 6;

/// The advertiser an initiator connects to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeInitiatorPeer {
    /// One device: random rather than public, and its address least
    /// significant octet first.
    Device { random: bool, address: [u8; 6] },
    /// Any device of the filter accept list.
    AcceptList,
}

/// Validated LE Create Connection parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeCreateConnectionParameters {
    /// Start-to-start scan interval in 0.625 ms units.
    pub scan_interval_units: u16,
    /// Scan window in 0.625 ms units.
    pub scan_window_units: u16,
    /// The advertiser to connect to.
    pub peer: LeInitiatorPeer,
    /// The own address is the static random address rather than the public
    /// one.
    pub own_random: bool,
    /// Connection interval bounds in 1.25 ms units.
    pub interval_min_units: u16,
    pub interval_max_units: u16,
    /// Maximum Peripheral latency in connection events.
    pub max_latency: u16,
    /// Supervision timeout in 10 ms units.
    pub supervision_timeout_units: u16,
    /// Expected connection event length bounds in 0.625 ms units.
    pub min_event_length_units: u16,
    pub max_event_length_units: u16,
}

/// A validated initiator command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeCentralCommand {
    /// LE Create Connection.
    CreateConnection(LeCreateConnectionParameters),
    /// LE Create Connection Cancel.
    CreateConnectionCancel,
}

impl LeCentralCommand {
    pub const fn opcode(self) -> Opcode {
        match self {
            Self::CreateConnection(_) => LeCreateConn::OPCODE,
            Self::CreateConnectionCancel => LeCreateConnCancel::OPCODE,
        }
    }

    pub(crate) fn decode(command: HciCommandPacket<'_>) -> Result<Self, LeCentralDecodeError> {
        let opcode = command.opcode();
        let refuse = |error: HciError| {
            LeCentralDecodeError::Malformed(LeCentralCommandResponse::refused(
                opcode,
                error.to_status(),
            ))
        };
        if opcode == LeCreateConnCancel::OPCODE {
            return if command.parameters().is_empty() {
                Ok(Self::CreateConnectionCancel)
            } else {
                Err(refuse(HciError::INVALID_HCI_PARAMETERS))
            };
        }
        if opcode != LeCreateConn::OPCODE {
            return Err(LeCentralDecodeError::Unsupported);
        }
        let Ok(p) = <&[u8; 25]>::try_from(command.parameters()) else {
            return Err(refuse(HciError::INVALID_HCI_PARAMETERS));
        };
        let word = |at: usize| u16::from_le_bytes([p[at], p[at + 1]]);
        let parameters = LeCreateConnectionParameters {
            scan_interval_units: word(0),
            scan_window_units: word(2),
            peer: match (p[4], p[5]) {
                (0, kind @ (0 | 1)) => {
                    let mut address = [0; 6];
                    address.copy_from_slice(&p[6..12]);
                    LeInitiatorPeer::Device {
                        random: kind == 1,
                        address,
                    }
                }
                (0, 2 | 3) => return Err(refuse(HciError::UNSUPPORTED)),
                (1, _) => LeInitiatorPeer::AcceptList,
                _ => return Err(refuse(HciError::INVALID_HCI_PARAMETERS)),
            },
            own_random: match p[12] {
                0 => false,
                1 => true,
                2 | 3 => return Err(refuse(HciError::UNSUPPORTED)),
                _ => return Err(refuse(HciError::INVALID_HCI_PARAMETERS)),
            },
            interval_min_units: word(13),
            interval_max_units: word(15),
            max_latency: word(17),
            supervision_timeout_units: word(19),
            min_event_length_units: word(21),
            max_event_length_units: word(23),
        };
        if valid(&parameters) {
            Ok(Self::CreateConnection(parameters))
        } else {
            Err(refuse(HciError::INVALID_HCI_PARAMETERS))
        }
    }
}

/// The ranges and relations of Core Vol 4 Part E 7.8.12.
fn valid(p: &LeCreateConnectionParameters) -> bool {
    let scan = 0x0004..=0x4000;
    let interval = 0x0006..=0x0c80;
    let timeout = 0x000a..=0x0c80;
    scan.contains(&p.scan_interval_units)
        && scan.contains(&p.scan_window_units)
        && p.scan_window_units <= p.scan_interval_units
        && interval.contains(&p.interval_min_units)
        && interval.contains(&p.interval_max_units)
        && p.interval_min_units <= p.interval_max_units
        && p.max_latency <= 0x01f3
        && timeout.contains(&p.supervision_timeout_units)
        // The timeout exceeds (1 + latency) * interval_max * 2, in 1.25 ms
        // units: 10 ms is 8 of them.
        && u32::from(p.supervision_timeout_units) * 8
            > (1 + u32::from(p.max_latency)) * u32::from(p.interval_max_units) * 2
        && p.min_event_length_units <= p.max_event_length_units
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeCentralDecodeError {
    Unsupported,
    Malformed(LeCentralCommandResponse),
}

/// Owned response of one initiator command: Command Status of LE Create
/// Connection, Command Complete of LE Create Connection Cancel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeCentralCommandResponse {
    bytes: [u8; LE_CENTRAL_COMMAND_RESPONSE_CAPACITY],
    opcode: Opcode,
}

impl LeCentralCommandResponse {
    /// Command Status of LE Create Connection.
    pub fn create_connection(status: Status) -> Self {
        let [low, high] = LeCreateConn::OPCODE.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandStatus.0,
                4,
                status.into_inner(),
                1,
                low,
                high,
            ],
            opcode: LeCreateConn::OPCODE,
        }
    }

    /// Command Complete of LE Create Connection Cancel.
    pub fn create_connection_cancel(status: Status) -> Self {
        let [low, high] = LeCreateConnCancel::OPCODE.to_raw().to_le_bytes();
        Self {
            bytes: [
                EventKind::CommandComplete.0,
                4,
                1,
                low,
                high,
                status.into_inner(),
            ],
            opcode: LeCreateConnCancel::OPCODE,
        }
    }

    fn refused(opcode: Opcode, status: Status) -> Self {
        if opcode == LeCreateConnCancel::OPCODE {
            Self::create_connection_cancel(status)
        } else {
            Self::create_connection(status)
        }
    }

    /// Opcode of the answered command.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Complete HCI event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests;
