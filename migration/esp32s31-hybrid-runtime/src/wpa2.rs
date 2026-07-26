//! Allocation-free WPA2-Personal EAPOL ingress boundary.
//!
//! This module does not call the vendor supplicant. It copies a complete
//! EAPOL-Key packet into Rust-owned fixed storage and transfers ownership to
//! the single radio executor. Cryptographic and key-install transitions are
//! intentionally separate; no synchronous callback is allowed to wait for
//! them here.

use crate::channel::{BoundedChannel, Receive, TrySendError};

pub const EAPOL_HEADER_LEN: usize = 4;
pub const EAPOL_KEY_FIXED_LEN: usize = 95;
pub const EAPOL_KEY_PACKET_LEN: usize = EAPOL_HEADER_LEN + EAPOL_KEY_FIXED_LEN;
pub const EAPOL_PACKET_TYPE_KEY: u8 = 3;
pub const RSN_KEY_DESCRIPTOR_TYPE: u8 = 2;
pub const DEFAULT_EAPOL_FRAME_CAPACITY: usize = 512;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2Interface {
    Station,
    AccessPoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolParseError {
    Truncated,
    LengthMismatch,
    NotKeyPacket,
    NotRsnKeyDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolCopyError {
    CapacityExceeded,
    Invalid(EapolParseError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2IngressError {
    CapacityExceeded,
    Invalid(EapolParseError),
    QueueFull,
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

/// A validated borrowed view of one complete RSN EAPOL-Key packet.
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
}

/// A complete EAPOL-Key packet copied out of the vendor RX callback.
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
        EapolKeyFrame::parse(self.as_bytes()).expect("OwnedEapolFrame is validated on creation")
    }
}

/// Fixed-capacity multi-producer ingress with one async consumer.
///
/// Producers never wait. `receive` is wake-driven and performs no periodic
/// polling or delay; the executor polls it only after a producer wakes it.
pub struct Wpa2Ingress<const Q: usize, const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    frames: BoundedChannel<OwnedEapolFrame<N>, Q>,
}

impl<const Q: usize, const N: usize> Wpa2Ingress<Q, N> {
    pub const fn new() -> Self {
        Self {
            frames: BoundedChannel::new(),
        }
    }

    pub fn try_push(
        &self,
        interface: Wpa2Interface,
        peer: [u8; 6],
        bytes: &[u8],
    ) -> Result<(), Wpa2IngressError> {
        let frame =
            OwnedEapolFrame::try_copy(interface, peer, bytes).map_err(|error| match error {
                EapolCopyError::CapacityExceeded => Wpa2IngressError::CapacityExceeded,
                EapolCopyError::Invalid(error) => Wpa2IngressError::Invalid(error),
            })?;
        self.frames
            .try_send(frame)
            .map_err(|TrySendError(_)| Wpa2IngressError::QueueFull)
    }

    pub fn try_receive(&self) -> Option<OwnedEapolFrame<N>> {
        self.frames.try_receive()
    }

    pub fn receive(&self) -> Receive<'_, OwnedEapolFrame<N>, Q> {
        self.frames.receive()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }
}

impl<const Q: usize, const N: usize> Default for Wpa2Ingress<Q, N> {
    fn default() -> Self {
        Self::new()
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
    fn ingress_owns_frames_and_fails_immediately_when_full() {
        let ingress = Wpa2Ingress::<2, EAPOL_KEY_PACKET_LEN>::new();
        let mut packet = key_packet(KEY_INFO_PAIRWISE | KEY_INFO_ACK | 2, 3);
        ingress
            .try_push(Wpa2Interface::Station, [1, 2, 3, 4, 5, 6], &packet)
            .unwrap();
        packet[16] = 9;
        ingress
            .try_push(Wpa2Interface::Station, [0; 6], &packet)
            .unwrap();

        assert_eq!(
            ingress.try_push(Wpa2Interface::Station, [0; 6], &packet),
            Err(Wpa2IngressError::QueueFull)
        );
        let owned = ingress.try_receive().unwrap();
        assert_eq!(owned.key_frame().replay_counter(), 3);
        assert_eq!(owned.peer(), &[1, 2, 3, 4, 5, 6]);
    }
}
