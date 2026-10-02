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

#[test]
fn a_repeated_igtk_keeps_its_replay_state_and_a_new_one_resets_it() {
    let mut bip = receiver(8);
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Ok(()));
    // The same IGTK redelivered with an older IPN.
    bip.rekey(&RsnIgtk::new(4, [2, 0, 0, 0, 0, 0], [0x77; 16]).unwrap());
    assert_eq!(bip.verify(&deauthentication(MIC, 9)), Err(BipError::Replay));
    // Another IGTK starts from its own IPN.
    bip.rekey(&RsnIgtk::new(5, [2, 0, 0, 0, 0, 0], [0x11; 16]).unwrap());
    assert_eq!(bip.key_id(), 5);
}

#[test]
fn a_transmitter_reproduces_the_independent_mic_and_advances_its_ipn() {
    let igtk = RsnIgtk::new(4, [8, 0, 0, 0, 0, 0], [0x77; 16]).unwrap();
    let mut transmitter = BipTransmitter::new(&igtk);
    let expected = deauthentication(MIC, 9);
    let mut frame = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    frame[..26].copy_from_slice(&expected[..26]);
    assert_eq!(transmitter.protect(&mut frame, 26), Some(expected.len()));
    assert_eq!(frame[..], expected[..]);

    let mut receiver = BipReceiver::new(&igtk);
    assert_eq!(receiver.verify(&frame), Ok(()));
    let mut next = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    next[..26].copy_from_slice(&expected[..26]);
    transmitter.protect(&mut next, 26).unwrap();
    assert_eq!(next[30], 10);
    assert_eq!(receiver.verify(&next), Ok(()));
    assert_eq!(transmitter.protect(&mut next[..30], 26), None);
}

fn outgoing_frame() -> [u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN] {
    let mut frame = [0; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    frame[..26].copy_from_slice(&deauthentication(MIC, 9)[..26]);
    frame
}

#[test]
fn the_last_ipn_is_usable_but_exhaustion_never_wraps_or_changes_storage() {
    let mut ipn = [0xff; RSN_IPN_LEN];
    ipn[0] -= 1;
    let key = RsnIgtk::new(4, ipn, [0x77; RSN_IGTK_LEN]).unwrap();
    let mut transmitter = BipTransmitter::new(&key);
    let mut receiver = BipReceiver::new(&key);
    let mut last = outgoing_frame();
    assert_eq!(transmitter.try_protect(&mut last, 26), Ok(last.len()));
    assert_eq!(receiver.verify(&last), Ok(()));

    let untouched = outgoing_frame();
    for _ in 0..2 {
        let mut rejected = untouched;
        assert_eq!(
            transmitter.try_protect(&mut rejected, 26),
            Err(BipTransmitError::PacketNumberExhausted)
        );
        assert_eq!(rejected, untouched);
    }
    // Reinstalling the same IGTK cannot make its counter available again.
    transmitter.rekey(&key);
    let mut rejected = untouched;
    assert_eq!(transmitter.protect(&mut rejected, 26), None);
    assert_eq!(rejected, untouched);
}

#[test]
fn a_transmitter_reinstallation_keeps_the_frontier_and_a_fresh_key_can_rotate() {
    let original = RsnIgtk::new(4, [8, 0, 0, 0, 0, 0], [0x77; RSN_IGTK_LEN]).unwrap();
    let mut transmitter = BipTransmitter::new(&original);
    let mut receiver = BipReceiver::new(&original);
    let mut first = outgoing_frame();
    transmitter.try_protect(&mut first, 26).unwrap();
    receiver.verify(&first).unwrap();

    transmitter.rekey(&original);
    let mut next = outgoing_frame();
    transmitter.try_protect(&mut next, 26).unwrap();
    receiver.verify(&next).unwrap();

    // An install-time frontier ahead of local transmissions also wins.
    let advanced = RsnIgtk::new(4, [20, 0, 0, 0, 0, 0], [0x77; RSN_IGTK_LEN]).unwrap();
    transmitter.rekey(&advanced);
    receiver.rekey(&advanced);
    let mut ahead = outgoing_frame();
    transmitter.try_protect(&mut ahead, 26).unwrap();
    receiver.verify(&ahead).unwrap();

    let fresh = RsnIgtk::new(5, [0; RSN_IPN_LEN], [0x11; RSN_IGTK_LEN]).unwrap();
    transmitter.rekey(&fresh);
    receiver.rekey(&fresh);
    let mut rotated = outgoing_frame();
    transmitter.try_protect(&mut rotated, 26).unwrap();
    receiver.verify(&rotated).unwrap();
}

#[test]
fn storage_errors_do_not_consume_an_ipn_and_a_maximum_kde_starts_exhausted() {
    let key = RsnIgtk::new(4, [8, 0, 0, 0, 0, 0], [0x77; RSN_IGTK_LEN]).unwrap();
    let mut transmitter = BipTransmitter::new(&key);
    let mut frame = outgoing_frame();
    let original = frame;
    assert_eq!(
        transmitter.try_protect(&mut frame, usize::MAX),
        Err(BipTransmitError::InvalidLength)
    );
    assert_eq!(
        transmitter.try_protect(&mut frame, MANAGEMENT_HEADER_LEN - 1),
        Err(BipTransmitError::InvalidLength)
    );
    assert_eq!(
        transmitter.try_protect(&mut frame[..26], 26),
        Err(BipTransmitError::OutputTooSmall)
    );
    assert_eq!(frame, original);
    transmitter.try_protect(&mut frame, 26).unwrap();
    assert_eq!(&frame[..], &deauthentication(MIC, 9));

    let exhausted = RsnIgtk::new(4, [0xff; RSN_IPN_LEN], [0x11; RSN_IGTK_LEN]).unwrap();
    let mut transmitter = BipTransmitter::new(&exhausted);
    assert_eq!(
        transmitter.try_protect(&mut frame, 26),
        Err(BipTransmitError::PacketNumberExhausted)
    );
}
