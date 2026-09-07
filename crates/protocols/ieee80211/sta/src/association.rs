//! Association preference among modes admitted by both local and peer profiles.
//!
//! The hardware profile decides eligibility, including supported MCS and width.
//! This policy neither advertises capabilities nor encodes a PHY channel command.

/// Shared with the wire encoder; no duplicate policy enum or upward dependency.
pub use oer_ieee80211::station::association::{PhyMode, Preference};

/// Choose a mode after the caller has intersected local and peer capabilities.
/// HT20 is the baseline; a later association encoder may still reject the peer.
pub fn select_phy(preference: Preference, ht40_available: bool, he20_available: bool) -> PhyMode {
    if preference == Preference::PreferHe20 && he20_available {
        PhyMode::He20
    } else if preference == Preference::ForceHt20 {
        PhyMode::Ht20
    } else if ht40_available {
        PhyMode::Ht40
    } else if he20_available {
        PhyMode::He20
    } else {
        PhyMode::Ht20
    }
}

#[cfg(test)]
mod tests;
