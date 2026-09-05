use super::*;

const CONFIG: TxBlockAckConfig = TxBlockAckConfig {
    tid: 7,
    window: 32,
    timeout_tu: 0,
    negotiation_timeout_us: 100_000,
    amsdu: true,
};

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

#[test]
fn request_encoding_is_exact_and_bounded() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x1abc, 50).unwrap();
    assert_eq!(request.starting_sequence, 0x0abc);
    assert_eq!(request.alarm.deadline_us, 100_050);
    assert_eq!(request.body, [3, 0, 1, 0x1f, 0x08, 0, 0, 0xc0, 0xab]);
}

#[test]
fn matching_response_commits_only_the_static_window() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x123, 0).unwrap();
    let response = [3, 1, request.dialog_token, 0, 0, 0x1f, 0x08, 0, 0];
    let agreement = OperationalTxBlockAck {
        tid: 7,
        window: 32,
        timeout_tu: 0,
        starting_sequence: 0x123,
        amsdu: true,
    };
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(agreement))
    );
    assert_eq!(session.operational(), Some(agreement));
    assert!(!session.on_alarm(request.alarm));
}

#[test]
fn matching_response_accepts_an_addba_extension_ie() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0x123, 0).unwrap();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        0x1f,
        0x08,
        0,
        0,
        159,
        1,
        0,
    ];
    assert!(matches!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Operational(_))
    ));
}

#[test]
fn stale_alarm_cannot_cancel_a_new_generation() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let stale = session.begin(1, 0).unwrap().alarm;
    let current = session.begin(2, 10).unwrap().alarm;
    assert!(!session.on_alarm(stale));
    assert!(session.on_alarm(current));
    assert_eq!(session.operational(), None);
}

#[test]
fn response_cannot_expand_the_static_capacity() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0, 0).unwrap();
    let parameters = encode_ba_parameters(7, 64, false).to_le_bytes();
    let response = [
        3,
        1,
        request.dialog_token,
        0,
        0,
        parameters[0],
        parameters[1],
        0,
        0,
    ];
    assert_eq!(
        session.on_response(&response),
        Err(TxBlockAckError::WindowExceedsCapacity(64))
    );
}

#[test]
fn rejected_response_returns_to_idle_without_a_timer_retry() {
    let mut session = TxBlockAckSession::new(CONFIG).unwrap();
    let request = session.begin(0, 0).unwrap();
    let response = [3, 1, request.dialog_token, 37, 0, 0, 0, 0, 0];
    assert_eq!(
        session.on_response(&response),
        Ok(TxBlockAckResponse::Rejected(37))
    );
    assert_eq!(session.operational(), None);
    assert!(!session.on_alarm(request.alarm));
}
