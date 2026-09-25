use super::*;
use oer_bluetooth_ll::security::LeLongTermKey;
use std::vec::Vec;

// Bluetooth Core Vol 6, Part C, Section 1: encryption start and ACL sample.
const LTK: [u8; 16] = [
    0xbf, 0x01, 0xfb, 0x9d, 0x4e, 0xf3, 0xbc, 0x36, 0xd8, 0x74, 0xf5, 0x39, 0x41, 0x38, 0x68, 0x4c,
];
const REQUEST: [u8; 25] = [
    0x03, 23, 0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13, 0x02, 0xf1,
    0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
];
const START: [u8; 7] = [0x0f, 5, 0x9f, 0xcd, 0xa7, 0xf4, 0x48];
const DATA: [u8; 33] = [
    0x0e, 31, 0x7a, 0x70, 0xd6, 0x64, 0x15, 0x22, 0x6d, 0xf2, 0x6b, 0x17, 0x83, 0x9a, 0x06, 0x04,
    0x05, 0x59, 0x6b, 0xd6, 0x56, 0x4f, 0x79, 0x6b, 0x5b, 0x9c, 0xe6, 0xff, 0x32, 0xf7, 0x5a, 0x6d,
    0x33,
];

fn receive(procedure: &mut Procedure, pdu: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = [0xa5; 257];
    decode(procedure, pdu, &mut decoded, || {
        assert_eq!(pdu, REQUEST);
        LePeripheralEncryptionRandom::new(
            [0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02],
            [0xbe, 0xba, 0xaf, 0xde],
        )
    })
    .map(<[u8]>::to_vec)
}

fn empty(procedure: &mut Procedure) {
    for header in [0x01, 0x05, 0x09, 0x1d] {
        assert_eq!(receive(procedure, &[header, 0]), None);
        assert_eq!(procedure.termination_reason(), None);
    }
}

fn waiting_for_key() -> Procedure {
    let mut procedure = Procedure::new();
    assert_eq!(receive(&mut procedure, &REQUEST), None);
    let response = procedure.pending_response();
    empty(&mut procedure);
    assert_eq!(procedure.pending_response(), response);
    procedure.response_enqueued().unwrap();
    empty(&mut procedure);
    assert_eq!(procedure.long_term_key_request(), None);
    procedure.observe_transmission_completion(true);
    empty(&mut procedure);
    assert!(procedure.long_term_key_request().is_some());
    procedure
}

fn established() -> Procedure {
    let mut procedure = waiting_for_key();
    assert!(procedure.take_long_term_key_request().is_some());
    empty(&mut procedure);
    assert_eq!(procedure.take_long_term_key_request(), None);
    assert!(
        procedure
            .provide_long_term_key(LeLongTermKey::new(LTK))
            .is_ok()
    );
    empty(&mut procedure);
    assert_eq!(procedure.pending_response().unwrap().as_bytes(), &[0x05]);
    procedure.response_enqueued().unwrap();
    empty(&mut procedure);
    assert_eq!(receive(&mut procedure, &START), None);
    let response = procedure.pending_response().unwrap();
    assert_eq!(response.as_bytes(), &[0xa3, 0x4c, 0x13, 0xa4, 0x15]);
    empty(&mut procedure);
    assert_eq!(procedure.pending_response(), Some(response));
    procedure.response_enqueued().unwrap();
    assert!(procedure.take_encryption_enabled());
    procedure
}

#[test]
fn empties_preserve_handshake_key_request_and_counter_one_data() {
    let mut procedure = established();
    empty(&mut procedure);
    assert!(!procedure.take_encryption_enabled());
    assert_eq!(
        receive(&mut procedure, &DATA).unwrap(),
        b"\x0e\x1b\x17\x00cdefghijklmnopq1234567890"
    );
    empty(&mut procedure);
    let cipher = procedure.active_encryption().unwrap();
    assert_eq!(cipher.next_central_transmit_counter(), Some(2));
    assert_eq!(cipher.next_peripheral_transmit_counter(), Some(1));
    let mut response = [0; 31];
    response[..27].copy_from_slice(b"\x17\x0076543210ABCDEFGHIJKLMNOPQ");
    cipher.encrypt_new_packet(0x06, &mut response, 27).unwrap();
    assert_eq!(
        response,
        [
            0xf3, 0x88, 0x81, 0xe7, 0xbd, 0x94, 0xc9, 0xc3, 0x69, 0xb9, 0xa6, 0x68, 0x46, 0xdd,
            0x47, 0x86, 0xaa, 0x8c, 0x39, 0xce, 0x54, 0x0d, 0x0d, 0xae, 0x3a, 0xdc, 0xdf, 0x89,
            0xb9, 0x60, 0x88,
        ]
    );
}

#[test]
fn empty_ack_cannot_bypass_missing_key_rejection() {
    let mut procedure = waiting_for_key();
    procedure.reject_long_term_key().unwrap();
    empty(&mut procedure);
    assert_eq!(
        procedure.pending_response().unwrap().as_bytes(),
        &[0x0d, 0x06]
    );
    procedure.response_enqueued().unwrap();
    empty(&mut procedure);
    assert!(!procedure.is_idle());
    procedure.observe_transmission_completion(true);
    assert!(procedure.is_idle());
}

#[test]
fn acknowledged_key_rejection_admits_plaintext_version_then_peer_termination() {
    use oer_bluetooth_ll::control::{
        LePeripheralControl, LePeripheralControlError, LeVersionInformation,
    };
    let mut procedure = waiting_for_key();
    procedure.reject_long_term_key().unwrap();
    procedure.response_enqueued().unwrap();
    procedure.observe_transmission_completion(true);
    let version = LeVersionInformation::new(0x0d, 0xffff, 1);
    let mut control = LePeripheralControl::new();
    // A new control PDU can carry the ACK which completed the reject.
    let peer_version = [3, 6, 0x0c, 0x0b, 2, 0, 3, 0];
    let plaintext = receive(&mut procedure, &peer_version).unwrap();
    control.receive(&plaintext, Some(version)).unwrap();
    assert_eq!(
        control.pending_response().unwrap().as_bytes(),
        &[0x0c, 0x0d, 0xff, 0xff, 1, 0]
    );
    control.response_enqueued();
    assert!(!procedure.blocks_unrelated_transmission());
    // Termination is valid even before our version response is acknowledged.
    let terminate = receive(&mut procedure, &[3, 2, 2, 0x13]).unwrap();
    assert_eq!(
        control.receive(&terminate, Some(version)),
        Err(LePeripheralControlError::PeerTermination { reason: 0x13 })
    );
    assert!(procedure.is_idle());
    assert!(!procedure.take_encryption_enabled());
}

#[test]
fn mic_failure_releases_no_plaintext_and_empty_ack_cannot_revive_session() {
    let mut procedure = established();
    let mut data = DATA;
    *data.last_mut().unwrap() ^= 1;
    let mut decoded = [0xa5; 257];
    assert!(decode(&mut procedure, &data, &mut decoded, || panic!("no new key")).is_none());
    assert_eq!(decoded, [0; 257]);
    assert_eq!(procedure.termination_reason(), Some(0x3d));
    assert_eq!(receive(&mut procedure, &[0x01, 0]), None);
    assert_eq!(receive(&mut procedure, &DATA), None);
    assert_eq!(procedure.termination_reason(), Some(0x3d));
    assert!(procedure.active_encryption().is_none());
    // A new connection must complete its own handshake and counter-zero start.
    let mut next_connection = established();
    assert!(receive(&mut next_connection, &DATA).is_some());
}

#[test]
fn empty_exception_never_admits_unencrypted_data_or_malformed_headers() {
    for packet in [&[0x02, 0][..], &[0x03, 0], &[0x01, 1, 0], &[0x01, 1], &[]] {
        let mut procedure = waiting_for_key();
        assert_eq!(receive(&mut procedure, packet), None);
        assert_eq!(procedure.termination_reason(), Some(0x3d));
    }
}

#[test]
fn wrong_host_ltk_fails_the_real_start_decoder_before_any_application_delivery() {
    let mut procedure = waiting_for_key();
    let mut wrong = LTK;
    wrong[0] ^= 1;
    assert!(
        procedure
            .provide_long_term_key(LeLongTermKey::new(wrong))
            .is_ok()
    );
    procedure.response_enqueued().unwrap();
    let mut decoded = [0xa5; 257];
    assert!(
        decode(&mut procedure, &START, &mut decoded, || panic!(
            "no new entropy"
        ))
        .is_none()
    );
    assert_eq!(decoded, [0; 257]);
    assert_eq!(procedure.termination_reason(), Some(0x3d));
    assert!(procedure.active_encryption().is_none());
    assert!(receive(&mut procedure, &DATA).is_none());
    assert!(receive(&mut established(), &DATA).is_some());
}

#[test]
fn ciphertext_matching_terminate_opcode_authenticates_and_keeps_acl_alive() {
    // Core sample key/diversifier with IVp 35 00 af de. These CCM packets were
    // independently generated with AESCCM (four-octet MIC), not the LL encoder.
    // The on-air 02 is encrypted START_ENC_RSP, not LL_TERMINATE_IND.
    let mut procedure = Procedure::new();
    let mut decoded = [0; 257];
    assert!(
        decode(&mut procedure, &REQUEST, &mut decoded, || {
            LePeripheralEncryptionRandom::new(
                [0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02],
                [0x35, 0x00, 0xaf, 0xde],
            )
        })
        .is_none()
    );
    procedure.response_enqueued().unwrap();
    procedure.observe_transmission_completion(true);
    assert!(
        procedure
            .provide_long_term_key(LeLongTermKey::new(LTK))
            .is_ok()
    );
    procedure.response_enqueued().unwrap();
    assert_eq!(
        receive(&mut procedure, &[3, 5, 2, 0xf2, 6, 0x6a, 0x44]),
        None
    );
    assert_eq!(procedure.termination_reason(), None);
    assert!(procedure.pending_response().unwrap().is_encrypted());
    procedure.response_enqueued().unwrap();
    assert!(procedure.take_encryption_enabled());
    assert_eq!(
        receive(
            &mut procedure,
            &[2, 9, 0xf5, 0x1a, 0xd4, 0xec, 0x9a, 0x65, 0xf9, 0xa3, 0xde]
        ),
        Some(Vec::from([2, 5, 1, 0, 4, 0, b'x'])),
    );
    assert_eq!(procedure.termination_reason(), None);
    assert_eq!(
        procedure
            .active_encryption()
            .unwrap()
            .next_central_transmit_counter(),
        Some(2)
    );
}
