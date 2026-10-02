use super::*;

use oer_ieee80211_mac::ccmp::{CCMP_PACKET_NUMBER_MAX, CcmpPacketNumber, CcmpPacketNumberStep};
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

fn transmitter() -> ManagementCcmpTransmitter {
    ManagementCcmpTransmitter::new(core::array::from_fn(|index| index as u8))
}

fn outgoing(body: &[u8]) -> Vec<u8> {
    let mut output = frame()[..MANAGEMENT_HEADER_LEN].to_vec();
    output[1] &= !PROTECTED;
    output.extend_from_slice(body);
    output.resize(output.len() + CCMP_HEADER_LEN + MIC_LEN, 0);
    output
}

#[test]
fn transmission_reproduces_the_independent_aesccm_vector() {
    let mut numbers = CcmpTxPacketNumber::continuing_after(
        CcmpPacketNumberStep::ONE,
        CcmpPacketNumber::new(0xa0).unwrap(),
    );
    let mut output = outgoing(&[8, 1, 0x12, 0x34]);
    let expected = frame();
    assert_eq!(
        transmitter().protect(&mut numbers, &mut output, MANAGEMENT_HEADER_LEN + 4),
        Ok(expected.len())
    );
    assert_eq!(output, expected);
    assert_eq!(numbers.last().value(), 0xa1);
    assert_eq!(receiver().open(&mut output), Ok(&[8, 1, 0x12, 0x34][..]));
}

/// Three CCM payload blocks (the last partial) with a six-octet PN,
/// independently encrypted by Python cryptography's AESCCM using key 00..0f,
/// nonce 100200000000020123456789ab and AAD
/// d0400200000000010200000000020200000000020500.
const MULTIBLOCK_FRAME: &str = "d04800000200000000010200000000020200000000023512ab89002067452301f06f50b949ce6ebae5aa55022cdbe65d151f36a978b4a3302dbb5996b425ed2fe1885a74b8c1111eeb";

#[test]
fn multi_block_transmission_and_receive_match_the_independent_vector() {
    let body: Vec<u8> = (0..33).collect();
    let mut numbers = CcmpTxPacketNumber::continuing_after(
        CcmpPacketNumberStep::ONE,
        CcmpPacketNumber::new(0x0123_4567_89aa).unwrap(),
    );
    let expected: Vec<u8> = (0..MULTIBLOCK_FRAME.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&MULTIBLOCK_FRAME[index..index + 2], 16).unwrap())
        .collect();
    let mut output = outgoing(&body);
    assert_eq!(
        transmitter().protect(
            &mut numbers,
            &mut output,
            MANAGEMENT_HEADER_LEN + body.len()
        ),
        Ok(expected.len())
    );
    assert_eq!(output, expected);
    assert_eq!(receiver().open(&mut output), Ok(body.as_slice()));
}

#[test]
fn malformed_ccmp_headers_do_not_mutate_the_frame_or_consume_replay_state() {
    for (offset, value, error) in [
        (
            26,
            1,
            ManagementCcmpError::InvalidCcmpHeader(CcmpHeaderError::ReservedOctet),
        ),
        (
            27,
            0x21,
            ManagementCcmpError::InvalidCcmpHeader(CcmpHeaderError::ReservedKeyBits),
        ),
        (27, 0x60, ManagementCcmpError::UnexpectedKeyId),
    ] {
        let mut ccmp = receiver();
        let mut invalid = frame();
        invalid[offset] = value;
        let original = invalid.clone();
        assert_eq!(ccmp.open(&mut invalid), Err(error));
        assert_eq!(invalid, original);
        assert!(ccmp.open(&mut frame()).is_ok());
    }
}

#[test]
fn the_shared_allocator_survives_key_redelivery_and_other_transmissions() {
    let mut numbers = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE);
    let mut ccmp = receiver();
    let mut first = outgoing(&[8, 0, 1, 2]);
    transmitter()
        .protect(&mut numbers, &mut first, MANAGEMENT_HEADER_LEN + 4)
        .unwrap();
    ccmp.open(&mut first).unwrap();
    // Another MAC transmission consumes a PN from the same owner.
    numbers.next().unwrap();
    // Recreating a crypto key view does not recreate the PN owner.
    let mut next = outgoing(&[8, 0, 1, 2]);
    transmitter()
        .protect(&mut numbers, &mut next, MANAGEMENT_HEADER_LEN + 4)
        .unwrap();
    assert_eq!(numbers.last().value(), 3);
    assert_eq!(next[MANAGEMENT_HEADER_LEN], 3);
    ccmp.open(&mut next).unwrap();
}

#[test]
fn failed_transmissions_preserve_the_buffer_and_the_unique_packet_number_owner() {
    let tx = transmitter();
    let mut numbers = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE);
    let original = outgoing(&[8, 0, 1, 2]);
    for length in [0, MANAGEMENT_HEADER_LEN - 1, usize::MAX] {
        let mut output = original.clone();
        assert_eq!(
            tx.protect(&mut numbers, &mut output, length),
            Err(ManagementCcmpTransmitError::InvalidLength)
        );
        assert_eq!(output, original);
        assert_eq!(numbers.last(), CcmpPacketNumber::ZERO);
    }
    let mut short = original.clone();
    assert_eq!(
        tx.protect(
            &mut numbers,
            &mut short[..MANAGEMENT_HEADER_LEN + 4],
            MANAGEMENT_HEADER_LEN + 4
        ),
        Err(ManagementCcmpTransmitError::OutputTooSmall)
    );
    assert_eq!(short, original);
    assert_eq!(numbers.last(), CcmpPacketNumber::ZERO);

    let mut numbers = CcmpTxPacketNumber::continuing_after(
        CcmpPacketNumberStep::ONE,
        CcmpPacketNumber::new(CCMP_PACKET_NUMBER_MAX - 1).unwrap(),
    );
    let mut last = original.clone();
    tx.protect(&mut numbers, &mut last, MANAGEMENT_HEADER_LEN + 4)
        .unwrap();
    receiver().open(&mut last).unwrap();
    for _ in 0..2 {
        let mut rejected = original.clone();
        assert_eq!(
            tx.protect(&mut numbers, &mut rejected, MANAGEMENT_HEADER_LEN + 4),
            Err(ManagementCcmpTransmitError::PacketNumberExhausted)
        );
        assert_eq!(rejected, original);
        assert_eq!(numbers.last().value(), CCMP_PACKET_NUMBER_MAX);
    }
}

#[test]
fn body_lengths_must_fit_the_ccm_field_before_any_mutation() {
    let body = std::vec![0; usize::from(u16::MAX) + 1];
    let mut output = outgoing(&body);
    let original = output.clone();
    let mut numbers = CcmpTxPacketNumber::new(CcmpPacketNumberStep::ONE);
    assert_eq!(
        transmitter().protect(
            &mut numbers,
            &mut output,
            MANAGEMENT_HEADER_LEN + body.len()
        ),
        Err(ManagementCcmpTransmitError::BodyTooLong)
    );
    assert_eq!(output, original);
    assert_eq!(numbers.last(), CcmpPacketNumber::ZERO);

    // The receive entry point rejects oversized ciphertext before XOR.
    output[1] |= PROTECTED;
    output[MANAGEMENT_HEADER_LEN..MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN].copy_from_slice(
        &CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode(),
    );
    let untouched = output.clone();
    let mut rx = receiver();
    assert_eq!(rx.open(&mut output), Err(ManagementCcmpError::BodyTooLong));
    assert_eq!(output, untouched);
    assert!(rx.open(&mut frame()).is_ok());
}
