use super::*;

use std::vec::Vec;

/// A protected SA Query Response with Retry set, sequence 0x123 fragment 5,
/// PN 0xa1 under the temporal key 00..0f. Encrypted independently with
/// Python's cryptography AESCCM.
const FRAME: &str =
    "d04800000200000000010200000000020200000000023512a10000200000000012714ee3c7f2391ea46ccabd";

fn frame() -> Vec<u8> {
    (0..FRAME.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&FRAME[index..index + 2], 16).unwrap())
        .collect()
}

fn receiver() -> ManagementCcmpReceiver {
    ManagementCcmpReceiver::new(core::array::from_fn(|index| index as u8))
}

#[test]
fn a_protected_management_frame_opens_once() {
    let mut ccmp = receiver();
    let mut first = frame();
    assert_eq!(ccmp.open(&mut first), Ok(&[8, 1, 0x12, 0x34][..]));
    let mut again = frame();
    assert_eq!(ccmp.open(&mut again), Err(ManagementCcmpError::Replay));
}

#[test]
fn a_changed_header_or_body_fails_without_advancing_the_replay_state() {
    let mut ccmp = receiver();
    let mut changed_address = frame();
    changed_address[9] ^= 1;
    assert_eq!(
        ccmp.open(&mut changed_address),
        Err(ManagementCcmpError::InvalidMic)
    );
    let mut changed_body = frame();
    changed_body[32] ^= 1;
    assert_eq!(
        ccmp.open(&mut changed_body),
        Err(ManagementCcmpError::InvalidMic)
    );
    // Retry, Power Management and More Data are outside the MIC.
    let mut retry_cleared = frame();
    retry_cleared[1] &= !0x08;
    assert!(ccmp.open(&mut retry_cleared).is_ok());
}

#[test]
fn an_unprotected_or_short_frame_is_refused() {
    let mut ccmp = receiver();
    let mut unprotected = frame();
    unprotected[1] &= !0x40;
    assert_eq!(
        ccmp.open(&mut unprotected),
        Err(ManagementCcmpError::NotProtected)
    );
    let mut no_ext_iv = frame();
    no_ext_iv[27] = 0;
    assert_eq!(
        ccmp.open(&mut no_ext_iv),
        Err(ManagementCcmpError::MissingExtendedIv)
    );
    assert_eq!(ccmp.open(&mut [0; 39]), Err(ManagementCcmpError::Truncated));
}
