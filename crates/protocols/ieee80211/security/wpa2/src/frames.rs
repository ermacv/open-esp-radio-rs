//! Fixed WPA2-CCMP EAPOL frame construction and GTK key-data parsing.

use hmac::Mac;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::state::{Wpa2ApState, Wpa2StaState, Wpa2Transmit, Wpa2TxMessage};
use crate::{
    EAPOL_KEY_FIXED_LEN, EAPOL_KEY_PACKET_LEN, EAPOL_PACKET_TYPE_KEY, EapolKeyFrame, Ptk,
    RSN_KEY_DESCRIPTOR_TYPE, Wpa2Interface, Wpa2KeyConfirmationKey,
};

pub const WPA2_RSN_IE_CAPACITY: usize = 64;
pub const WPA2_ASSOC_SECURITY_IES_CAPACITY: usize = 128;
pub const WPA2_GTK_LEN: usize = 16;
pub const WPA2_PLAIN_KEY_DATA_CAPACITY: usize = 128;
pub const WPA2_TX_EAPOL_CAPACITY: usize = 512;
pub const WPA2_TX_ETHERNET_CAPACITY: usize = WPA2_TX_EAPOL_CAPACITY + 14;

const RSN_ELEMENT_ID: u8 = 0x30;
const RSNXE_ELEMENT_ID: u8 = 0xf4;
const VENDOR_ELEMENT_ID: u8 = 0xdd;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const GTK_KDE_TYPE: u8 = 1;
const EAPOL_ETHERTYPE: [u8; 2] = [0x88, 0x8e];

const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_INSTALL: u16 = 1 << 6;
const KEY_INFO_ACK: u16 = 1 << 7;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_INFO_ENCRYPTED_KEY_DATA: u16 = 1 << 12;
const KEY_DESCRIPTOR_VERSION_HMAC_SHA1: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2FrameError {
    CapacityExceeded,
    InvalidRsnIe,
    InvalidRsnxe,
    InvalidKeyId,
    ZeroNonce,
    EmptyKeyData,
    MalformedKeyData,
    UnsupportedKeyData,
    DuplicateRsnIe,
    DuplicateRsnxe,
    DuplicateGtk,
    MissingRsnIe,
    MissingRsnxe,
    UnexpectedRsnxe,
    MissingGtk,
    RsnIeMismatch,
    RsnxeMismatch,
    UnexpectedTransmitAction,
}

mod key_data;
mod security_ies;
mod transmit;

pub use key_data::{Wpa2Gtk, Wpa2PlainKeyData, parse_group_gtk_key_data, parse_gtk_key_data};
pub use security_ies::{OwnedAssociationSecurityIes, OwnedRsnIe};
pub use transmit::{Wpa2EthernetFrame, Wpa2TxFrame, build_ap_action_frame, build_sta_action_frame};

#[cfg(test)]
mod tests;
