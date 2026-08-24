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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedRsnIe<const N: usize = WPA2_RSN_IE_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedRsnIe<N> {
    pub fn try_copy(bytes: &[u8]) -> Result<Self, Wpa2FrameError> {
        if bytes.len() < 2 || bytes[0] != RSN_ELEMENT_ID || bytes[1] as usize + 2 != bytes.len() {
            return Err(Wpa2FrameError::InvalidRsnIe);
        }
        if bytes.len() > N {
            return Err(Wpa2FrameError::CapacityExceeded);
        }
        let mut owned = [0; N];
        owned[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            len: bytes.len(),
            bytes: owned,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Exact RSN IE plus optional RSNXE emitted by the STA association request.
///
/// WPA2 message 2 must echo this complete byte sequence. Keeping it separate
/// from [`OwnedRsnIe`] preserves the stricter single-element invariant used
/// when validating message 3 key data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedAssociationSecurityIes<const N: usize = WPA2_ASSOC_SECURITY_IES_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedAssociationSecurityIes<N> {
    /// Copy the exact RSN IE plus optional RSNXE emitted by Association.
    pub fn try_copy_bytes(bytes: &[u8]) -> Result<Self, Wpa2FrameError> {
        if bytes.len() < 2 || bytes[0] != RSN_ELEMENT_ID {
            return Err(Wpa2FrameError::InvalidRsnIe);
        }
        let rsn_len = bytes[1] as usize + 2;
        let Some(rsn) = bytes.get(..rsn_len) else {
            return Err(Wpa2FrameError::InvalidRsnIe);
        };
        let rsn = OwnedRsnIe::<WPA2_RSN_IE_CAPACITY>::try_copy(rsn)?;
        let rsnxe = bytes.get(rsn_len..).ok_or(Wpa2FrameError::InvalidRsnxe)?;
        Self::try_copy(&rsn, rsnxe)
    }

    pub fn try_copy<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        rsnxe: &[u8],
    ) -> Result<Self, Wpa2FrameError> {
        if !rsnxe.is_empty()
            && (rsnxe.len() < 2
                || rsnxe[0] != RSNXE_ELEMENT_ID
                || rsnxe[1] as usize + 2 != rsnxe.len())
        {
            return Err(Wpa2FrameError::InvalidRsnxe);
        }
        let len = rsn_ie
            .as_bytes()
            .len()
            .checked_add(rsnxe.len())
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        if len > N {
            return Err(Wpa2FrameError::CapacityExceeded);
        }
        let mut bytes = [0; N];
        let rsn_len = rsn_ie.as_bytes().len();
        bytes[..rsn_len].copy_from_slice(rsn_ie.as_bytes());
        bytes[rsn_len..len].copy_from_slice(rsnxe);
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn rsn_ie(&self) -> &[u8] {
        let length = self.bytes[1] as usize + 2;
        &self.bytes[..length]
    }

    pub fn rsnxe(&self) -> &[u8] {
        let length = self.bytes[1] as usize + 2;
        &self.bytes[length..self.len]
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wpa2Gtk {
    key_id: u8,
    transmit: bool,
    key: [u8; WPA2_GTK_LEN],
}

impl Wpa2Gtk {
    pub fn new(
        key_id: u8,
        transmit: bool,
        key: [u8; WPA2_GTK_LEN],
    ) -> Result<Self, Wpa2FrameError> {
        if key_id > 3 {
            return Err(Wpa2FrameError::InvalidKeyId);
        }
        Ok(Self {
            key_id,
            transmit,
            key,
        })
    }

    pub const fn key_id(&self) -> u8 {
        self.key_id
    }

    pub const fn transmit(&self) -> bool {
        self.transmit
    }

    pub const fn key(&self) -> &[u8; WPA2_GTK_LEN] {
        &self.key
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wpa2PlainKeyData<const N: usize = WPA2_PLAIN_KEY_DATA_CAPACITY> {
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> Wpa2PlainKeyData<N> {
    pub fn build<const R: usize>(
        rsn_ie: &OwnedRsnIe<R>,
        gtk: &Wpa2Gtk,
    ) -> Result<Self, Wpa2FrameError> {
        let rsn = rsn_ie.as_bytes();
        let unpadded_len = rsn
            .len()
            .checked_add(24)
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        let padding = if unpadded_len < 16 {
            16 - unpadded_len
        } else {
            (8 - unpadded_len % 8) % 8
        };
        let len = unpadded_len
            .checked_add(padding)
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        if len > N {
            return Err(Wpa2FrameError::CapacityExceeded);
        }

        let mut bytes = [0; N];
        bytes[..rsn.len()].copy_from_slice(rsn);
        let kde = &mut bytes[rsn.len()..unpadded_len];
        kde[0] = VENDOR_ELEMENT_ID;
        kde[1] = 22;
        kde[2..5].copy_from_slice(&RSN_OUI);
        kde[5] = GTK_KDE_TYPE;
        kde[6] = gtk.key_id | u8::from(gtk.transmit) << 2;
        kde[7] = 0;
        kde[8..24].copy_from_slice(gtk.key());
        if padding != 0 {
            bytes[unpadded_len] = VENDOR_ELEMENT_ID;
        }
        Ok(Self { len, bytes })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

pub fn parse_gtk_key_data(
    bytes: &[u8],
    expected_rsn_ie: &[u8],
    expected_rsnxe: &[u8],
) -> Result<Wpa2Gtk, Wpa2FrameError> {
    let mut offset = 0;
    let mut saw_rsn = false;
    let mut saw_rsnxe = false;
    let mut gtk = None;

    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.iter().all(|byte| *byte == 0)
            || (remaining[0] == VENDOR_ELEMENT_ID && remaining[1..].iter().all(|byte| *byte == 0))
        {
            break;
        }
        if remaining.len() < 2 {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        match element[0] {
            RSN_ELEMENT_ID => {
                if saw_rsn {
                    return Err(Wpa2FrameError::DuplicateRsnIe);
                }
                if element != expected_rsn_ie {
                    return Err(Wpa2FrameError::RsnIeMismatch);
                }
                saw_rsn = true;
            }
            RSNXE_ELEMENT_ID => {
                if saw_rsnxe {
                    return Err(Wpa2FrameError::DuplicateRsnxe);
                }
                if expected_rsnxe.is_empty() {
                    return Err(Wpa2FrameError::UnexpectedRsnxe);
                }
                if element != expected_rsnxe {
                    return Err(Wpa2FrameError::RsnxeMismatch);
                }
                saw_rsnxe = true;
            }
            VENDOR_ELEMENT_ID => {
                if element.len() != 24
                    || element[2..5] != RSN_OUI
                    || element[5] != GTK_KDE_TYPE
                    || element[7] != 0
                    || element[6] & !0x07 != 0
                {
                    return Err(Wpa2FrameError::UnsupportedKeyData);
                }
                if gtk.is_some() {
                    return Err(Wpa2FrameError::DuplicateGtk);
                }
                let mut key = [0; WPA2_GTK_LEN];
                key.copy_from_slice(&element[8..24]);
                gtk = Some(Wpa2Gtk::new(
                    element[6] & 0x03,
                    element[6] & 0x04 != 0,
                    key,
                )?);
            }
            _ => return Err(Wpa2FrameError::UnsupportedKeyData),
        }
        offset += element_len;
    }

    if !saw_rsn {
        return Err(Wpa2FrameError::MissingRsnIe);
    }
    if !expected_rsnxe.is_empty() && !saw_rsnxe {
        return Err(Wpa2FrameError::MissingRsnxe);
    }
    gtk.ok_or(Wpa2FrameError::MissingGtk)
}

/// Parse the encrypted key-data body of a connected-state Group Message 1.
///
/// Unlike pairwise Message 3, a group rekey carries a GTK KDE without a copy
/// of the association RSN IEs. Only the recovered RSN GTK KDE and key-wrap
/// padding are accepted; unrelated elements fail closed.
pub fn parse_group_gtk_key_data(bytes: &[u8]) -> Result<Wpa2Gtk, Wpa2FrameError> {
    let mut offset = 0;
    let mut gtk = None;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.iter().all(|byte| *byte == 0)
            || (remaining[0] == VENDOR_ELEMENT_ID && remaining[1..].iter().all(|byte| *byte == 0))
        {
            break;
        }
        if remaining.len() < 2 {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element_len = remaining[1] as usize + 2;
        if element_len > remaining.len() {
            return Err(Wpa2FrameError::MalformedKeyData);
        }
        let element = &remaining[..element_len];
        if element[0] != VENDOR_ELEMENT_ID
            || element.len() != 24
            || element[2..5] != RSN_OUI
            || element[5] != GTK_KDE_TYPE
            || element[7] != 0
            || element[6] & !0x07 != 0
            || gtk.is_some()
        {
            return Err(if gtk.is_some() {
                Wpa2FrameError::DuplicateGtk
            } else {
                Wpa2FrameError::UnsupportedKeyData
            });
        }
        let mut key = [0; WPA2_GTK_LEN];
        key.copy_from_slice(&element[8..24]);
        gtk = Some(Wpa2Gtk::new(
            element[6] & 0x03,
            element[6] & 0x04 != 0,
            key,
        )?);
        offset += element_len;
    }
    gtk.ok_or(Wpa2FrameError::MissingGtk)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wpa2TxFrame<const N: usize = WPA2_TX_EAPOL_CAPACITY> {
    interface: Wpa2Interface,
    peer: [u8; 6],
    retransmission: bool,
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> Wpa2TxFrame<N> {
    pub fn message1(
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
    ) -> Result<Self, Wpa2FrameError> {
        Self::build(
            Wpa2Interface::AccessPoint,
            peer,
            2,
            KEY_INFO_PAIRWISE | KEY_INFO_ACK | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            WPA2_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            [0; 8],
            &[],
        )
    }

    pub fn message2<const R: usize>(
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        rsn_ie: &OwnedRsnIe<R>,
    ) -> Result<Self, Wpa2FrameError> {
        let security_ies = OwnedAssociationSecurityIes::<R>::try_copy(rsn_ie, &[])?;
        Self::message2_with_security_ies(peer, replay_counter, supplicant_nonce, &security_ies)
    }

    pub fn message2_with_security_ies<const R: usize>(
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        security_ies: &OwnedAssociationSecurityIes<R>,
    ) -> Result<Self, Wpa2FrameError> {
        Self::build(
            Wpa2Interface::Station,
            peer,
            1,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            0,
            replay_counter,
            supplicant_nonce,
            [0; 8],
            security_ies.as_bytes(),
        )
    }

    pub fn message3(
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, Wpa2FrameError> {
        if encrypted_key_data.is_empty() {
            return Err(Wpa2FrameError::EmptyKeyData);
        }
        Self::build(
            Wpa2Interface::AccessPoint,
            peer,
            2,
            KEY_INFO_PAIRWISE
                | KEY_INFO_INSTALL
                | KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            WPA2_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            key_rsc,
            encrypted_key_data,
        )
    }

    pub fn message4(peer: [u8; 6], replay_counter: u64) -> Result<Self, Wpa2FrameError> {
        Self::build(
            Wpa2Interface::Station,
            peer,
            1,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_INFO_SECURE | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            0,
            replay_counter,
            [0; 32],
            [0; 8],
            &[],
        )
    }

    /// Build the station response to one connected-state Group Message 1.
    pub fn group_message2(peer: [u8; 6], replay_counter: u64) -> Result<Self, Wpa2FrameError> {
        Self::build(
            Wpa2Interface::Station,
            peer,
            1,
            KEY_INFO_MIC | KEY_INFO_SECURE | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            0,
            replay_counter,
            [0; 32],
            [0; 8],
            &[],
        )
    }

    /// Build an authenticator Group Message 1 for protocol tests and the
    /// future AP authenticator. The caller supplies RFC3394-wrapped GTK data.
    pub fn group_message1(
        peer: [u8; 6],
        replay_counter: u64,
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, Wpa2FrameError> {
        if encrypted_key_data.is_empty() {
            return Err(Wpa2FrameError::EmptyKeyData);
        }
        Self::build(
            Wpa2Interface::AccessPoint,
            peer,
            2,
            KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | KEY_DESCRIPTOR_VERSION_HMAC_SHA1,
            WPA2_GTK_LEN as u16,
            replay_counter,
            [0; 32],
            key_rsc,
            encrypted_key_data,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        interface: Wpa2Interface,
        peer: [u8; 6],
        protocol_version: u8,
        key_info: u16,
        key_length: u16,
        replay_counter: u64,
        nonce: [u8; 32],
        key_rsc: [u8; 8],
        key_data: &[u8],
    ) -> Result<Self, Wpa2FrameError> {
        if key_info & (KEY_INFO_PAIRWISE | KEY_INFO_ACK) == (KEY_INFO_PAIRWISE | KEY_INFO_ACK)
            && nonce.iter().all(|byte| *byte == 0)
        {
            return Err(Wpa2FrameError::ZeroNonce);
        }
        let len = EAPOL_KEY_PACKET_LEN
            .checked_add(key_data.len())
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        let body_len = EAPOL_KEY_FIXED_LEN
            .checked_add(key_data.len())
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        if len > N || body_len > u16::MAX as usize || key_data.len() > u16::MAX as usize {
            return Err(Wpa2FrameError::CapacityExceeded);
        }

        let mut bytes = [0; N];
        bytes[0] = protocol_version;
        bytes[1] = EAPOL_PACKET_TYPE_KEY;
        bytes[2..4].copy_from_slice(&(body_len as u16).to_be_bytes());
        bytes[4] = RSN_KEY_DESCRIPTOR_TYPE;
        bytes[5..7].copy_from_slice(&key_info.to_be_bytes());
        bytes[7..9].copy_from_slice(&key_length.to_be_bytes());
        bytes[9..17].copy_from_slice(&replay_counter.to_be_bytes());
        bytes[17..49].copy_from_slice(&nonce);
        bytes[65..73].copy_from_slice(&key_rsc);
        bytes[97..99].copy_from_slice(&(key_data.len() as u16).to_be_bytes());
        bytes[EAPOL_KEY_PACKET_LEN..len].copy_from_slice(key_data);
        Ok(Self {
            interface,
            peer,
            retransmission: false,
            len,
            bytes,
        })
    }

    pub const fn interface(&self) -> Wpa2Interface {
        self.interface
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    /// Semantic retry classification supplied by the WPA state transition.
    /// EAPOL-Key does not carry a standalone retransmission flag, so keeping
    /// this metadata prevents the TX-complete owner from guessing from bytes.
    pub const fn retransmission(&self) -> bool {
        self.retransmission
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn key_frame(&self) -> EapolKeyFrame<'_> {
        EapolKeyFrame::parse(self.as_bytes()).expect("Wpa2TxFrame is validated on construction")
    }

    /// Authenticate a supplicant or authenticator action with the pairwise
    /// KCK. The builder always initializes the MIC field to zero; clearing it
    /// here as well makes repeated authentication deterministic.
    pub fn authenticate(mut self, ptk: &Ptk) -> Self {
        self.authenticate_with_kck(ptk.kck());
        self
    }

    pub(crate) fn authenticate_with_confirmation_key(
        mut self,
        key: &Wpa2KeyConfirmationKey,
    ) -> Self {
        self.authenticate_with_kck(key.as_bytes());
        self
    }

    fn authenticate_with_kck(&mut self, kck: &[u8; 16]) {
        self.set_mic(&[0; 16]);
        let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(kck)
            .expect("WPA2 KCK length is always accepted by HMAC");
        mac.update(self.as_bytes());
        let digest = mac.finalize().into_bytes();
        self.set_mic(
            digest[..16]
                .try_into()
                .expect("SHA-1 MIC prefix is 16 bytes"),
        );
    }

    pub(crate) const fn mark_retransmission(mut self) -> Self {
        self.retransmission = true;
        self
    }

    fn set_mic(&mut self, mic: &[u8; 16]) {
        self.bytes[81..97].copy_from_slice(mic);
    }
}

pub fn build_sta_action_frame<const N: usize, const R: usize>(
    state: &Wpa2StaState,
    transmit: Wpa2Transmit,
    security_ies: &OwnedAssociationSecurityIes<R>,
) -> Result<Wpa2TxFrame<N>, Wpa2FrameError> {
    let mut frame = match transmit.message {
        Wpa2TxMessage::PairwiseMessage2 => Wpa2TxFrame::message2_with_security_ies(
            *state.peer(),
            transmit.replay_counter,
            *state.supplicant_nonce(),
            security_ies,
        ),
        Wpa2TxMessage::PairwiseMessage4 => {
            Wpa2TxFrame::message4(*state.peer(), transmit.replay_counter)
        }
        Wpa2TxMessage::PairwiseMessage1 | Wpa2TxMessage::PairwiseMessage3 => {
            Err(Wpa2FrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

pub fn build_ap_action_frame<const N: usize>(
    state: &Wpa2ApState,
    transmit: Wpa2Transmit,
    key_rsc: [u8; 8],
    encrypted_key_data: &[u8],
) -> Result<Wpa2TxFrame<N>, Wpa2FrameError> {
    let mut frame = match transmit.message {
        Wpa2TxMessage::PairwiseMessage1 => Wpa2TxFrame::message1(
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
        ),
        Wpa2TxMessage::PairwiseMessage3 => Wpa2TxFrame::message3(
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
            key_rsc,
            encrypted_key_data,
        ),
        Wpa2TxMessage::PairwiseMessage2 | Wpa2TxMessage::PairwiseMessage4 => {
            Err(Wpa2FrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wpa2EthernetFrame<const N: usize = WPA2_TX_ETHERNET_CAPACITY> {
    interface: Wpa2Interface,
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> Wpa2EthernetFrame<N> {
    pub fn build<const E: usize>(
        local: [u8; 6],
        eapol: &Wpa2TxFrame<E>,
    ) -> Result<Self, Wpa2FrameError> {
        let len = 14_usize
            .checked_add(eapol.as_bytes().len())
            .ok_or(Wpa2FrameError::CapacityExceeded)?;
        if len > N {
            return Err(Wpa2FrameError::CapacityExceeded);
        }
        let mut bytes = [0; N];
        bytes[..6].copy_from_slice(eapol.peer());
        bytes[6..12].copy_from_slice(&local);
        bytes[12..14].copy_from_slice(&EAPOL_ETHERTYPE);
        bytes[14..len].copy_from_slice(eapol.as_bytes());
        Ok(Self {
            interface: eapol.interface(),
            len,
            bytes,
        })
    }

    pub const fn interface(&self) -> Wpa2Interface {
        self.interface
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EapolKeyMessage;
    use crate::state::Wpa2ApAction;

    fn rsn_ie() -> OwnedRsnIe<22> {
        let mut bytes = [0; 22];
        bytes[0] = RSN_ELEMENT_ID;
        bytes[1] = 20;
        OwnedRsnIe::try_copy(&bytes).unwrap()
    }

    fn association_ies() -> OwnedAssociationSecurityIes<22> {
        OwnedAssociationSecurityIes::try_copy(&rsn_ie(), &[]).unwrap()
    }

    #[test]
    fn contiguous_association_security_ies_retain_exact_rsn_and_rsnxe() {
        let rsn = rsn_ie();
        let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
        let mut bytes = [0_u8; 26];
        bytes[..22].copy_from_slice(rsn.as_bytes());
        bytes[22..].copy_from_slice(&rsnxe);
        let owned = OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes).unwrap();
        assert_eq!(owned.as_bytes(), &bytes);
        assert_eq!(owned.rsn_ie(), rsn.as_bytes());

        bytes[23] = 3;
        assert_eq!(
            OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes),
            Err(Wpa2FrameError::InvalidRsnxe)
        );
    }

    #[test]
    fn gtk_key_data_round_trips_with_key_wrap_padding() {
        let rsn = rsn_ie();
        let gtk = Wpa2Gtk::new(2, false, [0x5a; 16]).unwrap();
        let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        assert_eq!(data.as_bytes().len() % 8, 0);
        let parsed = parse_gtk_key_data(data.as_bytes(), rsn.as_bytes(), &[]).unwrap();
        assert_eq!(parsed.key_id(), 2);
        assert!(!parsed.transmit());
        assert_eq!(parsed.key(), &[0x5a; 16]);
    }

    #[test]
    fn builders_produce_classified_m1_to_m4() {
        let rsn = rsn_ie();
        let m1 = Wpa2TxFrame::<128>::message1([2; 6], 7, [3; 32]).unwrap();
        let m2 = Wpa2TxFrame::<128>::message2([1; 6], 7, [4; 32], &rsn).unwrap();
        let m3 = Wpa2TxFrame::<128>::message3([2; 6], 8, [3; 32], [5; 8], &[6; 24]).unwrap();
        let m4 = Wpa2TxFrame::<128>::message4([1; 6], 8).unwrap();
        assert_eq!(m1.key_frame().message(), EapolKeyMessage::PairwiseMessage1);
        assert_eq!(m2.key_frame().message(), EapolKeyMessage::PairwiseMessage2);
        assert_eq!(m3.key_frame().message(), EapolKeyMessage::PairwiseMessage3);
        assert!(m3.key_frame().key_info().encrypted_key_data());
        assert_eq!(m4.key_frame().message(), EapolKeyMessage::PairwiseMessage4);
        assert_eq!(m1.key_frame().key_length(), WPA2_GTK_LEN as u16);
        assert_eq!(m2.key_frame().key_length(), 0);
        assert_eq!(m3.key_frame().key_length(), WPA2_GTK_LEN as u16);
        assert_eq!(m4.key_frame().key_length(), 0);
        assert_eq!(m1.key_frame().protocol_version(), 2);
        assert_eq!(m2.key_frame().protocol_version(), 1);
        assert_eq!(m3.key_frame().protocol_version(), 2);
        assert_eq!(m4.key_frame().protocol_version(), 1);
    }

    #[test]
    fn state_actions_are_bound_to_role_peer_and_nonce_context() {
        let security_ies = association_ies();
        let sta = Wpa2StaState::new([1; 6], [2; 6], [3; 32]).unwrap();
        let m2_action = Wpa2Transmit {
            message: Wpa2TxMessage::PairwiseMessage2,
            replay_counter: 7,
            retransmission: false,
        };
        let m2 = build_sta_action_frame::<128, _>(&sta, m2_action, &security_ies).unwrap();
        assert_eq!(m2.peer(), &[2; 6]);
        assert_eq!(m2.key_frame().nonce(), &[3; 32]);
        assert_eq!(
            build_ap_action_frame::<128>(
                &Wpa2ApState::new([2; 6], [1; 6], [4; 32], 7).unwrap(),
                m2_action,
                [0; 8],
                &[]
            )
            .err(),
            Some(Wpa2FrameError::UnexpectedTransmitAction)
        );

        let ap = Wpa2ApState::new([2; 6], [1; 6], [4; 32], 7).unwrap();
        let Wpa2ApAction::Transmit(m1_action) = ap.message1(false).unwrap() else {
            panic!("message1 must produce a transmit action")
        };
        let m1 = build_ap_action_frame::<128>(&ap, m1_action, [0; 8], &[]).unwrap();
        assert_eq!(m1.peer(), &[1; 6]);
        assert_eq!(m1.key_frame().nonce(), &[4; 32]);
    }

    #[test]
    fn ethernet_builder_owns_complete_eapol_frame() {
        let mut m4 = Wpa2TxFrame::<128>::message4([1; 6], 9).unwrap();
        m4.set_mic(&[0xa5; 16]);
        let ethernet = Wpa2EthernetFrame::<160>::build([2; 6], &m4).unwrap();
        assert_eq!(&ethernet.as_bytes()[..6], &[1; 6]);
        assert_eq!(&ethernet.as_bytes()[6..12], &[2; 6]);
        assert_eq!(&ethernet.as_bytes()[12..14], &EAPOL_ETHERTYPE);
        assert_eq!(&ethernet.as_bytes()[14 + 81..14 + 97], &[0xa5; 16]);
    }

    #[test]
    fn parser_rejects_changed_rsn_ie_and_duplicate_gtk() {
        let rsn = rsn_ie();
        let gtk = Wpa2Gtk::new(1, false, [7; 16]).unwrap();
        let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let mut other = [0; 22];
        other[0] = RSN_ELEMENT_ID;
        other[1] = 20;
        other[2] = 1;
        assert_eq!(
            parse_gtk_key_data(data.as_bytes(), &other, &[]).err(),
            Some(Wpa2FrameError::RsnIeMismatch)
        );

        let mut duplicate = [0; 70];
        let source = data.as_bytes();
        duplicate[..46].copy_from_slice(&source[..46]);
        duplicate[46..70].copy_from_slice(&source[22..46]);
        assert_eq!(
            parse_gtk_key_data(&duplicate, rsn.as_bytes(), &[]).err(),
            Some(Wpa2FrameError::DuplicateGtk)
        );
    }

    #[test]
    fn parser_validates_authenticator_rsnxe_without_ignoring_unknown_elements() {
        let rsn = rsn_ie();
        let gtk = Wpa2Gtk::new(1, false, [7; 16]).unwrap();
        let data = Wpa2PlainKeyData::<64>::build(&rsn, &gtk).unwrap();
        let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
        let mut with_rsnxe = [0; 64];
        let source = data.as_bytes();
        with_rsnxe[..22].copy_from_slice(&source[..22]);
        with_rsnxe[22..26].copy_from_slice(&rsnxe);
        with_rsnxe[26..50].copy_from_slice(&source[22..46]);
        with_rsnxe[50] = VENDOR_ELEMENT_ID;

        let parsed = parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &rsnxe).unwrap();
        assert_eq!(parsed.key_id(), 1);
        assert_eq!(parsed.key(), &[7; 16]);

        assert_eq!(
            parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &[]).err(),
            Some(Wpa2FrameError::UnexpectedRsnxe)
        );
        assert_eq!(
            parse_gtk_key_data(source, rsn.as_bytes(), &rsnxe).err(),
            Some(Wpa2FrameError::MissingRsnxe)
        );

        let changed = [RSNXE_ELEMENT_ID, 2, 0x21, 0x00];
        assert_eq!(
            parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &changed).err(),
            Some(Wpa2FrameError::RsnxeMismatch)
        );
    }
}
