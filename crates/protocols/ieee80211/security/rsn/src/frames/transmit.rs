//! Bounded EAPOL/Ethernet transmission and typed handshake action encoding.

use super::*;
use crate::{AkmKeys, HandshakeSuite};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnTxFrame<const N: usize = RSN_TX_EAPOL_CAPACITY> {
    interface: RsnInterface,
    peer: [u8; 6],
    retransmission: bool,
    len: usize,
    bytes: [u8; N],
    suite: crate::akm::SuiteIdentity,
}

impl<const N: usize> RsnTxFrame<N> {
    pub fn message1(
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            akm.identity(),
            2,
            KEY_INFO_PAIRWISE | KEY_INFO_ACK | u16::from(akm.eapol_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            [0; 8],
            &[],
        )
    }

    pub fn message2<const R: usize>(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        rsn_ie: &OwnedRsnIe<R>,
    ) -> Result<Self, RsnFrameError> {
        let security_ies = OwnedAssociationSecurityIes::<R>::try_copy(rsn_ie, &[])?;
        Self::message2_with_security_ies(akm, peer, replay_counter, supplicant_nonce, &security_ies)
    }

    pub fn message2_with_security_ies<const R: usize>(
        akm: Akm,
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        security_ies: &OwnedAssociationSecurityIes<R>,
    ) -> Result<Self, RsnFrameError> {
        Self::message2_with_key_data(
            akm,
            peer,
            replay_counter,
            supplicant_nonce,
            security_ies.as_bytes(),
        )
    }

    pub fn message2_with_key_data(
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
        supplicant_nonce: [u8; 32],
        key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            akm.identity(),
            1,
            KEY_INFO_PAIRWISE | KEY_INFO_MIC | u16::from(akm.eapol_descriptor_version()),
            0,
            replay_counter,
            supplicant_nonce,
            [0; 8],
            key_data,
        )
    }

    pub fn message3(
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
        authenticator_nonce: [u8; 32],
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            akm.identity(),
            2,
            KEY_INFO_PAIRWISE
                | KEY_INFO_INSTALL
                | KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | u16::from(akm.eapol_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            authenticator_nonce,
            key_rsc,
            encrypted_key_data,
        )
    }

    pub fn message4(
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            akm.identity(),
            1,
            KEY_INFO_PAIRWISE
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | u16::from(akm.eapol_descriptor_version()),
            0,
            replay_counter,
            [0; 32],
            [0; 8],
            &[],
        )
    }

    /// Build the station response to one connected-state Group Message 1.
    pub fn group_message2(
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
    ) -> Result<Self, RsnFrameError> {
        Self::build(
            RsnInterface::Station,
            peer,
            akm.identity(),
            1,
            KEY_INFO_MIC | KEY_INFO_SECURE | u16::from(akm.eapol_descriptor_version()),
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
        akm: impl HandshakeSuite,
        peer: [u8; 6],
        replay_counter: u64,
        key_rsc: [u8; 8],
        encrypted_key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if encrypted_key_data.is_empty() {
            return Err(RsnFrameError::EmptyKeyData);
        }
        Self::build(
            RsnInterface::AccessPoint,
            peer,
            akm.identity(),
            2,
            KEY_INFO_ACK
                | KEY_INFO_MIC
                | KEY_INFO_SECURE
                | KEY_INFO_ENCRYPTED_KEY_DATA
                | u16::from(akm.eapol_descriptor_version()),
            RSN_GTK_LEN as u16,
            replay_counter,
            [0; 32],
            key_rsc,
            encrypted_key_data,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        interface: RsnInterface,
        peer: [u8; 6],
        suite: crate::akm::SuiteIdentity,
        protocol_version: u8,
        key_info: u16,
        key_length: u16,
        replay_counter: u64,
        nonce: [u8; 32],
        key_rsc: [u8; 8],
        key_data: &[u8],
    ) -> Result<Self, RsnFrameError> {
        if key_info & (KEY_INFO_PAIRWISE | KEY_INFO_ACK) == (KEY_INFO_PAIRWISE | KEY_INFO_ACK)
            && nonce.iter().all(|byte| *byte == 0)
        {
            return Err(RsnFrameError::ZeroNonce);
        }
        let mic_length = suite.mic_length();
        let prefix_len = mic_length.packet_prefix_len();
        let len = prefix_len
            .checked_add(key_data.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let body_len = (prefix_len - crate::EAPOL_HEADER_LEN)
            .checked_add(key_data.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N || body_len > u16::MAX as usize || key_data.len() > u16::MAX as usize {
            return Err(RsnFrameError::CapacityExceeded);
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
        bytes[mic_length.end()..prefix_len].copy_from_slice(&(key_data.len() as u16).to_be_bytes());
        bytes[prefix_len..len].copy_from_slice(key_data);
        Ok(Self {
            interface,
            peer,
            retransmission: false,
            len,
            bytes,
            suite,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
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
        EapolKeyFrame::parse_with_mic_length(self.as_bytes(), self.suite.mic_length())
            .expect("RsnTxFrame is validated on construction")
    }

    /// Authenticate a supplicant or authenticator action with the pairwise
    /// KCK. The builder always initializes the MIC field to zero; clearing it
    /// here as well makes repeated authentication deterministic.
    pub fn authenticate(mut self, ptk: &Ptk) -> Result<Self, RsnFrameError> {
        self.authenticate_with_kck(ptk.akm(), ptk.kck())?;
        Ok(self)
    }

    pub(crate) fn authenticate_with_confirmation_key(
        mut self,
        key: &RsnKeyConfirmationKey,
    ) -> Result<Self, RsnFrameError> {
        self.authenticate_with_kck(key.akm(), key.as_bytes())?;
        Ok(self)
    }

    fn authenticate_with_kck(
        &mut self,
        akm: Akm,
        kck: &[u8; RSN_KCK_LEN],
    ) -> Result<(), RsnFrameError> {
        if self.suite != akm.identity() {
            return Err(RsnFrameError::WrongCryptoSuite);
        }
        self.authenticate_with_mic(akm.mic(kck))
    }

    /// Authenticate an FT EAPOL frame with its FT-specific KCK.
    /// The negotiated suite and peer must match the key's derivation context.
    pub fn authenticate_ft(mut self, ptk: &crate::ft::FtPtk) -> Result<Self, RsnFrameError> {
        let expected_peer = match self.interface {
            RsnInterface::Station => ptk.context().addresses.access_point,
            RsnInterface::AccessPoint => ptk.context().addresses.station,
        };
        if self.peer != expected_peer
            || self.suite != ptk.akm().identity()
            || self.key_frame().key_info().descriptor_version()
                != ptk.akm().eapol_descriptor_version()
        {
            return Err(RsnFrameError::UnexpectedTransmitAction);
        }
        self.authenticate_with_mic(crate::akm::EapolMic::aes_cmac(ptk.kck()))?;
        Ok(self)
    }

    fn authenticate_with_mic(
        &mut self,
        mut mac: crate::akm::EapolMic,
    ) -> Result<(), RsnFrameError> {
        if self.suite.mic_length() != crate::eapol::KeyMicLength::Octets16 {
            return Err(RsnFrameError::WrongMicLength);
        }
        self.set_mic(&[0; RSN_MIC_LEN]);
        mac.update(self.as_bytes());
        self.set_mic(&mac.finalize());
        Ok(())
    }

    /// OWE keys must match the peer, AKM-defined descriptor and full MIC
    /// geometry; a legacy 16-octet key cannot authenticate a wider packet.
    pub fn authenticate_owe(mut self, ptk: &crate::owe::OwePtk) -> Result<Self, RsnFrameError> {
        let expected = match self.interface {
            RsnInterface::Station => ptk.addresses().access_point,
            RsnInterface::AccessPoint => ptk.addresses().station,
        };
        if self.peer != expected {
            return Err(RsnFrameError::UnexpectedTransmitAction);
        }
        if self.suite != ptk.group().identity() {
            return Err(RsnFrameError::WrongCryptoSuite);
        }
        ptk.authenticate_eapol(&mut self.bytes[..self.len])
            .map_err(|_| RsnFrameError::WrongCryptoSuite)?;
        Ok(self)
    }

    pub(crate) const fn mark_retransmission(mut self) -> Self {
        self.retransmission = true;
        self
    }

    fn set_mic(&mut self, mic: &[u8; RSN_MIC_LEN]) {
        self.bytes[crate::eapol::EAPOL_KEY_MIC_START..crate::eapol::KeyMicLength::Octets16.end()]
            .copy_from_slice(mic);
    }
}

pub fn build_sta_action_frame<const N: usize, const R: usize>(
    state: &RsnStaState,
    transmit: RsnTransmit,
    security_ies: &OwnedAssociationSecurityIes<R>,
) -> Result<RsnTxFrame<N>, RsnFrameError> {
    let mut frame = match transmit.message {
        RsnTxMessage::PairwiseMessage2 => RsnTxFrame::message2_with_security_ies(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.supplicant_nonce(),
            security_ies,
        ),
        RsnTxMessage::PairwiseMessage4 => {
            RsnTxFrame::message4(state.akm(), *state.peer(), transmit.replay_counter)
        }
        RsnTxMessage::PairwiseMessage1 | RsnTxMessage::PairwiseMessage3 => {
            Err(RsnFrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

pub fn build_ap_action_frame<const N: usize>(
    state: &RsnApState,
    transmit: RsnTransmit,
    key_rsc: [u8; 8],
    encrypted_key_data: &[u8],
) -> Result<RsnTxFrame<N>, RsnFrameError> {
    let mut frame = match transmit.message {
        RsnTxMessage::PairwiseMessage1 => RsnTxFrame::message1(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
        ),
        RsnTxMessage::PairwiseMessage3 => RsnTxFrame::message3(
            state.akm(),
            *state.peer(),
            transmit.replay_counter,
            *state.authenticator_nonce(),
            key_rsc,
            encrypted_key_data,
        ),
        RsnTxMessage::PairwiseMessage2 | RsnTxMessage::PairwiseMessage4 => {
            Err(RsnFrameError::UnexpectedTransmitAction)
        }
    }?;
    frame.retransmission = transmit.retransmission;
    Ok(frame)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RsnEthernetFrame<const N: usize = RSN_TX_ETHERNET_CAPACITY> {
    interface: RsnInterface,
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> RsnEthernetFrame<N> {
    pub fn build<const E: usize>(
        local: [u8; 6],
        eapol: &RsnTxFrame<E>,
    ) -> Result<Self, RsnFrameError> {
        let len = 14_usize
            .checked_add(eapol.as_bytes().len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        if len > N {
            return Err(RsnFrameError::CapacityExceeded);
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

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

#[cfg(test)]
mod tests;
