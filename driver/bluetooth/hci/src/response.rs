//! Complete Controller responses retained by an operational HCI runner.

use bt_hci::{
    PacketKind,
    cmd::Opcode,
    param::{Error as HciError, Status},
};

use crate::{
    BootstrapCommandCompleteEvent, LeDtmCommandCompleteEvent,
    LeLegacyAdvertisingCommandCompleteEvent,
};

const UNKNOWN_COMMAND_COMPLETE_EVENT_CAPACITY: usize = 6;

/// Owned `Unknown HCI Command` completion for one unclaimed opcode.
///
/// The portable Controller classifier terminates its closed command table with
/// this response rather than returning a packet which borrows the receive
/// scratch buffer. A runner may therefore clear or reuse that buffer while the
/// response remains retained across Controller-to-Host backpressure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnknownCommandCompleteEvent {
    bytes: [u8; UNKNOWN_COMMAND_COMPLETE_EVENT_CAPACITY],
    opcode: Opcode,
    status: Status,
}

impl UnknownCommandCompleteEvent {
    pub(crate) fn new(opcode: Opcode) -> Self {
        let status = HciError::UNKNOWN_CMD.to_status();
        let opcode_bytes = opcode.to_raw().to_le_bytes();
        Self {
            bytes: [
                0x0e,
                0x04,
                0x01,
                opcode_bytes[0],
                opcode_bytes[1],
                status.into_inner(),
            ],
            opcode,
            status,
        }
    }

    /// Complete HCI Event body without an H4 packet indicator.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Opcode rejected by the closed Controller command table.
    pub const fn opcode(&self) -> Opcode {
        self.opcode
    }

    /// Required `Unknown HCI Command` status.
    pub const fn status(&self) -> Status {
        self.status
    }
}

impl HciControllerResponse for UnknownCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// One complete Controller packet ready for publication toward the Host.
///
/// The trait is intentionally independent of command dispatch. A hardware
/// runner may retain an accepted command through arbitrary affine radio states,
/// build its response only at the proven completion boundary, and retry this
/// immutable packet across bounded output backpressure.
pub trait HciControllerResponse {
    /// HCI packet class published toward the Host.
    fn kind(&self) -> PacketKind;

    /// Complete packet body without an H4 indicator.
    fn as_bytes(&self) -> &[u8];
}

impl HciControllerResponse for BootstrapCommandCompleteEvent {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Closed Command Complete response set for the initial LE Controller.
///
/// Bootstrap, DTM and terminal Unknown Command responses have different
/// storage types but share the same publication boundary. Keeping the
/// distinction typed avoids copying a response into an unvalidated byte
/// scratch buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerCommandComplete {
    /// Pure software bootstrap command completion.
    Bootstrap(BootstrapCommandCompleteEvent),
    /// Completion supplied by the hardware-owned DTM session.
    Dtm(LeDtmCommandCompleteEvent),
    /// Completion for accepted or rejected advertising configuration.
    LegacyAdvertising(LeLegacyAdvertisingCommandCompleteEvent),
    /// Terminal response for an opcode outside the closed command table.
    UnknownCommand(UnknownCommandCompleteEvent),
}

impl HciControllerResponse for LeControllerCommandComplete {
    fn kind(&self) -> PacketKind {
        PacketKind::Event
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Bootstrap(response) => response.as_bytes(),
            Self::Dtm(response) => response.as_bytes(),
            Self::LegacyAdvertising(response) => response.as_bytes(),
            Self::UnknownCommand(response) => response.as_bytes(),
        }
    }
}

impl From<BootstrapCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: BootstrapCommandCompleteEvent) -> Self {
        Self::Bootstrap(response)
    }
}

impl From<LeDtmCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeDtmCommandCompleteEvent) -> Self {
        Self::Dtm(response)
    }
}

impl From<LeLegacyAdvertisingCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: LeLegacyAdvertisingCommandCompleteEvent) -> Self {
        Self::LegacyAdvertising(response)
    }
}

impl From<UnknownCommandCompleteEvent> for LeControllerCommandComplete {
    fn from(response: UnknownCommandCompleteEvent) -> Self {
        Self::UnknownCommand(response)
    }
}

#[cfg(test)]
mod tests {
    use bt_hci::{
        ControllerToHostPacket, FromHciBytes,
        cmd::{Opcode, OpcodeGroup},
        event::{CommandComplete, CommandCompleteWithStatus},
        param::Error as HciError,
        transport::Transport,
    };
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    use crate::{HciControllerResponse, InProcessHciChannel};

    use super::UnknownCommandCompleteEvent;

    #[test]
    fn unknown_command_response_roundtrips_through_the_real_hci_boundary() {
        let opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 7);
        let response = UnknownCommandCompleteEvent::new(opcode);
        let mut channel = InProcessHciChannel::<NoopRawMutex, 1, 1, 16>::new();
        let (host, controller) = channel.split();

        controller
            .try_publish(response.kind(), response.as_bytes())
            .expect("the owned completion fits the empty Controller queue");

        let mut packet = [0; 16];
        let ControllerToHostPacket::Event(event) =
            block_on(host.read(&mut packet)).expect("the Host receives the retained completion")
        else {
            panic!("Unknown Command completion changed packet kind");
        };
        let complete = CommandComplete::from_hci_bytes_complete(event.data)
            .expect("the response is a complete Command Complete event");
        let complete: CommandCompleteWithStatus<'_> = complete
            .try_into()
            .expect("the response retains its status return parameter");

        assert_eq!(complete.cmd_opcode, opcode);
        assert_eq!(complete.status, HciError::UNKNOWN_CMD.to_status());
        assert!(complete.return_param_bytes.is_empty());
    }
}
