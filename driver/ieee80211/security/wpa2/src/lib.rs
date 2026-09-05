#![no_std]
#![forbid(unsafe_code)]

//! Allocation-free WPA2-Personal protocol primitives.
//!
//! This is the source-owned home for hardware-independent WPA2 code. It
//! validates and classifies complete
//! RSN EAPOL-Key packets, owns the station/authenticator state machines and
//! joins station PTK/MIC/key-data processing to typed key-install requests.
//! Platform MAC crates remain responsible only for executing those requests.

#[cfg(test)]
extern crate std;

pub mod aes;
pub mod ap;
pub mod frames;
pub mod keys;
pub mod retry;
pub mod runner;
pub mod state;
pub mod supplicant;

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
const WPA2_ASSOCIATION_SECURITY_BINDING_LABEL: &[u8] =
    b"open-esp-radio-rs AP association security IEs";
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

/// PMK-authenticated commitment to the exact Association RSN IE plus RSNXE.
///
/// An authenticator stores this bounded value per peer instead of retaining a
/// second copy of the complete management-frame elements. It can later bind
/// Message 2 Key Data to the exact Association bytes without exposing the PMK
/// or expanding the fixed AP peer table by the IE capacity for every client.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AssociationSecurityBinding([u8; 16]);

impl AssociationSecurityBinding {
    pub fn matches(&self, pmk: &Pmk, association_security_ies: &[u8]) -> bool {
        let mac = pmk.association_security_binding_mac(association_security_ies);
        mac.verify_truncated_left(&self.0).is_ok()
    }
}

impl Pmk {
    /// Import an already-derived 256-bit PSK.
    ///
    /// This avoids retaining a passphrase in applications which provision raw
    /// key material. The owned bytes receive the same zeroize-on-drop policy
    /// as a locally derived key.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

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

    pub fn bind_association_security_ies(
        &self,
        association_security_ies: &[u8],
    ) -> AssociationSecurityBinding {
        let mut digest = self
            .association_security_binding_mac(association_security_ies)
            .finalize()
            .into_bytes();
        let mut binding = [0; 16];
        binding.copy_from_slice(&digest[..16]);
        digest.zeroize();
        AssociationSecurityBinding(binding)
    }

    fn association_security_binding_mac(&self, association_security_ies: &[u8]) -> Hmac<Sha1> {
        let mut mac = Hmac::<Sha1>::new_from_slice(&self.0)
            .expect("WPA2 PMK length is always accepted by HMAC");
        mac.update(WPA2_ASSOCIATION_SECURITY_BINDING_LABEL);
        mac.update(&(association_security_ies.len() as u64).to_be_bytes());
        mac.update(association_security_ies);
        mac
    }
}

/// WPA2 pairwise transient key material (KCK | KEK | TK), cleared on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Ptk([u8; WPA2_PTK_LEN]);

/// Connected-state EAPOL authentication authority derived from one PTK.
///
/// The temporal key is published to hardware before this value is created.
/// Keeping the KCK in its own zeroizing owner lets the connected supplicant
/// authenticate retransmitted Message 3 without retaining a software copy of
/// the installed CCMP key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct Wpa2KeyConfirmationKey([u8; WPA2_KCK_LEN]);

/// Connected-state key-data unwrap authority derived from one PTK.
///
/// This is retained only for an authenticated Group Message 1. It is separate
/// from the completed-Message-3 context so that path owns exactly its KCK and
/// protocol binding, never the installed temporal key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct Wpa2KeyEncryptionKey([u8; WPA2_KEK_LEN]);

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

    pub(crate) fn into_connected_keys(mut self) -> (Wpa2KeyConfirmationKey, Wpa2KeyEncryptionKey) {
        let mut kck = [0; WPA2_KCK_LEN];
        kck.copy_from_slice(self.kck());
        let mut kek = [0; WPA2_KEK_LEN];
        kek.copy_from_slice(self.kek());
        // Do not retain a second software copy of the installed temporal key
        // until this value happens to leave scope.
        self.zeroize();
        (Wpa2KeyConfirmationKey(kck), Wpa2KeyEncryptionKey(kek))
    }
}

impl Wpa2KeyConfirmationKey {
    pub(crate) const fn as_bytes(&self) -> &[u8; WPA2_KCK_LEN] {
        &self.0
    }
}

impl Wpa2KeyEncryptionKey {
    pub(crate) const fn as_bytes(&self) -> &[u8; WPA2_KEK_LEN] {
        &self.0
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

    pub fn key_iv(self) -> &'a [u8; 16] {
        self.bytes[49..65]
            .try_into()
            .expect("validated EAPOL-Key IV range")
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

    pub fn key_identifier(self) -> &'a [u8; 8] {
        self.bytes[73..81]
            .try_into()
            .expect("validated EAPOL-Key identifier range")
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.key_data
    }

    /// Verifies the WPA2 HMAC-SHA1-128 MIC without copying the frame.
    pub fn verify_mic(self, ptk: &Ptk) -> bool {
        self.verify_mic_with_kck(ptk.kck())
    }

    pub(crate) fn verify_mic_with_confirmation_key(self, key: &Wpa2KeyConfirmationKey) -> bool {
        self.verify_mic_with_kck(key.as_bytes())
    }

    fn verify_mic_with_kck(self, kck: &[u8; WPA2_KCK_LEN]) -> bool {
        let mut mac =
            Hmac::<Sha1>::new_from_slice(kck).expect("WPA2 KCK length is always accepted by HMAC");
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
mod tests;
