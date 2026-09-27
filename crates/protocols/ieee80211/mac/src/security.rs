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

pub mod rsn;
