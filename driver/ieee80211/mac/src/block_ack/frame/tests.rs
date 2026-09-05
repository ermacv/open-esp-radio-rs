use super::*;

#[test]
fn parses_all_block_ack_action_bodies() {
    assert_eq!(
        parse_block_ack_action(&[3, 0, 7, 0x87, 0x07, 0, 0, 0x30, 0x12]),
        Some(BlockAckAction::AddbaRequest {
            dialog_token: 7,
            tid: 1,
            immediate: true,
            amsdu: true,
            window: 30,
            timeout_tu: 0,
            starting_sequence: 0x123,
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 1, 7, 0, 0, 0x86, 0x07, 5, 0]),
        Some(BlockAckAction::AddbaResponse {
            dialog_token: 7,
            status: 0,
            tid: 1,
            immediate: true,
            amsdu: false,
            window: 30,
            timeout_tu: 5,
        })
    );
    assert_eq!(
        parse_block_ack_action(&[3, 2, 0, 0x58, 39, 0]),
        Some(BlockAckAction::Delba {
            tid: 5,
            initiator: true,
            reason: 39,
        })
    );
    assert_eq!(parse_block_ack_action(&[4, 0, 0]), None);
}
