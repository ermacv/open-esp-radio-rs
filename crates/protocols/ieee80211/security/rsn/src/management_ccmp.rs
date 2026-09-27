//! Software CCMP of individually addressed robust management frames.
//!
//! With management frame protection, a Deauthentication, Disassociation or
//! robust Action frame to the station arrives CCMP-protected under the
//! pairwise temporal key. The vendor opens these frames in software with its
//! own copy of the key (`ieee80211_ccmp_decrypt` through wpa_supplicant's
//! `ccmp_decrypt`), so the station keeps the temporal key for as long as its
//! association protects management frames.
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
    cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, ZeroizeOnDrop};

const HEADER_LEN: usize = 24;
const CCMP_HEADER_LEN: usize = 8;
const MIC_LEN: usize = 8;
const PROTECTED: u8 = 0x40;
const EXT_IV: u8 = 0x20;
/// Frame Control flags CCMP leaves out of its additional authentication
/// data: Retry, Power Management and More Data.
const AAD_MASKED_FLAGS: u8 = 0x08 | 0x10 | 0x20;
/// Nonce flags of a management frame: priority zero, Management set.
const MANAGEMENT_NONCE_FLAGS: u8 = 0x10;
const AAD_LEN: usize = 22;
const NONCE_LEN: usize = 13;
/// CCM flags of the first authentication block: Adata, an 8-byte MIC and
/// a 2-byte length field.
const B0_FLAGS: u8 = 0x40 | (((MIC_LEN as u8 - 2) / 2) << 3) | 1;
/// CCM flags of every counter block: a 2-byte counter.
const COUNTER_FLAGS: u8 = 1;

/// Why a protected management frame did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementCcmpError {
    /// Shorter than a header, CCMP header and MIC.
    Truncated,
    /// The Protected Frame bit is clear.
    NotProtected,
    /// The CCMP header lacks its Extended IV bit.
    MissingExtendedIv,
    /// The packet number does not exceed the last accepted one.
    Replay,
    /// The MIC does not verify.
    InvalidMic,
}

/// The receive state of protected management frames under one pairwise key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ManagementCcmpReceiver {
    temporal_key: [u8; 16],
    last_packet_number: Option<u64>,
}

impl ManagementCcmpReceiver {
    pub fn new(temporal_key: [u8; 16]) -> Self {
        Self {
            temporal_key,
            last_packet_number: None,
        }
    }

    /// Decrypt one received protected management frame in place and return
    /// its plaintext body. A frame that does not verify leaves the replay
    /// state unchanged.
    pub fn open<'a>(&mut self, frame: &'a mut [u8]) -> Result<&'a [u8], ManagementCcmpError> {
        if frame.len() < HEADER_LEN + CCMP_HEADER_LEN + MIC_LEN {
            return Err(ManagementCcmpError::Truncated);
        }
        if frame[1] & PROTECTED == 0 {
            return Err(ManagementCcmpError::NotProtected);
        }
        let ccmp = &frame[HEADER_LEN..HEADER_LEN + CCMP_HEADER_LEN];
        if ccmp[3] & EXT_IV == 0 {
            return Err(ManagementCcmpError::MissingExtendedIv);
        }
        let packet_number =
            u64::from_le_bytes([ccmp[0], ccmp[1], ccmp[4], ccmp[5], ccmp[6], ccmp[7], 0, 0]);
        if self
            .last_packet_number
            .is_some_and(|last| packet_number <= last)
        {
            return Err(ManagementCcmpError::Replay);
        }

        let mut aad = [0; AAD_LEN];
        aad[0] = frame[0];
        aad[1] = (frame[1] & !AAD_MASKED_FLAGS) | PROTECTED;
        aad[2..20].copy_from_slice(&frame[4..22]);
        // The sequence number is masked; the fragment number stays.
        aad[20] = frame[22] & 0x0f;
        let mut nonce = [0; NONCE_LEN];
        nonce[0] = MANAGEMENT_NONCE_FLAGS;
        nonce[1..7].copy_from_slice(&frame[10..16]);
        for (index, byte) in nonce[7..].iter_mut().enumerate() {
            *byte = (packet_number >> (8 * (5 - index))) as u8;
        }

        let cipher = Aes128::new(GenericArray::from_slice(&self.temporal_key));
        let body_start = HEADER_LEN + CCMP_HEADER_LEN;
        let mic_start = frame.len() - MIC_LEN;
        let body_len = mic_start - body_start;

        // Decrypt the body with counter blocks one and up.
        for (block_index, chunk) in frame[body_start..mic_start].chunks_mut(16).enumerate() {
            let mut stream = counter_block(&nonce, block_index as u16 + 1);
            cipher.encrypt_block(&mut stream);
            for (byte, key) in chunk.iter_mut().zip(stream.iter()) {
                *byte ^= key;
            }
            stream.zeroize();
        }

        // Authenticate the additional data and the plaintext.
        let mut mac = GenericArray::from([0_u8; 16]);
        mac[0] = B0_FLAGS;
        mac[1..14].copy_from_slice(&nonce);
        mac[14..16].copy_from_slice(&(body_len as u16).to_be_bytes());
        cipher.encrypt_block(&mut mac);
        let mut aad_blocks = [0_u8; 32];
        aad_blocks[..2].copy_from_slice(&(AAD_LEN as u16).to_be_bytes());
        aad_blocks[2..2 + AAD_LEN].copy_from_slice(&aad);
        for block in aad_blocks.chunks(16) {
            absorb(&cipher, &mut mac, block);
        }
        for block in frame[body_start..mic_start].chunks(16) {
            absorb(&cipher, &mut mac, block);
        }
        let mut first = counter_block(&nonce, 0);
        cipher.encrypt_block(&mut first);
        let mut expected = [0; MIC_LEN];
        for (index, byte) in expected.iter_mut().enumerate() {
            *byte = mac[index] ^ first[index];
        }
        first.zeroize();
        mac.zeroize();
        if !bool::from(expected.ct_eq(&frame[mic_start..])) {
            // Restore nothing: the caller drops a frame that failed.
            return Err(ManagementCcmpError::InvalidMic);
        }
        self.last_packet_number = Some(packet_number);
        Ok(&frame[body_start..mic_start])
    }
}

fn counter_block(
    nonce: &[u8; NONCE_LEN],
    counter: u16,
) -> GenericArray<u8, aes::cipher::consts::U16> {
    let mut block = GenericArray::from([0_u8; 16]);
    block[0] = COUNTER_FLAGS;
    block[1..14].copy_from_slice(nonce);
    block[14..16].copy_from_slice(&counter.to_be_bytes());
    block
}

/// One CBC-MAC step over a block padded with zeros.
fn absorb(cipher: &Aes128, mac: &mut GenericArray<u8, aes::cipher::consts::U16>, block: &[u8]) {
    for (state, byte) in mac.iter_mut().zip(block.iter()) {
        *state ^= byte;
    }
    cipher.encrypt_block(mac);
}

#[cfg(test)]
mod tests;
