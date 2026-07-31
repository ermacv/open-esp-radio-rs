#![no_std]

//! Allocation-free WPA2-Personal protocol primitives.
//!
//! This is the active home for hardware-independent WPA2 code recovered in
//! `migration/esp32s31-hybrid-runtime`. The first migrated boundary validates
//! and classifies complete RSN EAPOL-Key packets. Cryptographic state and key
//! installation remain separate from parsing and will move behind explicit
//! ownership types as the open STA path reaches those protocol transitions.

pub mod aes;
pub mod ap;
pub mod frames;
pub mod keys;
pub mod retry;
pub mod state;

use hmac::{Hmac, Mac};
use sha1::Sha1;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const EAPOL_HEADER_LEN: usize = 4;
pub const EAPOL_KEY_FIXED_LEN: usize = 95;
pub const EAPOL_KEY_PACKET_LEN: usize = EAPOL_HEADER_LEN + EAPOL_KEY_FIXED_LEN;
pub const EAPOL_PACKET_TYPE_KEY: u8 = 3;
pub const RSN_KEY_DESCRIPTOR_TYPE: u8 = 2;
pub const DEFAULT_EAPOL_FRAME_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2Interface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolCopyError {
    CapacityExceeded,
    Invalid(EapolParseError),
}

const KEY_INFO_DESCRIPTOR_VERSION_MASK: u16 = 0x0007;
const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_INSTALL: u16 = 1 << 6;
const KEY_INFO_ACK: u16 = 1 << 7;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_INFO_ERROR: u16 = 1 << 10;
const KEY_INFO_REQUEST: u16 = 1 << 11;
const KEY_INFO_ENCRYPTED_KEY_DATA: u16 = 1 << 12;
const KEY_INFO_SMK: u16 = 1 << 13;

pub const WPA2_PASSPHRASE_MIN_LEN: usize = 8;
pub const WPA2_PASSPHRASE_MAX_LEN: usize = 63;
pub const WPA2_SSID_MAX_LEN: usize = 32;
pub const WPA2_PBKDF2_ITERATIONS: u32 = 4096;
pub const WPA2_PTK_LEN: usize = 48;
pub const WPA2_KCK_LEN: usize = 16;
pub const WPA2_KEK_LEN: usize = 16;
pub const WPA2_KEY_DATA_CAPACITY: usize = 512;
pub const WPA2_UNWRAPPED_KEY_DATA_CAPACITY: usize = WPA2_KEY_DATA_CAPACITY - 8;

const WPA2_PRF_LABEL: &[u8] = b"Pairwise key expansion";
const EAPOL_KEY_MIC_START: usize = 81;
const EAPOL_KEY_MIC_END: usize = 97;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2CryptoError {
    InvalidPassphraseLength,
    InvalidSsidLength,
}

/// Canonical addresses and nonces used by the WPA2 PRF-384.
#[derive(Clone, Copy)]
pub struct PtkContext {
    pub authenticator_address: [u8; 6],
    pub supplicant_address: [u8; 6],
    pub authenticator_nonce: [u8; 32],
    pub supplicant_nonce: [u8; 32],
}

/// Pairwise master key. The bytes cannot be formatted and are cleared on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Pmk([u8; 32]);

impl Pmk {
    pub fn derive(passphrase: &[u8], ssid: &[u8]) -> Result<Self, Wpa2CryptoError> {
        if !(WPA2_PASSPHRASE_MIN_LEN..=WPA2_PASSPHRASE_MAX_LEN).contains(&passphrase.len()) {
            return Err(Wpa2CryptoError::InvalidPassphraseLength);
        }
        if ssid.is_empty() || ssid.len() > WPA2_SSID_MAX_LEN {
            return Err(Wpa2CryptoError::InvalidSsidLength);
        }
        let mut bytes = [0; 32];
        pbkdf2::pbkdf2_hmac::<Sha1>(passphrase, ssid, WPA2_PBKDF2_ITERATIONS, &mut bytes);
        Ok(Self(bytes))
    }

    pub fn derive_ptk(&self, context: PtkContext) -> Ptk {
        let mut canonical = [0; 76];
        let (first_address, second_address) =
            ordered(&context.authenticator_address, &context.supplicant_address);
        canonical[..6].copy_from_slice(first_address);
        canonical[6..12].copy_from_slice(second_address);
        let (first_nonce, second_nonce) =
            ordered(&context.authenticator_nonce, &context.supplicant_nonce);
        canonical[12..44].copy_from_slice(first_nonce);
        canonical[44..76].copy_from_slice(second_nonce);

        let mut ptk = [0; WPA2_PTK_LEN];
        let mut written = 0;
        let mut counter = 0_u8;
        while written < ptk.len() {
            let mut mac = Hmac::<Sha1>::new_from_slice(&self.0)
                .expect("WPA2 PMK length is always accepted by HMAC");
            mac.update(WPA2_PRF_LABEL);
            mac.update(&[0]);
            mac.update(&canonical);
            mac.update(&[counter]);
            let block = mac.finalize().into_bytes();
            let count = core::cmp::min(block.len(), ptk.len() - written);
            ptk[written..written + count].copy_from_slice(&block[..count]);
            written += count;
            counter = counter.wrapping_add(1);
        }
        canonical.zeroize();
        Ptk(ptk)
    }
}

/// WPA2 pairwise transient key material (KCK | KEK | TK), cleared on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Ptk([u8; WPA2_PTK_LEN]);

impl Ptk {
    pub fn kck(&self) -> &[u8; WPA2_KCK_LEN] {
        self.0[..WPA2_KCK_LEN]
            .try_into()
            .expect("KCK is the first 16 PTK bytes")
    }

    pub fn kek(&self) -> &[u8; WPA2_KEK_LEN] {
        self.0[WPA2_KCK_LEN..WPA2_KCK_LEN + WPA2_KEK_LEN]
            .try_into()
            .expect("KEK is PTK bytes 16..32")
    }

    pub fn temporal_key(&self) -> &[u8; 16] {
        self.0[32..48]
            .try_into()
            .expect("CCMP temporal key is PTK bytes 32..48")
    }
}

fn ordered<'a, const N: usize>(
    left: &'a [u8; N],
    right: &'a [u8; N],
) -> (&'a [u8; N], &'a [u8; N]) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolParseError {
    Truncated,
    LengthMismatch,
    NotKeyPacket,
    NotRsnKeyDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolKeyMessage {
    PairwiseMessage1,
    PairwiseMessage2,
    PairwiseMessage3,
    PairwiseMessage4,
    GroupMessage1,
    GroupMessage2,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyInfo(u16);

impl EapolKeyInfo {
    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn descriptor_version(self) -> u8 {
        (self.0 & KEY_INFO_DESCRIPTOR_VERSION_MASK) as u8
    }

    pub const fn is_pairwise(self) -> bool {
        self.0 & KEY_INFO_PAIRWISE != 0
    }

    pub const fn install(self) -> bool {
        self.0 & KEY_INFO_INSTALL != 0
    }

    pub const fn ack(self) -> bool {
        self.0 & KEY_INFO_ACK != 0
    }

    pub const fn mic(self) -> bool {
        self.0 & KEY_INFO_MIC != 0
    }

    pub const fn secure(self) -> bool {
        self.0 & KEY_INFO_SECURE != 0
    }

    pub const fn error(self) -> bool {
        self.0 & KEY_INFO_ERROR != 0
    }

    pub const fn request(self) -> bool {
        self.0 & KEY_INFO_REQUEST != 0
    }

    pub const fn encrypted_key_data(self) -> bool {
        self.0 & KEY_INFO_ENCRYPTED_KEY_DATA != 0
    }

    pub const fn smk(self) -> bool {
        self.0 & KEY_INFO_SMK != 0
    }

    pub const fn classify(self) -> EapolKeyMessage {
        if self.error() || self.request() || self.smk() {
            return EapolKeyMessage::Other;
        }

        if self.is_pairwise() {
            match (self.ack(), self.mic(), self.install(), self.secure()) {
                (true, false, false, false) => EapolKeyMessage::PairwiseMessage1,
                (false, true, false, false) => EapolKeyMessage::PairwiseMessage2,
                (true, true, true, _) => EapolKeyMessage::PairwiseMessage3,
                (false, true, false, true) => EapolKeyMessage::PairwiseMessage4,
                _ => EapolKeyMessage::Other,
            }
        } else {
            match (self.ack(), self.mic()) {
                (true, true) => EapolKeyMessage::GroupMessage1,
                (false, true) => EapolKeyMessage::GroupMessage2,
                _ => EapolKeyMessage::Other,
            }
        }
    }
}

/// Validated borrowed view of one complete RSN EAPOL-Key packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyFrame<'a> {
    bytes: &'a [u8],
    key_data: &'a [u8],
    key_info: EapolKeyInfo,
}

impl<'a> EapolKeyFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        if bytes.len() < EAPOL_KEY_PACKET_LEN {
            return Err(EapolParseError::Truncated);
        }
        if bytes[1] != EAPOL_PACKET_TYPE_KEY {
            return Err(EapolParseError::NotKeyPacket);
        }

        let body_len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        let packet_len = EAPOL_HEADER_LEN
            .checked_add(body_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if packet_len != bytes.len() || body_len < EAPOL_KEY_FIXED_LEN {
            return Err(EapolParseError::LengthMismatch);
        }
        if bytes[4] != RSN_KEY_DESCRIPTOR_TYPE {
            return Err(EapolParseError::NotRsnKeyDescriptor);
        }

        let key_data_len = u16::from_be_bytes([bytes[97], bytes[98]]) as usize;
        let key_data_end = EAPOL_KEY_PACKET_LEN
            .checked_add(key_data_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if key_data_end != packet_len {
            return Err(EapolParseError::LengthMismatch);
        }

        Ok(Self {
            bytes,
            key_data: &bytes[EAPOL_KEY_PACKET_LEN..key_data_end],
            key_info: EapolKeyInfo(u16::from_be_bytes([bytes[5], bytes[6]])),
        })
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn protocol_version(self) -> u8 {
        self.bytes[0]
    }

    pub const fn key_info(self) -> EapolKeyInfo {
        self.key_info
    }

    pub const fn message(self) -> EapolKeyMessage {
        self.key_info.classify()
    }

    pub fn key_length(self) -> u16 {
        u16::from_be_bytes([self.bytes[7], self.bytes[8]])
    }

    pub fn replay_counter(self) -> u64 {
        u64::from_be_bytes([
            self.bytes[9],
            self.bytes[10],
            self.bytes[11],
            self.bytes[12],
            self.bytes[13],
            self.bytes[14],
            self.bytes[15],
            self.bytes[16],
        ])
    }

    pub fn nonce(self) -> &'a [u8; 32] {
        self.bytes[17..49]
            .try_into()
            .expect("validated EAPOL-Key nonce range")
    }

    pub fn mic(self) -> &'a [u8; 16] {
        self.bytes[81..97]
            .try_into()
            .expect("validated EAPOL-Key MIC range")
    }

    pub fn key_receive_sequence(self) -> &'a [u8; 8] {
        self.bytes[65..73]
            .try_into()
            .expect("validated EAPOL-Key RSC range")
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.key_data
    }

    /// Verifies the WPA2 HMAC-SHA1-128 MIC without copying the frame.
    pub fn verify_mic(self, ptk: &Ptk) -> bool {
        let mut mac = Hmac::<Sha1>::new_from_slice(ptk.kck())
            .expect("WPA2 KCK length is always accepted by HMAC");
        mac.update(&self.bytes[..EAPOL_KEY_MIC_START]);
        mac.update(&[0; EAPOL_KEY_MIC_END - EAPOL_KEY_MIC_START]);
        mac.update(&self.bytes[EAPOL_KEY_MIC_END..]);
        mac.verify_truncated_left(self.mic()).is_ok()
    }
}

/// Complete validated EAPOL-Key packet with fixed, Rust-owned storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedEapolFrame<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    interface: Wpa2Interface,
    peer: [u8; 6],
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedEapolFrame<N> {
    pub fn try_copy(
        interface: Wpa2Interface,
        peer: [u8; 6],
        bytes: &[u8],
    ) -> Result<Self, EapolCopyError> {
        EapolKeyFrame::parse(bytes).map_err(EapolCopyError::Invalid)?;
        if bytes.len() > N {
            return Err(EapolCopyError::CapacityExceeded);
        }

        let mut owned = [0; N];
        owned[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            interface,
            peer,
            len: bytes.len(),
            bytes: owned,
        })
    }

    pub const fn interface(&self) -> Wpa2Interface {
        self.interface
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn key_frame(&self) -> EapolKeyFrame<'_> {
        EapolKeyFrame::parse(self.as_bytes()).expect("owned EAPOL frame was validated on creation")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_packet(key_info: u16, replay_counter: u64) -> [u8; EAPOL_KEY_PACKET_LEN] {
        let mut packet = [0; EAPOL_KEY_PACKET_LEN];
        packet[0] = 2;
        packet[1] = EAPOL_PACKET_TYPE_KEY;
        packet[2..4].copy_from_slice(&(EAPOL_KEY_FIXED_LEN as u16).to_be_bytes());
        packet[4] = RSN_KEY_DESCRIPTOR_TYPE;
        packet[5..7].copy_from_slice(&key_info.to_be_bytes());
        packet[9..17].copy_from_slice(&replay_counter.to_be_bytes());
        packet
    }

    #[test]
    fn parses_and_classifies_pairwise_messages() {
        let message1 = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 7);
        let parsed = EapolKeyFrame::parse(&message1).unwrap();
        assert_eq!(parsed.message(), EapolKeyMessage::PairwiseMessage1);
        assert_eq!(parsed.key_info().descriptor_version(), 2);
        assert_eq!(parsed.replay_counter(), 7);

        let message3 = key_packet(
            KEY_INFO_PAIRWISE
                | KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_INSTALL
                | KEY_INFO_SECURE
                | 2,
            8,
        );
        assert_eq!(
            EapolKeyFrame::parse(&message3).unwrap().message(),
            EapolKeyMessage::PairwiseMessage3
        );
    }

    #[test]
    fn rejects_ambiguous_lengths_and_non_rsn_descriptors() {
        let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
        packet[2..4].copy_from_slice(&0_u16.to_be_bytes());
        assert_eq!(
            EapolKeyFrame::parse(&packet),
            Err(EapolParseError::LengthMismatch)
        );

        let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 1);
        packet[4] = 254;
        assert_eq!(
            EapolKeyFrame::parse(&packet),
            Err(EapolParseError::NotRsnKeyDescriptor)
        );
    }

    #[test]
    fn derives_known_ieee_pmk() {
        let pmk = Pmk::derive(b"password", b"IEEE").unwrap();
        assert_eq!(
            pmk.0,
            [
                0xf4, 0x2c, 0x6f, 0xc5, 0x2d, 0xf0, 0xeb, 0xef, 0x9e, 0xbb, 0x4b, 0x90, 0xb3, 0x8a,
                0x5f, 0x90, 0x2e, 0x83, 0xfe, 0x1b, 0x13, 0x5a, 0x70, 0xe2, 0x3a, 0xed, 0x76, 0x2e,
                0x97, 0x10, 0xa1, 0x2e,
            ]
        );
    }

    #[test]
    fn derives_known_ptk_and_builds_message_2() {
        let pmk = Pmk(core::array::from_fn(|index| index as u8));
        let context = PtkContext {
            authenticator_address: [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            supplicant_address: [0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb],
            authenticator_nonce: core::array::from_fn(|index| index as u8 + 32),
            supplicant_nonce: core::array::from_fn(|index| index as u8 + 64),
        };
        let ptk = pmk.derive_ptk(context);
        assert_eq!(
            ptk.0,
            [
                0x35, 0x07, 0x82, 0xa5, 0x49, 0x8c, 0x27, 0x32, 0x15, 0xbf, 0x37, 0x70, 0x79, 0xe3,
                0x65, 0x0f, 0x63, 0x13, 0xd9, 0x26, 0xdb, 0xe9, 0xed, 0x87, 0x53, 0xa6, 0x0f, 0x1b,
                0x6e, 0x62, 0x25, 0xea, 0x5c, 0xbe, 0xca, 0x83, 0xd7, 0xbb, 0xa7, 0x6c, 0x9e, 0x6d,
                0x02, 0xa8, 0x48, 0xd1, 0xe5, 0x5f,
            ]
        );
        let rsn = [
            0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
        ];
        let security_ies =
            frames::OwnedAssociationSecurityIes::<128>::try_copy_bytes(&rsn).unwrap();
        let message = frames::Wpa2TxFrame::<512>::message2_with_security_ies(
            context.authenticator_address,
            7,
            context.supplicant_nonce,
            &security_ies,
        )
        .unwrap()
        .authenticate(&ptk);
        let parsed = EapolKeyFrame::parse(message.as_bytes()).unwrap();
        assert_eq!(parsed.message(), EapolKeyMessage::PairwiseMessage2);
        assert_eq!(parsed.replay_counter(), 7);
        assert_eq!(parsed.nonce(), &context.supplicant_nonce);
        assert_ne!(parsed.mic(), &[0; 16]);
        assert_eq!(parsed.key_data(), &rsn);

        let message4 = frames::Wpa2TxFrame::<512>::message4(context.authenticator_address, 8)
            .unwrap()
            .authenticate(&ptk);
        let parsed4 = EapolKeyFrame::parse(message4.as_bytes()).unwrap();
        assert_eq!(parsed4.protocol_version(), 1);
        assert_eq!(parsed4.message(), EapolKeyMessage::PairwiseMessage4);
        assert_eq!(parsed4.replay_counter(), 8);
        assert!(parsed4.verify_mic(&ptk));

        let mut changed = [0; EAPOL_KEY_PACKET_LEN];
        changed.copy_from_slice(message4.as_bytes());
        changed[17] ^= 1;
        assert!(!EapolKeyFrame::parse(&changed).unwrap().verify_mic(&ptk));
    }
}
