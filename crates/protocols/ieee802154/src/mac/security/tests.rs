//! Expectations follow `otPlatRadioTransmit` of ESP-IDF's OpenThread port
//! and OpenThread's `Frame::SetFrameCounter` / `Frame::SetKeyId`.

use std::vec::Vec;

use super::MacKeys;
use crate::KeyIdMode;

/// `[PHR, MAC...]` of a 2006 data frame secured at ENC-MIC-32 in key
/// identifier mode 1: security header at image offset 10.
fn secured_mode_1() -> Vec<u8> {
    let mac = [
        0x69, 0x98, 0x07, 0x34, 0x12, 0x02, 0x00, 0x01, 0x00, // header
        0x0d, 0, 0, 0, 0, 0, // ENC-MIC-32 mode 1, counter, key index
        0xaa, 0, 0, 0, 0,
    ];
    let mut image = std::vec![mac.len() as u8 + 2];
    image.extend_from_slice(&mac);
    image
}

/// A first transmission takes a new frame counter and the current key
/// index and key; a retransmission keeps the frame's counter.
#[test]
fn first_transmissions_take_a_counter_and_retransmissions_keep_it() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 40);
    let mut image = secured_mode_1();

    let first = keys.transmit_security(false);
    assert_eq!(
        (first.frame_counter, first.key_id, first.key),
        (Some(40), 3, [2; 16])
    );
    assert_eq!(first.apply(&mut image), Some(KeyIdMode::Index));
    assert_eq!(image[11..16], [40, 0, 0, 0, 3]);

    // Another CCA attempt of the first transmission is a transmit of its own.
    let again = keys.transmit_security(false);
    again.apply(&mut image);
    assert_eq!(image[11..15], [41, 0, 0, 0]);

    let retry = keys.transmit_security(true);
    assert_eq!(retry.frame_counter, None);
    retry.apply(&mut image);
    assert_eq!(image[11..15], [41, 0, 0, 0]);
    assert_eq!(keys.frame_counter(), 42);
}

/// Unsecured frames and frames without room for their header are left.
#[test]
fn only_secured_frames_are_written() {
    let mut keys = MacKeys::new(3, [1; 16], [2; 16], [3; 16], 0);
    let security = keys.transmit_security(false);
    let mut plain = secured_mode_1();
    plain[1] &= !0x08;
    let before = plain.clone();
    assert_eq!(security.apply(&mut plain), None);
    assert_eq!(plain, before);

    let mut short = secured_mode_1();
    short.truncate(13);
    assert_eq!(security.apply(&mut short), None);

    let mut implicit = secured_mode_1();
    implicit[10] = 0x05;
    assert_eq!(security.apply(&mut implicit), Some(KeyIdMode::Implicit));
    assert_eq!(implicit[15], 0, "mode 0 carries no key index");
}
