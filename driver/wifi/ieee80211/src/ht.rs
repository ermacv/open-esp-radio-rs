//! Shared, allocation-free IEEE 802.11n HT information elements.
//!
//! These records describe the one-spatial-stream HT profile shared by the
//! radio roles. The validated channel geometry decides whether a BSS
//! advertises HT20 or HT40; callers cannot independently select contradictory
//! capability and operation elements.

use crate::channel::{WifiChannel, WifiChannelWidth};

pub const HT_CAPABILITY_IE_LEN: usize = 28;
pub const HT_OPERATION_IE_LEN: usize = 24;

/// The standard HT Duplicate modulation-and-coding selector, MCS32.
///
/// MCS32 is a special single-stream 40-MHz duplicate mode. It is not the next
/// member of the ordinary one-spatial-stream MCS0..MCS7 range, so keeping it
/// as a zero-sized marker prevents rate-selection code from ordering it as a
/// faster spatial-stream rate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtDuplicateMcs32;

impl HtDuplicateMcs32 {
    pub const INDEX: u8 = 32;
    pub const CAPABILITY_IE_BYTE: usize = 9;
    pub const CAPABILITY_IE_MASK: u8 = 1;

    pub const fn new() -> Self {
        Self
    }

    pub const fn supports_width(width: WifiChannelWidth) -> bool {
        matches!(
            width,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below
        )
    }

    /// Advertise receive-only MCS32 in a complete HT Capabilities IE.
    ///
    /// TX MCS Parameters bit one marks the TX and RX sets as unequal. Without
    /// it, setting the RX MCS32 bit would also claim unimplemented TX support.
    pub const fn advertise_receive_only(self, element: &mut [u8; HT_CAPABILITY_IE_LEN]) {
        element[Self::CAPABILITY_IE_BYTE] |= Self::CAPABILITY_IE_MASK;
        element[17] |= 0x03;
    }
}

/// Build the complete one-stream HT Capabilities element for one BSS.
pub const fn ht_capability_ie(channel: WifiChannel) -> [u8; HT_CAPABILITY_IE_LEN] {
    ht_capability_ie_for_peer(channel, None)
}

/// Build the HT Capabilities element advertised to one known peer.
///
/// Complete vendor `ieee80211_add_htcap_body` negotiates the receive A-MPDU
/// parameters in an association response: it selects the smaller maximum
/// A-MPDU exponent and the larger minimum MPDU spacing. Beacons have no peer
/// record and therefore retain the local interface value.
pub const fn ht_capability_ie_for_peer(
    channel: WifiChannel,
    peer: Option<HtPeerCapabilities>,
) -> [u8; HT_CAPABILITY_IE_LEN] {
    // `ieee80211_ht_attach` initializes the vendor interface capability at
    // offset `+0x14c` to `0x100c`: spatial multiplexing power save disabled
    // and DSSS/CCK reception permitted in 40 MHz. The shared vendor
    // `ieee80211_add_htcap_body` then adds channel width and short-GI bits.
    // Keep that role-independent base here; advertising the former `0x0000`
    // base made our AP claim static SMPS even though no corresponding power-
    // save behavior exists in the Rust implementation.
    let capability_info: u16 = 0x100c
        | match channel.width() {
            // Short GI for 20 MHz.
            WifiChannelWidth::Mhz20 => 1 << 5,
            // Supported Channel Width plus short GI for both 20 and 40 MHz.
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
                (1 << 1) | (1 << 5) | (1 << 6)
            }
        };
    let mut element = [0_u8; HT_CAPABILITY_IE_LEN];
    element[0] = 45;
    element[1] = 26;
    element[2] = capability_info as u8;
    element[3] = (capability_info >> 8) as u8;
    // 65,535-byte receive A-MPDU with no local MPDU-density limit.
    //
    // Complete vendor `ieee80211_ht_attach` derives only the two-bit maximum
    // A-MPDU exponent from the configured static RX-buffer count and stores
    // that value at interface offset `+0x148`. Complete
    // S31 profile owns 64 RX descriptors, selecting exponent three by the same
    // vendor thresholds. For a peer-specific response the vendor keeps the
    // stricter of both peers' limits rather than repeating this local value.
    let local_ampdu_parameters = 0x03;
    element[4] = match peer {
        Some(peer) => {
            let exponent = if local_ampdu_parameters & 0x03 < peer.ampdu_parameters & 0x03 {
                local_ampdu_parameters & 0x03
            } else {
                peer.ampdu_parameters & 0x03
            };
            let local_spacing = local_ampdu_parameters & 0x1c;
            let peer_spacing = peer.ampdu_parameters & 0x1c;
            let spacing = if local_spacing > peer_spacing {
                local_spacing
            } else {
                peer_spacing
            };
            exponent | spacing
        }
        None => local_ampdu_parameters,
    };
    // One receive spatial stream, MCS0 through MCS7.
    element[5] = 0xff;
    // Supported MCS Set byte 12: the ordinary TX MCS set is defined and equal
    // to RX until an independently advertised receive-only mode changes it.
    // `ieee80211_add_htcap_body` writes this at body offset 15; the complete
    // information element has a two-byte header, so the byte is index 17.
    element[17] = 0x01;
    // MCS32 is the independent HT Duplicate receive bit. It is valid only for
    // a 40-MHz HT BSS and must never be treated as a fifth spatial stream.
    if HtDuplicateMcs32::supports_width(channel.width()) {
        HtDuplicateMcs32::new().advertise_receive_only(&mut element);
    }
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
    rx_ht_duplicate_mcs32: bool,
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

    /// Whether the peer can receive the special 40-MHz HT Duplicate mode.
    ///
    /// A malformed peer which sets MCS32 without also admitting 40 MHz is
    /// rejected here even though its raw Supported MCS Set bit is retained by
    /// the parser.
    pub const fn supports_ht_duplicate_mcs32(self) -> bool {
        self.rx_ht_duplicate_mcs32 && self.supports_40_mhz()
    }

    pub const fn ht_duplicate_mcs32(self) -> Option<HtDuplicateMcs32> {
        if self.supports_ht_duplicate_mcs32() {
            Some(HtDuplicateMcs32::new())
        } else {
            None
        }
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
                rx_ht_duplicate_mcs32: record[HtDuplicateMcs32::CAPABILITY_IE_BYTE]
                    & HtDuplicateMcs32::CAPABILITY_IE_MASK
                    != 0,
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
        assert_eq!(u16::from_le_bytes([capability[2], capability[3]]), 0x102c);
        assert_eq!(capability[5], 0xff);
        assert_eq!(capability[17], 0x01);
        assert_eq!(capability[18], 0);
        assert_eq!(ht_operation_ie(channel)[..4], [61, 22, 6, 0]);
        assert_eq!(
            ht_peer_capabilities(&capability),
            Some(HtPeerCapabilities {
                capability_info: 0x102c,
                ampdu_parameters: 0x03,
                rx_mcs_0_to_7: 0xff,
                rx_ht_duplicate_mcs32: false,
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
        assert_eq!(u16::from_le_bytes([capability[2], capability[3]]), 0x106e);
        assert_eq!(ht_operation_ie(above)[..4], [61, 22, 6, 0x05]);
        assert_eq!(ht_operation_ie(below)[..4], [61, 22, 6, 0x07]);
        let peer = ht_peer_capabilities(&capability).unwrap();
        assert!(peer.supports_40_mhz());
        assert!(peer.supports_short_guard_interval(WifiChannelWidth::Mhz40Above));
        assert_eq!(peer.highest_rx_mcs(), 7);
    }

    #[test]
    fn peer_specific_capability_uses_vendor_ampdu_negotiation() {
        let channel = WifiChannel::mhz20(6).unwrap();
        let mut peer_record = ht_capability_ie(channel);
        peer_record[4] = 0x17;
        let peer = ht_peer_capabilities(&peer_record).unwrap();

        assert_eq!(ht_capability_ie(channel)[4], 0x03);
        assert_eq!(ht_capability_ie_for_peer(channel, Some(peer))[4], 0x17);

        peer_record[4] = 0x15;
        let stricter_exponent = ht_peer_capabilities(&peer_record).unwrap();
        assert_eq!(
            ht_capability_ie_for_peer(channel, Some(stricter_exponent))[4],
            0x15
        );
    }
}
