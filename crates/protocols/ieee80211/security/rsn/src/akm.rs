//! Authentication and key management suites negotiated in the RSN element.
//!
//! The four-way handshake, EAPOL-Key framing, AES key wrap and key
//! installation are shared by every suite. A suite selects only its key
//! descriptor version, pairwise key expansion and EAPOL-Key MIC; each property
//! is an exhaustive match so adding a suite forces every dependent decision.

use hmac::{Hmac, Mac};
use oer_ieee80211_mac::security::rsn::ieee_suite;
use sha1::Sha1;
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
}

impl Akm {
    /// The suite named by an RSN element AKM selector, if supported.
    pub const fn from_suite_selector(selector: [u8; 4]) -> Option<Self> {
        match selector {
            [0x00, 0x0f, 0xac, 2] => Some(Self::Psk),
            _ => None,
        }
    }

    pub const fn suite_selector(self) -> [u8; 4] {
        let type_ = match self {
            Self::Psk => 2,
        };
        ieee_suite(type_)
    }

    /// Key Information descriptor version carried by every EAPOL-Key frame.
    pub const fn key_descriptor_version(self) -> u8 {
        match self {
            Self::Psk => 2,
        }
    }

    /// Expand a PMK over the canonical address/nonce context into a PTK.
    pub(crate) fn expand_ptk(self, pmk: &[u8; 32], context: &[u8; 76]) -> [u8; RSN_PTK_LEN] {
        match self {
            Self::Psk => prf_sha1(pmk, context),
        }
    }

    pub(crate) fn mic(self, kck: &[u8; RSN_KCK_LEN]) -> EapolMic {
        match self {
            Self::Psk => EapolMic::HmacSha1(
                Hmac::<Sha1>::new_from_slice(kck).expect("KCK length is always accepted by HMAC"),
            ),
        }
    }
}

/// Incremental EAPOL-Key MIC of one suite.
pub(crate) enum EapolMic {
    HmacSha1(Hmac<Sha1>),
}

impl EapolMic {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::HmacSha1(mac) => mac.update(bytes),
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
        }
    }

    /// Constant-time comparison against a received MIC.
    pub(crate) fn verify(self, mic: &[u8; RSN_MIC_LEN]) -> bool {
        match self {
            Self::HmacSha1(mac) => mac.verify_truncated_left(mic).is_ok(),
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
        // SAE and PSK-SHA256 are recognizable selectors but not implemented.
        assert_eq!(Akm::from_suite_selector([0x00, 0x0f, 0xac, 8]), None);
        assert_eq!(Akm::from_suite_selector([0x00, 0x0f, 0xac, 6]), None);
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
}
