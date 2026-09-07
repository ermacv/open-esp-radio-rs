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

pub mod crypto;
pub mod eapol;

pub use crypto::{
    AssociationSecurityBinding, Pmk, Ptk, PtkContext, WPA2_KCK_LEN, WPA2_KEK_LEN,
    WPA2_KEY_DATA_CAPACITY, WPA2_PASSPHRASE_MAX_LEN, WPA2_PASSPHRASE_MIN_LEN,
    WPA2_PBKDF2_ITERATIONS, WPA2_PTK_LEN, WPA2_SSID_MAX_LEN, WPA2_UNWRAPPED_KEY_DATA_CAPACITY,
    Wpa2CryptoError,
};
pub(crate) use crypto::{Wpa2KeyConfirmationKey, Wpa2KeyEncryptionKey};
pub use eapol::{
    DEFAULT_EAPOL_FRAME_CAPACITY, EAPOL_HEADER_LEN, EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN,
    EAPOL_PACKET_TYPE_KEY, EapolCopyError, EapolKeyFrame, EapolKeyInfo, EapolKeyMessage,
    EapolParseError, OwnedEapolFrame, RSN_KEY_DESCRIPTOR_TYPE,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2Interface {
    Station,
    AccessPoint,
}
