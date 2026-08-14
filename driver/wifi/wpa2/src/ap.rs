//! WPA2-Personal association validation for an allocation-free authenticator.
//!
//! This is the hardware-independent prefix of the production AP
//! implementation. Peer queues, callbacks and ESP32-S31 node
//! pointers are deliberately outside this crate.

use crate::frames::{OwnedRsnIe, Wpa2FrameError};

const RSN_ELEMENT_ID: u8 = 0x30;
const RSN_VERSION: u16 = 1;
const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
const RSN_CIPHER_CCMP: u8 = 4;
const RSN_AKM_PSK: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wpa2ApRsnError {
    Malformed,
    CapacityExceeded,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionUnsupported,
    PmkidCachingUnsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWpa2ApRsn {
    owned: OwnedRsnIe,
    capabilities: u16,
}

impl ValidatedWpa2ApRsn {
    pub fn owned(&self) -> &OwnedRsnIe {
        &self.owned
    }

    pub const fn capabilities(&self) -> u16 {
        self.capabilities
    }

    pub fn into_owned(self) -> OwnedRsnIe {
        self.owned
    }
}

fn read_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, Wpa2ApRsnError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(Wpa2ApRsnError::Malformed)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], Wpa2ApRsnError> {
    let suite = bytes
        .get(*offset..*offset + 4)
        .ok_or(Wpa2ApRsnError::Malformed)?;
    *offset += 4;
    Ok([suite[0], suite[1], suite[2], suite[3]])
}

fn supported_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}

/// Validate the WPA2-PSK/CCMP subset implemented by [`crate::state::Wpa2ApState`].
pub fn validate_wpa2_ap_rsn(bytes: &[u8]) -> Result<ValidatedWpa2ApRsn, Wpa2ApRsnError> {
    let owned = OwnedRsnIe::try_copy(bytes).map_err(|error| match error {
        Wpa2FrameError::CapacityExceeded => Wpa2ApRsnError::CapacityExceeded,
        _ => Wpa2ApRsnError::Malformed,
    })?;
    if bytes.first() != Some(&RSN_ELEMENT_ID) {
        return Err(Wpa2ApRsnError::Malformed);
    }

    let body = &bytes[2..];
    let mut offset = 0;
    if read_u16(body, &mut offset)? != RSN_VERSION {
        return Err(Wpa2ApRsnError::UnsupportedVersion);
    }
    if !supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(Wpa2ApRsnError::UnsupportedGroupCipher);
    }

    let pairwise_count = usize::from(read_u16(body, &mut offset)?);
    if pairwise_count == 0 {
        return Err(Wpa2ApRsnError::UnsupportedPairwiseCipher);
    }
    let mut pairwise_ccmp = false;
    for _ in 0..pairwise_count {
        pairwise_ccmp |= supported_suite(read_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !pairwise_ccmp {
        return Err(Wpa2ApRsnError::UnsupportedPairwiseCipher);
    }

    let akm_count = usize::from(read_u16(body, &mut offset)?);
    if akm_count == 0 {
        return Err(Wpa2ApRsnError::UnsupportedAkm);
    }
    let mut psk = false;
    for _ in 0..akm_count {
        psk |= supported_suite(read_suite(body, &mut offset)?, RSN_AKM_PSK);
    }
    if !psk {
        return Err(Wpa2ApRsnError::UnsupportedAkm);
    }

    let capabilities = if offset < body.len() {
        let capabilities = read_u16(body, &mut offset)?;
        if capabilities & RSN_CAPABILITY_MFPR != 0 {
            return Err(Wpa2ApRsnError::ManagementFrameProtectionUnsupported);
        }
        capabilities
    } else {
        0
    };
    if offset < body.len() && read_u16(body, &mut offset)? != 0 {
        return Err(Wpa2ApRsnError::PmkidCachingUnsupported);
    }
    if offset != body.len() {
        return Err(Wpa2ApRsnError::Malformed);
    }
    Ok(ValidatedWpa2ApRsn {
        owned,
        capabilities,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rsn(pairwise: u8, akm: u8, capabilities: u16) -> [u8; 22] {
        let mut ie = [0_u8; 22];
        ie[0] = 0x30;
        ie[1] = 20;
        ie[2..4].copy_from_slice(&1_u16.to_le_bytes());
        ie[4..8].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
        ie[8..10].copy_from_slice(&1_u16.to_le_bytes());
        ie[10..14].copy_from_slice(&[0x00, 0x0f, 0xac, pairwise]);
        ie[14..16].copy_from_slice(&1_u16.to_le_bytes());
        ie[16..20].copy_from_slice(&[0x00, 0x0f, 0xac, akm]);
        ie[20..22].copy_from_slice(&capabilities.to_le_bytes());
        ie
    }

    #[test]
    fn accepts_wpa2_psk_ccmp_and_optional_mfpc() {
        let ie = rsn(4, 2, 1 << 7);
        let validated = validate_wpa2_ap_rsn(&ie).unwrap();
        assert_eq!(validated.owned().as_bytes(), &ie);
        assert_eq!(validated.capabilities(), 1 << 7);
    }

    #[test]
    fn rejects_non_ccmp_non_psk_and_required_pmf() {
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(2, 2, 0)),
            Err(Wpa2ApRsnError::UnsupportedPairwiseCipher)
        );
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(4, 8, 0)),
            Err(Wpa2ApRsnError::UnsupportedAkm)
        );
        assert_eq!(
            validate_wpa2_ap_rsn(&rsn(4, 2, 1 << 6)),
            Err(Wpa2ApRsnError::ManagementFrameProtectionUnsupported)
        );
    }

    #[test]
    fn accepts_zero_pmkid_count_and_rejects_nonzero_lists() {
        let mut ie = [0_u8; 24];
        ie[..22].copy_from_slice(&rsn(4, 2, 0));
        ie[1] = 22;
        assert!(validate_wpa2_ap_rsn(&ie).is_ok());
        ie[22..24].copy_from_slice(&1_u16.to_le_bytes());
        assert_eq!(
            validate_wpa2_ap_rsn(&ie),
            Err(Wpa2ApRsnError::PmkidCachingUnsupported)
        );
    }
}
