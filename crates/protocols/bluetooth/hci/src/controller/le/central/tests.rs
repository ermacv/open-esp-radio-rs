use bt_hci::cmd::{Opcode, OpcodeGroup};

use super::*;

fn decode(ocf: u16, parameters: &[u8]) -> Result<LeCentralCommand, LeCentralDecodeError> {
    LeCentralCommand::decode(HciCommandPacket::new(
        Opcode::new(OpcodeGroup::LE, ocf),
        parameters,
    ))
}

fn status(result: Result<LeCentralCommand, LeCentralDecodeError>) -> u8 {
    match result {
        Err(LeCentralDecodeError::Malformed(response)) => match response.as_bytes()[0] {
            // Command Status carries its status first, Command Complete last.
            0x0f => response.as_bytes()[2],
            _ => response.as_bytes()[5],
        },
        other => panic!("not a refused command: {other:?}"),
    }
}

/// Scan 60/30 ms, peer public 11:22:33:44:55:66, interval 30–50 ms,
/// latency 0, timeout 2 s, event length 0.
fn parameters() -> [u8; 25] {
    let mut p = [0; 25];
    p[0..2].copy_from_slice(&96u16.to_le_bytes());
    p[2..4].copy_from_slice(&48u16.to_le_bytes());
    p[6..12].copy_from_slice(&[0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
    p[13..15].copy_from_slice(&24u16.to_le_bytes());
    p[15..17].copy_from_slice(&40u16.to_le_bytes());
    p[19..21].copy_from_slice(&200u16.to_le_bytes());
    p
}

#[test]
fn create_connection_decodes_its_peer_and_timing() {
    assert_eq!(
        decode(0x0d, &parameters()),
        Ok(LeCentralCommand::CreateConnection(
            LeCreateConnectionParameters {
                scan_interval_units: 96,
                scan_window_units: 48,
                peer: LeInitiatorPeer::Device {
                    random: false,
                    address: [0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
                },
                own_random: false,
                interval_min_units: 24,
                interval_max_units: 40,
                max_latency: 0,
                supervision_timeout_units: 200,
                min_event_length_units: 0,
                max_event_length_units: 0,
            }
        ))
    );
    let mut accept_list = parameters();
    accept_list[4] = 1;
    // The peer address is ignored with the filter accept list.
    accept_list[5] = 0x7f;
    let Ok(LeCentralCommand::CreateConnection(p)) = decode(0x0d, &accept_list) else {
        panic!("an accept-list initiator");
    };
    assert_eq!(p.peer, LeInitiatorPeer::AcceptList);
    assert_eq!(
        decode(0x0e, &[]),
        Ok(LeCentralCommand::CreateConnectionCancel)
    );
    assert_eq!(decode(0x0f, &[]), Err(LeCentralDecodeError::Unsupported));
}

#[test]
fn out_of_range_or_private_parameters_are_refused() {
    let with = |at: usize, bytes: &[u8]| {
        let mut p = parameters();
        p[at..at + bytes.len()].copy_from_slice(bytes);
        p
    };
    // Scan window longer than the interval, interval bounds swapped,
    // latency above 499, timeout too short for the interval.
    for p in [
        with(2, &100u16.to_le_bytes()),
        with(13, &50u16.to_le_bytes()),
        with(17, &500u16.to_le_bytes()),
        with(19, &10u16.to_le_bytes()),
    ] {
        assert_eq!(status(decode(0x0d, &p)), 0x12);
    }
    // Resolvable address types need a resolving list.
    assert_eq!(status(decode(0x0d, &with(5, &[2]))), 0x11);
    assert_eq!(status(decode(0x0d, &with(12, &[3]))), 0x11);
    assert_eq!(status(decode(0x0d, &with(4, &[2]))), 0x12);
    assert_eq!(status(decode(0x0d, &parameters()[..24])), 0x12);
    assert_eq!(status(decode(0x0e, &[0])), 0x12);
}

#[test]
fn create_connection_answers_with_status_and_cancel_with_complete() {
    assert_eq!(
        LeCentralCommandResponse::create_connection(Status::SUCCESS).as_bytes(),
        [0x0f, 4, 0, 1, 0x0d, 0x20]
    );
    assert_eq!(
        LeCentralCommandResponse::create_connection_cancel(Status::SUCCESS).as_bytes(),
        [0x0e, 4, 1, 0x0e, 0x20, 0]
    );
}
