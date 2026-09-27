//! Link-security selection shared by station and access-point protocol code.

/// Exact security contract for one infrastructure BSS.
///
/// This is deliberately not an ordered strength or preference. A caller
/// selects one variant and candidate/association/data paths must match it
/// exactly; there is no downgrade or mixed WPA/Open fallback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSecurityMode {
    /// IEEE 802.11 Open System with plaintext data and no RSN element.
    Open,
    /// RSN with CCMP data protection under a personal key: WPA2-Personal
    /// (PSK) or, for a station, WPA3-Personal (SAE).
    Wpa2Personal,
}

/// What one station request admits.
///
/// A personal request never downgrades: WPA2-Personal admits PSK or, as the
/// vendor's WPA3-enabled station does, upgrades to SAE whenever the access
/// point offers it; WPA3-Personal admits SAE only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityPolicy {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

impl StaSecurityPolicy {
    /// The data protection of the link.
    pub const fn link_mode(self) -> WifiSecurityMode {
        match self {
            Self::Open => WifiSecurityMode::Open,
            Self::Wpa2Personal | Self::Wpa3Personal => WifiSecurityMode::Wpa2Personal,
        }
    }
}

/// The security an access point offers.
///
/// WPA2-Personal keeps the reviewed PSK profile without management frame
/// protection. WPA3-Personal follows the vendor's softAP: SAE only, with
/// management frame protection required, BIP-CMAC-128 as the group
/// management cipher of a CCMP BSS, and both SAE password element methods,
/// which it advertises through the H2E bit of its RSNXE. Each variant owns
/// the exact RSN element the beacon, the probe response and EAPOL Message 3
/// carry.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/esp_supplicant/src/esp_hostap.c`
/// (`hostap_init`: `WIFI_AUTH_WPA3_PSK` selects `WPA_KEY_MGMT_SAE`, PMF
/// required, `WPA_CIPHER_AES_128_CMAC` for CCMP) and
/// `src/ap/wpa_auth_ie.c` (`wpa_own_rsn_capab`, `rsne_write_data`,
/// `wpa_write_rsnxe`); `docs/en/api-guides/wifi-security.rst` ("For WPA3
/// SoftAP, PMF Required is mandatory").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurityPolicy {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

/// RSN element of the WPA2-Personal access point: CCMP group and pairwise
/// ciphers, PSK, no capabilities.
pub const AP_WPA2_PERSONAL_RSN_ELEMENT: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];

/// RSN element of the WPA3-Personal access point: CCMP group and pairwise
/// ciphers, SAE, MFPC and MFPR, no PMKID and BIP-CMAC-128.
pub const AP_WPA3_PERSONAL_RSN_ELEMENT: [u8; 28] = [
    0x30, 26, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 8, 0xc0, 0, 0,
    0, 0, 0x0f, 0xac, 6,
];

/// RSNXE of an access point offering SAE hash-to-element: a one-octet
/// field whose length nibble is zero and whose bit 5 is SAE H2E.
pub const AP_SAE_H2E_RSNX_ELEMENT: [u8; 3] = [244, 1, 1 << 5];

impl ApSecurityPolicy {
    /// The data protection of the BSS.
    pub const fn link_mode(self) -> WifiSecurityMode {
        match self {
            Self::Open => WifiSecurityMode::Open,
            Self::Wpa2Personal | Self::Wpa3Personal => WifiSecurityMode::Wpa2Personal,
        }
    }

    /// The RSN element this BSS advertises; empty for an Open BSS.
    pub const fn rsn_element(self) -> &'static [u8] {
        match self {
            Self::Open => &[],
            Self::Wpa2Personal => &AP_WPA2_PERSONAL_RSN_ELEMENT,
            Self::Wpa3Personal => &AP_WPA3_PERSONAL_RSN_ELEMENT,
        }
    }

    /// The RSNXE this BSS advertises; empty when no extended RSN capability
    /// applies.
    pub const fn rsnx_element(self) -> &'static [u8] {
        match self {
            Self::Open | Self::Wpa2Personal => &[],
            Self::Wpa3Personal => &AP_SAE_H2E_RSNX_ELEMENT,
        }
    }

    /// Whether every association protects its management frames.
    pub const fn requires_management_protection(self) -> bool {
        matches!(self, Self::Wpa3Personal)
    }
}

pub mod rsn;
