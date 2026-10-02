//! Software CCMP of individually addressed robust management frames.
//!
//! With management frame protection, an individually addressed
//! Deauthentication, Disassociation or robust Action frame uses CCMP under
//! the pairwise temporal key. Software transmission borrows the key owner's
//! existing packet-number allocator; software reception retains a distinct
//! management replay frontier. Neither primitive admits a peer or dispatches
//! the decrypted action. The caller validates the normal management header,
//! addresses and robust subtype before using these primitives.
//!
//! The nonce of a management frame sets the Management flag with priority
//! zero, its additional authentication data keeps the Frame Control subtype,
//! and its packet numbers form one replay counter apart from every data TID.
//!
//! SOURCE(esp32s31): IEEE 802.11-2016 12.5.3 (CCMP); complete pinned
//! `libnet80211.a[ieee80211_sta.o]::sta_recv_mgmt` and
//! `libnet80211.a[ieee80211_crypto_ccmp.o]::ieee80211_ccmp_decrypt`.

use aes::{
    Aes128,
    cipher::{
        Block, BlockEncrypt, BlockSizeUser, KeyInit, generic_array::GenericArray, typenum::Unsigned,
    },
};
use oer_ieee80211_mac::{
    ccmp::{CCMP_HEADER_LEN, CcmpHeader, CcmpHeaderError, CcmpKeyId, CcmpTxPacketNumber},
    management::MANAGEMENT_HEADER_LEN,
    security::CCMP_128_KEY_LEN,
};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

const MIC_LEN: usize = 8;
const AES_BLOCK_LEN: usize = <Aes128 as BlockSizeUser>::BlockSize::USIZE;
/// This CCMP profile encodes CCM body lengths and counters in two octets.
const CCM_LENGTH_LEN: usize = 2;
const PROTECTED: u8 = 0x40;
/// Frame Control flags CCMP leaves out of its additional authentication
/// data: Retry, Power Management and More Data.
const AAD_MASKED_FLAGS: u8 = 0x08 | 0x10 | 0x20;
/// Nonce flags of a management frame: priority zero, Management set.
const MANAGEMENT_NONCE_FLAGS: u8 = 0x10;
/// Management AAD omits Duration but keeps FC, three addresses and SC.
const AAD_LEN: usize = MANAGEMENT_HEADER_LEN - 2;
const NONCE_LEN: usize = AES_BLOCK_LEN - 1 - CCM_LENGTH_LEN;
/// CCM flags of the first authentication block: Adata, an 8-byte MIC and
/// a 2-byte length field.
const B0_FLAGS: u8 = 0x40 | (((MIC_LEN as u8 - 2) / 2) << 3) | COUNTER_FLAGS;
/// CCM flags of every counter block: a 2-byte counter.
const COUNTER_FLAGS: u8 = (CCM_LENGTH_LEN - 1) as u8;

/// Why a protected management frame did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementCcmpError {
    /// Shorter than a header, CCMP header and MIC.
    Truncated,
    /// The Protected Frame bit is clear.
    NotProtected,
    /// The CCMP header lacks its Extended IV bit.
    MissingExtendedIv,
    /// A reserved CCMP header field is nonzero.
    InvalidCcmpHeader(CcmpHeaderError),
    /// The ordinary pairwise management key uses Key ID zero.
    UnexpectedKeyId,
    /// The body cannot be represented by CCM's two-octet length field.
    BodyTooLong,
    /// The packet number does not exceed the last accepted one.
    Replay,
    /// The MIC does not verify.
    InvalidMic,
}

/// The receive state of protected management frames under one pairwise key.
/// Keep this owner across redelivery of the same installed key; constructing
/// another receiver would discard its management replay frontier.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ManagementCcmpReceiver {
    temporal_key: [u8; CCMP_128_KEY_LEN],
    last_packet_number: Option<u64>,
}

impl ManagementCcmpReceiver {
    pub fn new(temporal_key: [u8; CCMP_128_KEY_LEN]) -> Self {
        Self {
            temporal_key,
            last_packet_number: None,
        }
    }

    /// Decrypt one received protected management frame in place and return
    /// its plaintext body. A frame that does not verify leaves the replay
    /// state unchanged. The caller admits a normal 24-octet management
    /// header, individual destination, peer/BSS identity and robust subtype.
    /// This primitive does not authorize the sender. On any error, discard
    /// the buffer: failed MIC verification can leave unauthenticated plaintext.
    pub fn open<'a>(&mut self, frame: &'a mut [u8]) -> Result<&'a [u8], ManagementCcmpError> {
        let body_start = MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN;
        if frame.len() < body_start + MIC_LEN {
            return Err(ManagementCcmpError::Truncated);
        }
        if frame[1] & PROTECTED == 0 {
            return Err(ManagementCcmpError::NotProtected);
        }
        let ccmp = CcmpHeader::parse(
            frame[MANAGEMENT_HEADER_LEN..body_start]
                .try_into()
                .expect("the complete CCMP header was checked above"),
        )
        .map_err(|error| match error {
            CcmpHeaderError::ExtIvMissing => ManagementCcmpError::MissingExtendedIv,
            error => ManagementCcmpError::InvalidCcmpHeader(error),
        })?;
        if ccmp.key_id() != CcmpKeyId::PAIRWISE {
            return Err(ManagementCcmpError::UnexpectedKeyId);
        }
        let packet_number = ccmp.packet_number().value();
        if self
            .last_packet_number
            .is_some_and(|last| packet_number <= last)
        {
            return Err(ManagementCcmpError::Replay);
        }

        let mic_start = frame.len() - MIC_LEN;
        let body_len =
            u16::try_from(mic_start - body_start).map_err(|_| ManagementCcmpError::BodyTooLong)?;
        let (aad, nonce) = authentication_context(frame, packet_number);
        let cipher = Aes128::new(GenericArray::from_slice(&self.temporal_key));
        apply_keystream(&cipher, &nonce, &mut frame[body_start..mic_start]);
        let mut expected = authentication_tag(
            &cipher,
            &nonce,
            &aad,
            &frame[body_start..mic_start],
            body_len,
        );
        let valid = bool::from(expected.ct_eq(&frame[mic_start..]));
        expected.zeroize();
        if !valid {
            // Restore nothing: the caller drops a frame that failed.
            return Err(ManagementCcmpError::InvalidMic);
        }
        self.last_packet_number = Some(packet_number);
        Ok(&frame[body_start..mic_start])
    }
}

/// Why an individually addressed management frame cannot be protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementCcmpTransmitError {
    /// The supplied length lacks a management header or exceeds storage.
    InvalidLength,
    /// Storage has no room for the CCMP header and MIC.
    OutputTooSmall,
    /// The body cannot be represented by CCM's two-octet length field.
    BodyTooLong,
    /// The key's transmit PN space is exhausted and the key must be replaced.
    PacketNumberExhausted,
}

/// A zeroizing pairwise key for software management-frame transmission.
///
/// Packet numbers remain with the existing MAC key owner. Each operation
/// borrows its unique [`CcmpTxPacketNumber`] instead of maintaining a second
/// counter here. The caller must use the allocator belonging to this key,
/// keep it across key redelivery, and reuse encoded bytes for retransmission.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ManagementCcmpTransmitter {
    temporal_key: [u8; CCMP_128_KEY_LEN],
}

impl ManagementCcmpTransmitter {
    pub fn new(temporal_key: [u8; CCMP_128_KEY_LEN]) -> Self {
        Self { temporal_key }
    }

    /// Protect the header and plaintext body in `frame[..length]` in place.
    /// Inserts the CCMP header, sets Protected Frame and appends the MIC.
    /// The caller admits the normal 24-octet header, individual destination,
    /// peer/BSS identity and robust subtype. Failure leaves both the buffer
    /// and allocator unchanged; a successful call consumes exactly one PN.
    pub fn protect(
        &self,
        packet_numbers: &mut CcmpTxPacketNumber,
        frame: &mut [u8],
        length: usize,
    ) -> Result<usize, ManagementCcmpTransmitError> {
        if length < MANAGEMENT_HEADER_LEN || length > frame.len() {
            return Err(ManagementCcmpTransmitError::InvalidLength);
        }
        let body_len = u16::try_from(length - MANAGEMENT_HEADER_LEN)
            .map_err(|_| ManagementCcmpTransmitError::BodyTooLong)?;
        let protected = length
            .checked_add(CCMP_HEADER_LEN + MIC_LEN)
            .ok_or(ManagementCcmpTransmitError::OutputTooSmall)?;
        if protected > frame.len() {
            return Err(ManagementCcmpTransmitError::OutputTooSmall);
        }
        let packet_number = packet_numbers
            .next()
            .map_err(|_| ManagementCcmpTransmitError::PacketNumberExhausted)?;
        let body_start = MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN;
        let mic_start = protected - MIC_LEN;
        frame.copy_within(MANAGEMENT_HEADER_LEN..length, body_start);
        frame[1] |= PROTECTED;
        frame[MANAGEMENT_HEADER_LEN..body_start]
            .copy_from_slice(&CcmpHeader::new(packet_number, CcmpKeyId::PAIRWISE).encode());
        let (aad, nonce) = authentication_context(frame, packet_number.value());
        let cipher = Aes128::new(GenericArray::from_slice(&self.temporal_key));
        let mut tag = authentication_tag(
            &cipher,
            &nonce,
            &aad,
            &frame[body_start..mic_start],
            body_len,
        );
        apply_keystream(&cipher, &nonce, &mut frame[body_start..mic_start]);
        frame[mic_start..protected].copy_from_slice(&tag);
        tag.zeroize();
        Ok(protected)
    }
}

fn authentication_context(frame: &[u8], packet_number: u64) -> ([u8; AAD_LEN], [u8; NONCE_LEN]) {
    let mut aad = [0; AAD_LEN];
    aad[0] = frame[0];
    aad[1] = (frame[1] & !AAD_MASKED_FLAGS) | PROTECTED;
    aad[2..20].copy_from_slice(&frame[4..22]);
    // The sequence number is masked; the fragment number stays.
    aad[20] = frame[22] & 0x0f;
    let mut nonce = [0; NONCE_LEN];
    nonce[0] = MANAGEMENT_NONCE_FLAGS;
    nonce[1..7].copy_from_slice(&frame[10..16]);
    nonce[7..].copy_from_slice(&packet_number.to_be_bytes()[2..]);
    (aad, nonce)
}

/// Encryption and decryption use the same counter-mode XOR. The caller
/// checks the two-octet body length before mutation, so counters fit u16.
fn apply_keystream(cipher: &Aes128, nonce: &[u8; NONCE_LEN], body: &mut [u8]) {
    for (block_index, chunk) in body.chunks_mut(AES_BLOCK_LEN).enumerate() {
        let mut stream = counter_block(nonce, block_index as u16 + 1);
        cipher.encrypt_block(&mut stream);
        for (byte, key) in chunk.iter_mut().zip(stream.iter()) {
            *byte ^= key;
        }
        stream.zeroize();
    }
}

/// Authenticate the common AAD and plaintext without duplicating CCM
/// between transmitter and receiver. `body_len` was checked by their entry
/// points before any buffer mutation.
fn authentication_tag(
    cipher: &Aes128,
    nonce: &[u8; NONCE_LEN],
    aad: &[u8; AAD_LEN],
    body: &[u8],
    body_len: u16,
) -> [u8; MIC_LEN] {
    let mut mac = Block::<Aes128>::default();
    mac[0] = B0_FLAGS;
    mac[1..1 + NONCE_LEN].copy_from_slice(nonce);
    mac[AES_BLOCK_LEN - CCM_LENGTH_LEN..].copy_from_slice(&body_len.to_be_bytes());
    cipher.encrypt_block(&mut mac);
    // CCM prefixes AAD with its two-octet length, then pads to AES blocks.
    let mut aad_blocks = [0_u8; (2 + AAD_LEN).div_ceil(AES_BLOCK_LEN) * AES_BLOCK_LEN];
    aad_blocks[..2].copy_from_slice(&(AAD_LEN as u16).to_be_bytes());
    aad_blocks[2..2 + AAD_LEN].copy_from_slice(aad);
    for block in aad_blocks.chunks(AES_BLOCK_LEN) {
        absorb(cipher, &mut mac, block);
    }
    for block in body.chunks(AES_BLOCK_LEN) {
        absorb(cipher, &mut mac, block);
    }
    let mut first = counter_block(nonce, 0);
    cipher.encrypt_block(&mut first);
    let mut tag = [0; MIC_LEN];
    for (index, byte) in tag.iter_mut().enumerate() {
        *byte = mac[index] ^ first[index];
    }
    first.zeroize();
    mac.zeroize();
    tag
}

fn counter_block(nonce: &[u8; NONCE_LEN], counter: u16) -> Block<Aes128> {
    let mut block = Block::<Aes128>::default();
    block[0] = COUNTER_FLAGS;
    block[1..1 + NONCE_LEN].copy_from_slice(nonce);
    block[AES_BLOCK_LEN - CCM_LENGTH_LEN..].copy_from_slice(&counter.to_be_bytes());
    block
}

/// One CBC-MAC step over a block padded with zeros.
fn absorb(cipher: &Aes128, mac: &mut Block<Aes128>, block: &[u8]) {
    for (state, byte) in mac.iter_mut().zip(block.iter()) {
        *state ^= byte;
    }
    cipher.encrypt_block(mac);
}

#[cfg(test)]
mod tests;
