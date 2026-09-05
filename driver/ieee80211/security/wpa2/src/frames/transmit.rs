//! Bounded EAPOL/Ethernet transmission and typed handshake action encoding.

use super::*;

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
mod tests;
