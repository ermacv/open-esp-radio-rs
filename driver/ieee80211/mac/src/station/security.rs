//! Exact Open/WPA2 association admission and selected RSN elements.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityError {
    SecurityModeMismatch,
    MissingRsn,
    MalformedRsn,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    ManagementFrameProtectionRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRsn {
    length: u8,
    bytes: [u8; SELECTED_RSN_IE_LEN],
}

impl SelectedRsn {
    const EMPTY: Self = Self {
        length: 0,
        bytes: [0; SELECTED_RSN_IE_LEN],
    };

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

pub fn select_wpa2_psk_rsn(access_point: &ScanRecord) -> Result<SelectedRsn, StaSecurityError> {
    if !access_point.matches_security(WifiSecurityMode::Wpa2Personal) {
        return Err(StaSecurityError::SecurityModeMismatch);
    }
    let rsn = access_point.rsn_ie_bytes();
    if rsn.len() < 2 || rsn[0] != 48 || usize::from(rsn[1]) + 2 != rsn.len() {
        return Err(if rsn.is_empty() {
            StaSecurityError::MissingRsn
        } else {
            StaSecurityError::MalformedRsn
        });
    }

    let body = &rsn[2..];
    let mut offset = 0;
    if read_rsn_u16(body, &mut offset)? != 1 {
        return Err(StaSecurityError::UnsupportedVersion);
    }
    if !is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP) {
        return Err(StaSecurityError::UnsupportedGroupCipher);
    }

    let pairwise_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_ccmp = false;
    for _ in 0..pairwise_count {
        has_ccmp |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_CIPHER_CCMP);
    }
    if !has_ccmp {
        return Err(StaSecurityError::UnsupportedPairwiseCipher);
    }

    let akm_count = usize::from(read_rsn_u16(body, &mut offset)?);
    let mut has_psk = false;
    for _ in 0..akm_count {
        has_psk |= is_rsn_suite(read_rsn_suite(body, &mut offset)?, RSN_AKM_PSK);
    }
    if !has_psk {
        return Err(StaSecurityError::UnsupportedAkm);
    }
    let capabilities = if offset < body.len() {
        read_rsn_u16(body, &mut offset)?
    } else {
        0
    };
    if offset < body.len() {
        let pmkid_count = usize::from(read_rsn_u16(body, &mut offset)?);
        let pmkid_bytes = pmkid_count
            .checked_mul(16)
            .ok_or(StaSecurityError::MalformedRsn)?;
        skip_rsn_bytes(body, &mut offset, pmkid_bytes)?;
    }
    if offset < body.len() {
        // The optional Group Management Cipher Suite is retained only as a
        // syntactic boundary. This WPA2 profile does not negotiate PMF, and
        // MFPR is rejected below.
        if capabilities & RSN_CAPABILITY_MFPC == 0 {
            return Err(StaSecurityError::MalformedRsn);
        }
        let _group_management_cipher = read_rsn_suite(body, &mut offset)?;
    }
    if offset != body.len() {
        return Err(StaSecurityError::MalformedRsn);
    }
    if capabilities & RSN_CAPABILITY_MFPR != 0 {
        return Err(StaSecurityError::ManagementFrameProtectionRequired);
    }

    let mut selected = SelectedRsn::EMPTY;
    selected.length = SELECTED_RSN_IE_LEN as u8;
    // The open STA owns protected A-MSDU construction and receive
    // decapsulation, so it retains the vendor SPP A-MSDU-capable contract.
    // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: frame 7624 carries RSN
    // Capabilities 0x0400 in the successful HE association request.
    selected.bytes.copy_from_slice(&[
        48,
        20,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_AKM_PSK,
        RSN_CAPABILITY_SPP_AMSDU_CAPABLE as u8,
        (RSN_CAPABILITY_SPP_AMSDU_CAPABLE >> 8) as u8,
    ]);
    Ok(selected)
}

/// Select the association security IE for one exact requested mode.
///
/// Open never accepts a Privacy/RSN/WPA advertisement. WPA2 never accepts an
/// open or mixed WPA/WPA2 advertisement, and then validates the complete
/// retained RSN suites before returning a source-owned RSN element.
pub fn select_association_rsn(
    access_point: &ScanRecord,
    security: WifiSecurityMode,
) -> Result<SelectedRsn, StaSecurityError> {
    match security {
        WifiSecurityMode::Open if access_point.matches_security(security) => Ok(SelectedRsn::EMPTY),
        WifiSecurityMode::Open => Err(StaSecurityError::SecurityModeMismatch),
        WifiSecurityMode::Wpa2Personal => select_wpa2_psk_rsn(access_point),
    }
}

fn read_rsn_u16(bytes: &[u8], offset: &mut usize) -> Result<u16, StaSecurityError> {
    let value = bytes
        .get(*offset..*offset + 2)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset += 2;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_rsn_suite(bytes: &[u8], offset: &mut usize) -> Result<[u8; 4], StaSecurityError> {
    let value = bytes
        .get(*offset..*offset + 4)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset += 4;
    Ok([value[0], value[1], value[2], value[3]])
}

fn skip_rsn_bytes(bytes: &[u8], offset: &mut usize, length: usize) -> Result<(), StaSecurityError> {
    let end = offset
        .checked_add(length)
        .ok_or(StaSecurityError::MalformedRsn)?;
    bytes
        .get(*offset..end)
        .ok_or(StaSecurityError::MalformedRsn)?;
    *offset = end;
    Ok(())
}

fn is_rsn_suite(suite: [u8; 4], selector: u8) -> bool {
    suite[..3] == RSN_OUI && suite[3] == selector
}
