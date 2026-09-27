//! Authentication and key management suites negotiated in the RSN element.
//!
//! The four-way handshake, EAPOL-Key framing, AES key wrap and key
//! installation are shared by every suite. A suite selects only its key
//! descriptor version, pairwise key expansion and EAPOL-Key MIC; each property
//! is an exhaustive match so adding a suite forces every dependent decision.

use aes::Aes128;
use cmac::Cmac;
use hmac::{Hmac, Mac};
use oer_ieee80211_mac::security::rsn::ieee_suite;
use sha1::Sha1;
use sha2::Sha256;
use zeroize::Zeroize;

use crate::{RSN_KCK_LEN, RSN_PTK_LEN};

const PTK_EXPANSION_LABEL: &[u8] = b"Pairwise key expansion";

/// Length of every EAPOL-Key MIC produced by a supported suite.
pub const RSN_MIC_LEN: usize = 16;

/// Authentication and key management suite of one association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Akm {
    /// `00-0F-AC:2`, PSK with PRF-SHA1 key expansion and HMAC-SHA1-128 MIC
    /// (key descriptor version 2).
    Psk,
    /// `00-0F-AC:6`, PSK with the SHA-256 key derivation function and
    /// AES-128-CMAC MIC (key descriptor version 3), as protected management
    /// frames select.
    PskSha256,
}

impl Akm {
    /// The suite named by an RSN element AKM selector, if supported.
    pub const fn from_suite_selector(selector: [u8; 4]) -> Option<Self> {
        match selector {
            [0x00, 0x0f, 0xac, 2] => Some(Self::Psk),
            [0x00, 0x0f, 0xac, 6] => Some(Self::PskSha256),
            _ => None,
        }
    }

    pub const fn suite_selector(self) -> [u8; 4] {
        let type_ = match self {
            Self::Psk => 2,
            Self::PskSha256 => 6,
        };
        ieee_suite(type_)
    }

    /// Key Information descriptor version carried by every EAPOL-Key frame.
    pub const fn key_descriptor_version(self) -> u8 {
        match self {
            Self::Psk => 2,
            Self::PskSha256 => 3,
        }
    }

    /// Expand a PMK over the canonical address/nonce context into a PTK.
    pub(crate) fn expand_ptk(self, pmk: &[u8; 32], context: &[u8; 76]) -> [u8; RSN_PTK_LEN] {
        match self {
            Self::Psk => prf_sha1(pmk, context),
            Self::PskSha256 => kdf_sha256(pmk, context),
        }
    }

    pub(crate) fn mic(self, kck: &[u8; RSN_KCK_LEN]) -> EapolMic {
        match self {
            Self::Psk => EapolMic::HmacSha1(
                Hmac::<Sha1>::new_from_slice(kck).expect("KCK length is always accepted by HMAC"),
            ),
            Self::PskSha256 => EapolMic::AesCmac(
                Cmac::<Aes128>::new_from_slice(kck).expect("a 16-byte KCK is an AES-128 key"),
            ),
        }
    }
}

/// Incremental EAPOL-Key MIC of one suite.
#[expect(
    clippy::large_enum_variant,
    reason = "a MIC lives for one frame on the stack; the AES-CMAC key schedule is its whole state"
)]
pub(crate) enum EapolMic {
    HmacSha1(Hmac<Sha1>),
    AesCmac(Cmac<Aes128>),
}

impl EapolMic {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::HmacSha1(mac) => mac.update(bytes),
            Self::AesCmac(mac) => mac.update(bytes),
        }
    }

    pub(crate) fn finalize(self) -> [u8; RSN_MIC_LEN] {
        match self {
            Self::HmacSha1(mac) => {
                let mut digest = mac.finalize().into_bytes();
                let mut mic = [0; RSN_MIC_LEN];
                mic.copy_from_slice(&digest[..RSN_MIC_LEN]);
                digest.zeroize();
                mic
            }
            Self::AesCmac(mac) => {
                let mut tag = mac.finalize().into_bytes();
                let mut mic = [0; RSN_MIC_LEN];
                mic.copy_from_slice(&tag);
                tag.zeroize();
                mic
            }
        }
    }

    /// Constant-time comparison against a received MIC.
    pub(crate) fn verify(self, mic: &[u8; RSN_MIC_LEN]) -> bool {
        match self {
            Self::HmacSha1(mac) => mac.verify_truncated_left(mic).is_ok(),
            Self::AesCmac(mac) => mac.verify_slice(mic).is_ok(),
        }
    }
}

/// IEEE 802.11 PRF-384 with HMAC-SHA1 and the pairwise expansion label.
fn prf_sha1(pmk: &[u8; 32], context: &[u8; 76]) -> [u8; RSN_PTK_LEN] {
    let mut ptk = [0; RSN_PTK_LEN];
    let mut written = 0;
    let mut counter = 0_u8;
    while written < ptk.len() {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(pmk).expect("PMK length is always accepted by HMAC");
        mac.update(PTK_EXPANSION_LABEL);
        mac.update(&[0]);
        mac.update(context);
        mac.update(&[counter]);
        let mut block = mac.finalize().into_bytes();
        let count = core::cmp::min(block.len(), ptk.len() - written);
        ptk[written..written + count].copy_from_slice(&block[..count]);
        block.zeroize();
        written += count;
        counter = counter.wrapping_add(1);
    }
    ptk
}

/// IEEE 802.11 KDF-SHA-256-384 with the pairwise expansion label: HMAC-SHA-256
/// over a little-endian block counter from one, the label, the context and
/// the little-endian output length in bits.
fn kdf_sha256(pmk: &[u8; 32], context: &[u8; 76]) -> [u8; RSN_PTK_LEN] {
    const OUTPUT_BITS: u16 = (RSN_PTK_LEN * 8) as u16;
    let mut ptk = [0; RSN_PTK_LEN];
    let mut written = 0;
    let mut counter = 1_u16;
    while written < ptk.len() {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(pmk).expect("PMK length is always accepted by HMAC");
        mac.update(&counter.to_le_bytes());
        mac.update(PTK_EXPANSION_LABEL);
        mac.update(context);
        mac.update(&OUTPUT_BITS.to_le_bytes());
        let mut block = mac.finalize().into_bytes();
        let count = core::cmp::min(block.len(), ptk.len() - written);
        ptk[written..written + count].copy_from_slice(&block[..count]);
        block.zeroize();
        written += count;
        counter += 1;
    }
    ptk
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suite_selectors_round_trip_and_unknown_suites_are_unsupported() {
        assert_eq!(
            Akm::from_suite_selector(Akm::Psk.suite_selector()),
            Some(Akm::Psk)
        );
        assert_eq!(Akm::Psk.suite_selector(), [0x00, 0x0f, 0xac, 2]);
        assert_eq!(
            Akm::from_suite_selector([0x00, 0x0f, 0xac, 6]),
            Some(Akm::PskSha256)
        );
        // SAE is a recognizable selector but not implemented.
        assert_eq!(Akm::from_suite_selector([0x00, 0x0f, 0xac, 8]), None);
        assert_eq!(Akm::from_suite_selector([0x00, 0x50, 0xf2, 2]), None);
    }

    #[test]
    fn mic_verification_accepts_only_the_finalized_value() {
        let kck = [7; RSN_KCK_LEN];
        let mut mic = Akm::Psk.mic(&kck);
        mic.update(b"frame");
        let value = mic.finalize();
        let mut check = Akm::Psk.mic(&kck);
        check.update(b"frame");
        assert!(check.verify(&value));
        let mut tampered = Akm::Psk.mic(&kck);
        tampered.update(b"framf");
        assert!(!tampered.verify(&value));
    }

    #[test]
    fn psk_sha256_expands_with_the_sha256_kdf_and_signs_with_aes_cmac() {
        let pmk: [u8; 32] = core::array::from_fn(|index| index as u8);
        let context: [u8; 76] = core::array::from_fn(|index| 100 + index as u8);
        // Computed independently with Python's hmac/hashlib.
        let expected = [
            0xf8, 0xbe, 0xe5, 0x0e, 0x6e, 0x9c, 0xfe, 0x26, 0xa3, 0xd5, 0x6c, 0x3b, 0x7f, 0x72,
            0x81, 0x59, 0x7f, 0xda, 0xe9, 0x91, 0x87, 0x90, 0xe8, 0xf1, 0xd4, 0xf3, 0xeb, 0x09,
            0x11, 0xfa, 0x6b, 0x7b, 0x86, 0xea, 0xca, 0xbc, 0x7a, 0xde, 0xcc, 0x4a, 0xe7, 0xd5,
            0x81, 0xc5, 0xac, 0xb1, 0x5c, 0xf9,
        ];
        assert_eq!(Akm::PskSha256.expand_ptk(&pmk, &context), expected);
        assert_eq!(Akm::PskSha256.key_descriptor_version(), 3);

        // Computed independently with Python's cryptography CMAC.
        let mut mic = Akm::PskSha256.mic(&[7; RSN_KCK_LEN]);
        mic.update(b"eapol-frame-bytes");
        assert_eq!(
            mic.finalize(),
            [
                0xc9, 0x54, 0x6e, 0xe7, 0xb6, 0xed, 0x61, 0x8c, 0x15, 0x91, 0x63, 0xcb, 0x3a, 0x83,
                0xe9, 0xb4
            ]
        );
    }
}
