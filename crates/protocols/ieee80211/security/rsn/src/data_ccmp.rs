//! Software CCMP for data, including negotiated SPP A-MSDU.
//!
//! The association owns the SPP decision; individual keys do not select it.
//! The transform consumes the same checked MAC geometry as hardware-backed RX.
//! It does not allocate packet numbers or commit replay: those belong to the
//! installed key's separate TX/RX owners.
//!
//! IEEE 802.11 CCMP AAD masks mutable FC/sequence/QoS fields. With SPP, the
//! QoS A-MSDU-present bit joins TID in AAD and nonce flags. This follows
//! [Linux v6.12 `ccmp_gcmp_aad` / `ccmp_special_blocks`](https://github.com/torvalds/linux/blob/v6.12/net/mac80211/wpa.c).

use crate::ccm::{self, MIC_LEN, NONCE_LEN};
use aes::{
    Aes128,
    cipher::{KeyInit, generic_array::GenericArray},
};
use oer_ieee80211_mac::{ccmp::CcmpHeader, frame::DecodedHeader, security::Spp};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// Why a CCMP data MPDU cannot be authenticated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataCcmpError {
    MalformedHeader,
    NotProtectedData,
    InvalidCcmpHeader,
    Truncated,
    TooLong,
    InvalidMic,
}

/// Immutable authentication policy of one association, shared by PTK and GTK.
/// It contains no key material. Hardware/model installation retains this
/// record while every installed key refers to the same association owner.
#[derive(Debug)]
pub struct CcmpAssociation {
    spp: Spp,
}

impl CcmpAssociation {
    pub const fn new(spp: Spp) -> Self {
        Self { spp }
    }
    pub const fn spp(&self) -> Spp {
        self.spp
    }

    /// Encrypt an MPDU in place. The caller has already allocated its PN and
    /// encoded the CCMP header, and reserved eight trailing bytes for the MIC.
    pub fn seal(&self, key: &[u8; 16], frame: &mut [u8]) -> Result<(), DataCcmpError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame.split_at_mut(blocks.body_start);
        let (body, mic) = body_and_mic.split_at_mut(blocks.body_len);
        let cipher = Aes128::new(GenericArray::from_slice(key));
        mic.copy_from_slice(&ccm::authenticate(
            &cipher,
            &blocks.nonce,
            &blocks.aad[..blocks.aad_len],
            body,
        ));
        ccm::crypt(&cipher, &blocks.nonce, body);
        Ok(())
    }

    /// Verify and decrypt. Failed authentication erases the tentative plaintext;
    /// a caller must discard the frame and leave its replay frontier unchanged.
    pub fn open<'a>(&self, key: &[u8; 16], frame: &'a mut [u8]) -> Result<&'a [u8], DataCcmpError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame.split_at_mut(blocks.body_start);
        let (body, mic) = body_and_mic.split_at_mut(blocks.body_len);
        let cipher = Aes128::new(GenericArray::from_slice(key));
        ccm::crypt(&cipher, &blocks.nonce, body);
        let mut expected =
            ccm::authenticate(&cipher, &blocks.nonce, &blocks.aad[..blocks.aad_len], body);
        let valid = bool::from(expected.ct_eq(mic));
        expected.zeroize();
        if !valid {
            body.zeroize();
            return Err(DataCcmpError::InvalidMic);
        }
        Ok(body)
    }

    fn blocks(&self, frame: &[u8]) -> Result<Blocks, DataCcmpError> {
        let header = DecodedHeader::decode(frame).ok_or(DataCcmpError::MalformedHeader)?;
        let control = header.control();
        if !control.is_data() || !control.protected() {
            return Err(DataCcmpError::NotProtectedData);
        }
        let (ccmp, protected) = header
            .body()
            .split_first_chunk::<8>()
            .ok_or(DataCcmpError::Truncated)?;
        let ccmp = CcmpHeader::parse(*ccmp).map_err(|_| DataCcmpError::InvalidCcmpHeader)?;
        let body_len = protected
            .len()
            .checked_sub(MIC_LEN)
            .ok_or(DataCcmpError::Truncated)?;
        u16::try_from(body_len).map_err(|_| DataCcmpError::TooLong)?;
        let mut aad = [0; 30];
        // Mutable FC subtype bits 4..6 and Retry/PM/MoreData are not authenticated.
        let mut fc = control.get() & !0x3870;
        if control.is_qos_data() {
            fc &= !0x8000;
        }
        aad[..2].copy_from_slice(&fc.to_le_bytes());
        aad[2..8].copy_from_slice(&header.receiver());
        aad[8..14].copy_from_slice(&header.transmitter());
        aad[14..20].copy_from_slice(&header.address3());
        aad[20] = (header.sequence_control() & 15) as u8;
        let mut aad_len = 22;
        if let Some(address4) = header.address4() {
            aad[22..28].copy_from_slice(&address4);
            aad_len = 28;
        }
        let flags = if let Some(qos) = header.qos() {
            let mask = match self.spp {
                Spp::Negotiated => 0x8f,
                Spp::NotNegotiated => 0x0f,
            };
            let flags = (qos as u8) & mask;
            aad[aad_len] = flags;
            aad_len += 2;
            flags
        } else {
            0
        };
        let mut nonce = [0; NONCE_LEN];
        nonce[0] = flags;
        nonce[1..7].copy_from_slice(&header.transmitter());
        nonce[7..].copy_from_slice(&ccmp.packet_number().value().to_be_bytes()[2..]);
        Ok(Blocks {
            aad,
            aad_len,
            nonce,
            body_len,
            body_start: frame.len() - protected.len(),
        })
    }
}

struct Blocks {
    aad: [u8; 30],
    aad_len: usize,
    nonce: [u8; NONCE_LEN],
    body_start: usize,
    body_len: usize,
}

#[cfg(test)]
mod tests;
