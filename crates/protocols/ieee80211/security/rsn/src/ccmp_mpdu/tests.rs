use super::*;
use std::vec::Vec;

// Independent Python cryptography AESCCM fixtures, explicit 802.11 AAD/nonce.
// Both directions carry the same valid one-subframe A-MSDU, TID 5, PN 010203040506.
const WIRE: &[(bool, bool, &str)] = &[
    (
        false,
        false,
        "88410000020000000002020000000001020000000003301285000605002004030201f07eede0098f221607e15fba00f6d3700d45aacc36579a5c2fb58aad5da6dac379a5",
    ),
    (
        false,
        true,
        "88420000020000000001020000000002020000000003301285000605002004030201d48908a774db803d957983d3cada8502b9396fef745b09a70a3e9546c93ea961a5e2",
    ),
    (
        true,
        false,
        "884100000200000000020200000000010200000000033012850006050020040302013edc756662394dbcfb0ae5899079214592511815ae78bf94e73bebfda59517877bdd",
    ),
    (
        true,
        true,
        "88420000020000000001020000000002020000000003301285000605002004030201bcd472f2766f73794735481b94e75ff9bde8c13d1359f9fd5341ddf58e25ff82a425",
    ),
];

fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn mode(spp: bool) -> CcmpAssociation {
    CcmpAssociation::new(if spp {
        Spp::Negotiated
    } else {
        Spp::NotNegotiated
    })
}
fn key() -> [u8; 16] {
    core::array::from_fn(|i| i as u8)
}

#[test]
fn amsdu_wire_peer_matches_in_both_directions_with_and_without_spp() {
    for &(spp, _down, fixture) in WIRE {
        let wire = bytes(fixture);
        let mut opened = wire.clone();
        let association = mode(spp);
        let body = association.open(&key(), &mut opened).unwrap();
        assert_eq!(&body[..6], &[2, 0, 0, 0, 0, 3]);
        assert_eq!(&body[14..], b"\xaa\xaa\x03\x00\x00\x00\x08\x00test");
        association.seal(&key(), &mut opened).unwrap();
        assert_eq!(opened, wire);
    }
}

#[test]
fn spp_authenticates_amsdu_present_but_ordinary_ccmp_does_not() {
    for &(spp, _, fixture) in WIRE {
        let mut changed = bytes(fixture);
        changed[24] ^= 0x80;
        let result = mode(spp).open(&key(), &mut changed);
        if spp {
            assert_eq!(result, Err(CcmpMpduError::InvalidMic));
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn mutable_retry_power_and_sequence_fields_do_not_change_mic() {
    for &(spp, _, fixture) in WIRE {
        let mut changed = bytes(fixture);
        changed[1] ^= 0x38;
        changed[22] ^= 0xf0;
        changed[23] ^= 0xff;
        assert!(mode(spp).open(&key(), &mut changed).is_ok());
        let mut tampered = bytes(fixture);
        tampered[4] ^= 2;
        assert_eq!(
            mode(spp).open(&key(), &mut tampered),
            Err(CcmpMpduError::InvalidMic)
        );
        assert!(
            tampered[34..tampered.len() - 8]
                .iter()
                .all(|byte| *byte == 0)
        );
    }
}

#[test]
fn oversized_data_is_refused_before_encryption_or_decryption() {
    let mut oversized = bytes(WIRE[0].2);
    oversized.resize(26 + 8 + usize::from(u16::MAX) + 1 + MIC_LEN, 0xa5);
    let before = oversized.clone();
    let association = mode(false);
    assert_eq!(
        association.seal(&key(), &mut oversized),
        Err(CcmpMpduError::TooLong)
    );
    assert_eq!(oversized, before);
    assert_eq!(
        association.open(&key(), &mut oversized),
        Err(CcmpMpduError::TooLong)
    );
    assert_eq!(oversized, before);
}

#[test]
fn management_frames_share_ccm_but_keep_their_own_aad_and_nonce_flags() {
    use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    for spp in [false, true] {
        let mut frame = std::vec![0; 24];
        frame[..2].copy_from_slice(&0x40d0_u16.to_le_bytes());
        frame[4..10].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        frame.extend_from_slice(
            &CcmpHeader::new(CcmpPacketNumber::new(1).unwrap(), CcmpKeyId::PAIRWISE).encode(),
        );
        frame.extend_from_slice(&[8, 0, 0x42, 0x23]);
        frame.extend_from_slice(&[0; 8]);
        mode(spp).seal(&key(), &mut frame).unwrap();
        let mut receiver = crate::management_ccmp::ManagementCcmpReceiver::new(key());
        assert_eq!(receiver.open(&mut frame).unwrap(), [8, 0, 0x42, 0x23]);
    }
}
