//! Exact Open/WPA2 association admission and selected RSN elements.

use super::*;

use crate::security::rsn::{
    RSN_AKM_PSK, RSN_AKM_PSK_SHA256, RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR,
    RSN_CAPABILITY_SPP_AMSDU_CAPABLE, RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP, RsnElement,
    RsnSyntaxError, ieee_suite,
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
    /// The access point protects management frames with a group management
    /// cipher other than BIP-CMAC-128.
    UnsupportedGroupManagementCipher,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRsn {
    length: u8,
    bytes: [u8; SELECTED_RSN_IE_LEN],
    management_protection: bool,
}

impl SelectedRsn {
    const EMPTY: Self = Self {
        length: 0,
        bytes: [0; SELECTED_RSN_IE_LEN],
        management_protection: false,
    };

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// The association protects its robust management frames.
    pub const fn management_protection(&self) -> bool {
        self.management_protection
    }
}

/// Select the WPA2-Personal RSN element of the association request.
///
/// As the vendor's `ieee80211_parse_rsn` does, the station prefers
/// PSK-SHA256 whenever the access point offers it, and protects management
/// frames whenever the access point is capable of it: the vendor supplicant
/// then sets MFPC in its own element (the configuration's `capable` is always
/// set). Only the default group management cipher, BIP-CMAC-128, is
/// implemented, so the element omits the Group Management Cipher Suite as the
/// vendor supplicant does for the default.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_input.o]::
/// ieee80211_parse_rsn` (AKM switch table `CSWTCH.76`, PMF flag
/// `g_ic+0x210`); ESP-IDF `wpa_gen_wpa_ie_rsn`.
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
    let akm = if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK_SHA256)) {
        RSN_AKM_PSK_SHA256
    } else if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK)) {
        RSN_AKM_PSK
    } else {
        return Err(StaSecurityError::UnsupportedAkm);
    };
    // An advertised PMKID list is ignored.
    let capabilities = rsn.capabilities().unwrap_or(0);
    let management_protection = capabilities & RSN_CAPABILITY_MFPC != 0;
    if !management_protection
        && (rsn.group_management_cipher().is_some() || capabilities & RSN_CAPABILITY_MFPR != 0)
    {
        return Err(StaSecurityError::MalformedRsn);
    }
    if management_protection
        && rsn
            .group_management_cipher()
            .is_some_and(|cipher| cipher != ieee_suite(RSN_CIPHER_BIP_CMAC_128))
    {
        return Err(StaSecurityError::UnsupportedGroupManagementCipher);
    }

    // The open STA owns protected A-MSDU construction and receive
    // decapsulation, so it retains the vendor SPP A-MSDU-capable contract.
    // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: frame 7624 carries RSN
    // Capabilities 0x0400 in the successful HE association request.
    let station_capabilities = RSN_CAPABILITY_SPP_AMSDU_CAPABLE
        | if management_protection {
            RSN_CAPABILITY_MFPC
        } else {
            0
        };
    let mut selected = SelectedRsn::EMPTY;
    selected.length = SELECTED_RSN_IE_LEN as u8;
    selected.management_protection = management_protection;
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
        akm,
        station_capabilities as u8,
        (station_capabilities >> 8) as u8,
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
