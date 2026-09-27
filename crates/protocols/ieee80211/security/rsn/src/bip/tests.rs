use super::*;

use std::vec::Vec;

/// A group-addressed Deauthentication with Retry and Power Management set,
/// reason 7, protected under IGTK 4 at IPN 9. The MIC was computed
/// independently with Python's cryptography CMAC.
fn deauthentication(mic: [u8; 8], ipn: u8) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0xc0, 0x18, 0, 0]);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[0x10, 0x00]);
    frame.extend_from_slice(&[7, 0]);
    frame.extend_from_slice(&[76, 16, 4, 0, ipn, 0, 0, 0, 0, 0]);
    frame.extend_from_slice(&mic);
    frame
}

const MIC: [u8; 8] = [0xeb, 0x79, 0x2a, 0x77, 0x23, 0x5b, 0xf1, 0x7a];

fn receiver(ipn: u8) -> BipReceiver {
    BipReceiver::new(&RsnIgtk::new(4, [ipn, 0, 0, 0, 0, 0], [0x77; 16]).unwrap())
}

#[test]
fn a_frame_under_the_igtk_verifies_once() {
    let mut bip = receiver(8);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    // The same IPN again is a replay.
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
}

#[test]
fn a_changed_frame_or_mic_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[24] = 8;
    assert_eq!(bip.verify(&frame), Err(BipError::InvalidMic));
    let mut mic = MIC;
    mic[0] ^= 1;
    assert_eq!(
        bip.verify(&deauthentication(mic, 9)),
        Err(BipError::InvalidMic)
    );
    // A failed frame advances nothing.
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
}

#[test]
fn an_ipn_not_above_the_kde_ipn_is_a_replay() {
    let mut bip = receiver(9);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
}

#[test]
fn a_frame_without_its_mic_element_or_another_key_fails() {
    let mut bip = receiver(8);
    let mut frame = deauthentication(MIC, 9);
    frame[26] = 75;
    assert_eq!(bip.verify(&frame), Err(BipError::MissingManagementMic));
    let mut frame = deauthentication(MIC, 9);
    frame[28] = 5;
    assert_eq!(bip.verify(&frame), Err(BipError::UnknownKeyId));
    assert_eq!(bip.verify(&[0; 30]), Err(BipError::Truncated));
}
