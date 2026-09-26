use oer_ieee802154::{AutoPendingMode, FrameAddress, TxStatus};

use super::{
    TransmitFailure, extended_address, extended_pending_address, pending_mode, psdu_mac,
    scan_micros, short_pending_address, transmit_failure, write_psdu,
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
