//! Exact Open/WPA2 association admission and selected RSN elements.

use super::*;

use crate::security::rsn::{
    RSN_AKM_PSK, RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RSN_CAPABILITY_SPP_AMSDU_CAPABLE,
    RSN_CIPHER_CCMP, RsnElement, RsnSyntaxError, ieee_suite,
};

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
    if rsn.is_empty() {
        return Err(StaSecurityError::MissingRsn);
    }
    let rsn = RsnElement::parse(rsn).map_err(|error| match error {
        RsnSyntaxError::Malformed => StaSecurityError::MalformedRsn,
        RsnSyntaxError::UnsupportedVersion => StaSecurityError::UnsupportedVersion,
    })?;

    if rsn.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP) {
        return Err(StaSecurityError::UnsupportedGroupCipher);
    }
    if !rsn.pairwise_ciphers().contains(ieee_suite(RSN_CIPHER_CCMP)) {
        return Err(StaSecurityError::UnsupportedPairwiseCipher);
    }
    if !rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK)) {
        return Err(StaSecurityError::UnsupportedAkm);
    }
    // An advertised PMKID list is ignored. The optional Group Management
    // Cipher Suite is retained only as a boundary accepted with MFPC. This
    // WPA2 profile does not negotiate PMF, and MFPR is rejected.
    let capabilities = rsn.capabilities().unwrap_or(0);
    if rsn.group_management_cipher().is_some() && capabilities & RSN_CAPABILITY_MFPC == 0 {
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
