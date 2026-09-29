use std::vec::Vec;

use bt_hci::cmd::{Opcode, OpcodeGroup};

use super::*;

fn decode(ocf: u16, parameters: &[u8]) -> Result<LePhyCommand, LePhyDecodeError> {
    LePhyCommand::decode(HciCommandPacket::new(
        Opcode::new(OpcodeGroup::LE, ocf),
        parameters,
    ))
}

fn refusal(result: Result<LePhyCommand, LePhyDecodeError>) -> Vec<u8> {
    match result {
        Err(LePhyDecodeError::Malformed(response)) => response.as_bytes().to_vec(),
        other => panic!("not a refused command: {other:?}"),
    }
}

#[test]
fn phy_commands_decode_their_connection_and_preferences() {
    assert_eq!(
        decode(0x30, &[0x01, 0x00]),
        Ok(LePhyCommand::Read(ConnHandle::new(1)))
    );
    assert_eq!(
        decode(0x31, &[0, 2, 1]),
        Ok(LePhyCommand::SetDefault(LePhyMasks {
            transmit: 2,
            receive: 1,
        }))
    );
    // ALL_PHYS: no preference in either direction, whatever the masks say.
    assert_eq!(
        decode(0x32, &[0x02, 0x00, 3, 0, 0, 0, 0]),
        Ok(LePhyCommand::Set(
            ConnHandle::new(2),
            LePhyMasks {
                transmit: 3,
                receive: 3,
            }
        ))
    );
    assert_eq!(decode(0x33, &[]), Err(LePhyDecodeError::Unsupported));
}

#[test]
fn coded_reserved_and_empty_preferences_are_refused_as_the_vendor_does() {
    // Reserved bit and LE Coded: Unsupported Feature or Parameter Value.
    for masks in [[0, 8, 1], [0, 1, 4]] {
        assert_eq!(refusal(decode(0x31, &masks))[5], 0x11);
    }
    // An empty direction without its no-preference bit.
    assert_eq!(refusal(decode(0x31, &[2, 0, 1]))[5], 0x12);
    assert_eq!(refusal(decode(0x31, &[1, 1, 0]))[5], 0x12);
    // Set PHY answers with Command Status, Read PHY with a full return.
    let status = refusal(decode(0x32, &[0, 0, 0, 1]));
    assert_eq!(status[..3], [EventKind::CommandStatus.0, 4, 0x12]);
    let complete = refusal(decode(0x30, &[0]));
    assert_eq!(
        complete[..6],
        [EventKind::CommandComplete.0, 8, 1, 0x30, 0x20, 0x12]
    );
    assert_eq!(complete.len(), 10);
}

#[test]
fn the_update_event_carries_status_handle_and_both_phys() {
    assert_eq!(
        LePhyUpdateCompleteEvent::new(Status::SUCCESS, ConnHandle::new(0x0102), 2, 1).as_bytes(),
        [EventKind::Le.0, 6, 0x0c, 0, 0x02, 0x01, 2, 1]
    );
    assert_eq!(
        LePhyCommandResponse::read(Status::SUCCESS, ConnHandle::new(0), 2, 2).as_bytes(),
        [
            EventKind::CommandComplete.0,
            8,
            1,
            0x30,
            0x20,
            0,
            0,
            0,
            2,
            2
        ]
    );
}
