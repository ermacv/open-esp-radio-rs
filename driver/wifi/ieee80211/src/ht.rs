//! Shared, allocation-free IEEE 802.11n HT information elements.
//!
//! These records describe the one-spatial-stream HT profile shared by the
//! radio roles. The validated channel geometry decides whether a BSS
//! advertises HT20 or HT40; callers cannot independently select contradictory
//! capability and operation elements.

use crate::channel::{WifiChannel, WifiChannelWidth};

pub const HT_CAPABILITY_IE_LEN: usize = 28;
pub const HT_OPERATION_IE_LEN: usize = 24;

/// Build the complete one-stream HT Capabilities element for one BSS.
pub const fn ht_capability_ie(channel: WifiChannel) -> [u8; HT_CAPABILITY_IE_LEN] {
    let capability_info = match channel.width() {
        // Short GI for 20 MHz.
        WifiChannelWidth::Mhz20 => 1 << 5,
        // Supported Channel Width plus short GI for both 20 and 40 MHz. SMPS
        // is a station behavior and is not asserted by the AP BSS profile.
        WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
            (1 << 1) | (1 << 5) | (1 << 6)
        }
    };
    let mut element = [0_u8; HT_CAPABILITY_IE_LEN];
    element[0] = 45;
    element[1] = 26;
    element[2] = capability_info;
    // 65,535-byte receive A-MPDU and four-microsecond minimum spacing.
    element[4] = 0x17;
    // One receive spatial stream, MCS0 through MCS7.
    element[5] = 0xff;
    // TX MCS set is defined and equal to the RX set.
    element[18] = 0x01;
    element
}

/// Build the complete HT Operation element for one validated BSS channel.
pub const fn ht_operation_ie(channel: WifiChannel) -> [u8; HT_OPERATION_IE_LEN] {
    let mut element = [0_u8; 24];
    element[0] = 61;
    element[1] = 22;
    element[2] = channel.primary();
    element[3] = match channel.width() {
        // Secondary-channel offset zero and STA channel width zero select
        // HT20.
        WifiChannelWidth::Mhz20 => 0,
        // IEEE secondary offset one and STA channel width one.
        WifiChannelWidth::Mhz40Above => 0x05,
        // IEEE secondary offset three and STA channel width one.
        WifiChannelWidth::Mhz40Below => 0x07,
    };
    element
}

/// One peer's directly observed one-stream HT receive capabilities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtPeerCapabilities {
    capability_info: u16,
    ampdu_parameters: u8,
    rx_mcs_0_to_7: u8,
}

impl HtPeerCapabilities {
    pub const fn supports_40_mhz(self) -> bool {
        self.capability_info & (1 << 1) != 0
    }

    pub const fn supports_short_guard_interval(self, width: WifiChannelWidth) -> bool {
        match width {
            WifiChannelWidth::Mhz20 => self.capability_info & (1 << 5) != 0,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
                self.supports_40_mhz() && self.capability_info & (1 << 6) != 0
            }
        }
    }

    pub const fn ampdu_parameters(self) -> u8 {
        self.ampdu_parameters
    }

    /// Highest common one-spatial-stream MCS advertised by the peer.
    pub const fn highest_rx_mcs(self) -> u8 {
        7 - self.rx_mcs_0_to_7.leading_zeros() as u8
    }
}

/// Parse a complete information-element stream into one-stream HT facts.
pub fn ht_peer_capabilities(bytes: &[u8]) -> Option<HtPeerCapabilities> {
    let mut remaining = bytes;
    while remaining.len() >= 2 {
        let id = remaining[0];
        let length = usize::from(remaining[1]);
        let record = remaining.get(..length.saturating_add(2))?;
        if id == 45 && length == 26 && record[5] != 0 {
            return Some(HtPeerCapabilities {
                capability_info: u16::from_le_bytes([record[2], record[3]]),
                ampdu_parameters: record[4],
                rx_mcs_0_to_7: record[5],
            });
        }
        remaining = &remaining[record.len()..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ht20_records_are_complete_and_bounded() {
        let channel = WifiChannel::mhz20(6).unwrap();
        let capability = ht_capability_ie(channel);
        assert_eq!(&capability[..2], &[45, 26]);
        assert_eq!(capability[2], 1 << 5);
        assert_eq!(capability[5], 0xff);
        assert_eq!(ht_operation_ie(channel)[..4], [61, 22, 6, 0]);
        assert_eq!(
            ht_peer_capabilities(&capability),
            Some(HtPeerCapabilities {
                capability_info: 1 << 5,
                ampdu_parameters: 0x17,
                rx_mcs_0_to_7: 0xff,
            })
        );
        let mut empty = [0_u8; 28];
        empty[..2].copy_from_slice(&[45, 26]);
        assert_eq!(ht_peer_capabilities(&empty), None);
    }

    #[test]
    fn ht40_records_keep_width_geometry_and_peer_facts_coherent() {
        let above = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
        let below = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Below).unwrap();
        let capability = ht_capability_ie(above);
        assert_eq!(capability[2], 0x62);
        assert_eq!(ht_operation_ie(above)[..4], [61, 22, 6, 0x05]);
        assert_eq!(ht_operation_ie(below)[..4], [61, 22, 6, 0x07]);
        let peer = ht_peer_capabilities(&capability).unwrap();
        assert!(peer.supports_40_mhz());
        assert!(peer.supports_short_guard_interval(WifiChannelWidth::Mhz40Above));
        assert_eq!(peer.highest_rx_mcs(), 7);
    }
}
