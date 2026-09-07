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
    /// WPA2-Personal using RSN, PSK authentication and CCMP.
    Wpa2Personal,
}
