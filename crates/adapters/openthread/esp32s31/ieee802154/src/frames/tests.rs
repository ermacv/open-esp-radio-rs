use oer_ieee802154::{
    AppliedSecurity, AutoPendingMode, FrameAddress, SentAcknowledgement, TxSecurity, TxStatus,
};

use super::{
    PORT_INITIAL_KEYS, TransmitFailure, extended_address, extended_pending_address, pending_mode,
    psdu_mac, scan_micros, sent_ack_security, set_frame_counter, set_mac_keys,
    short_pending_address, transmit_failure, tx_security, write_applied_security, write_psdu,
};

#[test]
fn psdus_count_the_fcs() {
    assert_eq!(
        psdu_mac(&[0x41, 0x88, 0x01, 0xaa, 0xbb]),
        Some(&[0x41, 0x88, 0x01][..])
    );
    assert_eq!(psdu_mac(&[0xaa, 0xbb]), None);
    assert_eq!(psdu_mac(&[]), None);

    let mut psdu = [0xff; 8];
    assert_eq!(write_psdu(&[1, 2, 3], &mut psdu), Some(5));
    assert_eq!(psdu[..5], [1, 2, 3, 0, 0]);
    assert_eq!(write_psdu(&[1, 2, 3], &mut [0; 4]), None);
}

/// OpenThread's glue packs `otExtAddress.m8` big-endian; over the air the
/// address goes least significant byte first.
#[test]
fn extended_addresses_go_over_the_air_reversed() {
    let address = u64::from_be_bytes([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    assert_eq!(
        extended_address(address),
        [0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]
    );
    assert_eq!(
        extended_pending_address(address),
        FrameAddress::Extended([0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11])
    );
    assert_eq!(
        short_pending_address(0x1234),
        FrameAddress::Short([0x34, 0x12])
    );
}

#[test]
fn source_matching_selects_the_pending_mode() {
    assert_eq!(pending_mode(false), AutoPendingMode::Disable);
    assert_eq!(pending_mode(true), AutoPendingMode::Enhanced);
}

/// ESP-IDF's OpenThread port reports a busy channel, an abort and a
/// coexistence rejection as channel-access failures.
#[test]
fn transmit_statuses_map_to_openthread_failures() {
    assert_eq!(transmit_failure(TxStatus::Success), None);
    for status in [
        TxStatus::ChannelBusy,
        TxStatus::Aborted,
        TxStatus::CoexistenceRejected,
    ] {
        assert_eq!(
            transmit_failure(status),
            Some(TransmitFailure::ChannelAccess)
        );
    }
    assert_eq!(
        transmit_failure(TxStatus::NoAcknowledgement),
        Some(TransmitFailure::NoAcknowledgement)
    );
    assert_eq!(
        transmit_failure(TxStatus::InvalidAcknowledgement),
        Some(TransmitFailure::InvalidAcknowledgement)
    );
    assert_eq!(
        transmit_failure(TxStatus::SecurityFailure),
        Some(TransmitFailure::Other)
    );
    assert_eq!(scan_micros(20), 20_000);
}

/// The port's transmit information decides who secures the frame.
#[test]
fn transmit_information_selects_the_security() {
    assert_eq!(tx_security(false, false), TxSecurity::Radio);
    assert_eq!(tx_security(true, false), TxSecurity::Retransmission);
    assert_eq!(tx_security(false, true), TxSecurity::Processed);
    assert_eq!(tx_security(true, true), TxSecurity::Processed);
}

/// The counter and key index the radio wrote land in OpenThread's PSDU at
/// the auxiliary security header, leaving payload and FCS as given.
#[test]
fn applied_security_is_written_into_the_psdu() {
    // 2006 data frame, ENC-MIC-32 in key identifier mode 1, one payload
    // byte, MIC and FCS.
    let mut psdu = [
        0x69, 0x98, 0x07, 0x34, 0x12, 0x02, 0x00, 0x01, 0x00, 0x0d, 0, 0, 0, 0, 0, 0xaa, 1, 2, 3,
        4, 0xf1, 0xf2,
    ];
    let applied = AppliedSecurity {
        frame_counter: 0x0403_0201,
        key_id: Some(7),
    };
    assert!(write_applied_security(applied, &mut psdu));
    assert_eq!(psdu[10..15], [1, 2, 3, 4, 7]);
    assert_eq!(psdu[15..], [0xaa, 1, 2, 3, 4, 0xf1, 0xf2]);

    let mut plain = [
        0x41, 0x98, 0x07, 0x34, 0x12, 0xff, 0xff, 0x02, 0x00, 0xaa, 0, 0,
    ];
    let before = plain;
    assert!(!write_applied_security(applied, &mut plain));
    assert_eq!(plain, before);
}

/// Keys and counter updates start from the port's zeroed state and keep
/// each other.
#[test]
fn keys_and_counter_updates_keep_each_other() {
    let mut keys = None;
    set_frame_counter(&mut keys, 40, true);
    set_mac_keys(&mut keys, 2, [1; 16], [2; 16], [3; 16]);
    let mut installed = keys.unwrap();
    assert_eq!(installed.frame_counter(), 40);
    let security = installed.transmit_security(false);
    assert_eq!((security.key_id, security.key), (Some(2), [2; 16]));

    set_frame_counter(&mut keys, 30, true);
    assert_eq!(keys.unwrap().frame_counter(), 40);
    set_frame_counter(&mut keys, 30, false);
    assert_eq!(keys.unwrap().frame_counter(), 30);
    assert_eq!(PORT_INITIAL_KEYS.frame_counter(), 0);
}

/// Only a secured enhanced ACK of key identifier mode 1 reports its fields.
#[test]
fn sent_ack_security_needs_a_key_index() {
    let sent = |key_id| SentAcknowledgement {
        frame_pending: true,
        security: Some(AppliedSecurity {
            frame_counter: 9,
            key_id,
        }),
    };
    assert_eq!(sent_ack_security(sent(Some(3))), Some((9, 3)));
    assert_eq!(sent_ack_security(sent(None)), None);
    assert_eq!(sent_ack_security(SentAcknowledgement::NONE), None);
}
