//! How many queued frames one A-MPDU carries.
//!
//! An originator aggregates the frames at the head of its queue that share
//! a TID and a recipient. [`AmpduLimits`] bounds that run by the Block Ack
//! agreement's window, the port's A-MPDU capabilities, the recipient's HT
//! A-MPDU Parameters and the access category's TXOP limit, at the rate the
//! aggregate goes out. The bound counts each subframe as its largest
//! protected encoding ([`subframe_bound`]), so a run it admits fits once
//! encoded.

use oer_ieee80211_lower_mac::{AmpduCapabilities, PhyRate};
use oer_ieee80211_mac::phy::LegacyRate;

use crate::ampdu::MAX_AMPDU_SUBFRAMES;

/// The SIFS and the compressed BlockAck that answers an aggregate, at the
/// lowest mandatory ERP-OFDM rate (6 Mb/s): the response a TXOP holds after
/// the aggregate.
pub const BLOCK_ACK_RESPONSE_MICROS: u32 =
    10 + PhyRate::Legacy(LegacyRate::Ofdm6M).max_ppdu_duration_micros(32);

/// The A-MPDU subframe of an Ethernet-II frame of `ethernet_len` octets at
/// most: delimiter, QoS data header, CCMP header, LLC/SNAP, the payload, MIC
/// and FCS, padded to four octets.
pub const fn subframe_bound(ethernet_len: usize) -> u32 {
    (4 + 26 + 8 + 8 + ethernet_len as u32 + 8 + 4 + 3) & !3
}

/// What bounds one A-MPDU to one recipient.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduLimits {
    /// The operational agreement's window.
    pub window: u16,
    pub port: AmpduCapabilities,
    /// The recipient's HT A-MPDU Parameters field: its Maximum A-MPDU
    /// Length Exponent (bits 0-1) and Minimum MPDU Start Spacing (bits 2-4).
    pub peer_ampdu_parameters: u8,
    /// The TXOP limit of the access category; `None` without one.
    pub txop_limit_micros: Option<u32>,
    /// The rate the aggregate goes out at.
    pub rate: PhyRate,
}

impl AmpduLimits {
    /// The recipient's Maximum A-MPDU Length in octets.
    pub const fn peer_max_length(&self) -> u32 {
        (1_u32 << (13 + (self.peer_ampdu_parameters & 0x03) as u32)) - 1
    }

    /// The recipient's Minimum MPDU Start Spacing, IEEE encoding 0-7.
    pub const fn min_mpdu_start_spacing(&self) -> u8 {
        (self.peer_ampdu_parameters >> 2) & 0x07
    }

    /// How many frames of `ethernet_lens`, from the first, one A-MPDU
    /// carries: none when the port sends no A-MPDU at the rate. The
    /// aggregate and its BlockAck stay within the TXOP limit; a run of
    /// fewer than two frames goes out as single MPDUs, which may exceed it.
    pub fn run(&self, ethernet_lens: impl IntoIterator<Item = usize>) -> usize {
        if !self.port.formats.contains_rate(self.rate) {
            return 0;
        }
        let limit = usize::from(self.window)
            .min(usize::from(self.port.max_subframes))
            .min(usize::from(MAX_AMPDU_SUBFRAMES));
        let maximum = self.peer_max_length().min(self.port.max_length);
        let mut length = 0_u32;
        let mut run = 0;
        for ethernet_len in ethernet_lens.into_iter().take(limit) {
            let subframe = subframe_bound(ethernet_len);
            if length + subframe > maximum {
                break;
            }
            if self.txop_limit_micros.is_some_and(|txop| {
                self.rate.max_ppdu_duration_micros(length + subframe) + BLOCK_ACK_RESPONSE_MICROS
                    > txop
            }) {
                break;
            }
            length += subframe;
            run += 1;
        }
        run
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_lower_mac::PhyFormatSet;
    use oer_ieee80211_mac::phy::{HtMcs, HtRate, PpduBandwidth};

    use super::*;

    fn ht_mcs7() -> PhyRate {
        PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz20, false).unwrap())
    }

    fn limits() -> AmpduLimits {
        AmpduLimits {
            window: 32,
            port: AmpduCapabilities {
                max_subframes: 16,
                formats: PhyFormatSet::HT,
                max_length: 65_535,
            },
            // A Maximum A-MPDU Length of 65 535 octets, 8 µs spacing.
            peer_ampdu_parameters: 0x03 | (6 << 2),
            txop_limit_micros: None,
            rate: ht_mcs7(),
        }
    }

    #[test]
    fn the_smallest_of_window_port_and_frames_bounds_a_run() {
        let limits = limits();
        assert_eq!(limits.run([100; 20]), 16);
        assert_eq!(limits.run([100; 5]), 5);
        assert_eq!(
            AmpduLimits {
                window: 3,
                ..limits
            }
            .run([100; 20]),
            3
        );
        assert_eq!(limits.min_mpdu_start_spacing(), 6);
    }

    #[test]
    fn the_peers_length_exponent_ends_a_run() {
        // 8 191 octets hold five subframes of 1 514-octet frames.
        let limits = AmpduLimits {
            peer_ampdu_parameters: 0,
            ..limits()
        };
        assert_eq!(limits.peer_max_length(), 8_191);
        assert_eq!(subframe_bound(1_514), 1_572);
        assert_eq!(limits.run([1_514; 8]), 5);
    }

    #[test]
    fn the_txop_limit_holds_the_aggregate_and_its_block_ack() {
        let rate = ht_mcs7();
        let one = subframe_bound(1_514);
        // Room for two subframes and the BlockAck, not three.
        let txop = rate.max_ppdu_duration_micros(2 * one) + BLOCK_ACK_RESPONSE_MICROS;
        let limits = AmpduLimits {
            txop_limit_micros: Some(txop),
            ..limits()
        };
        assert_eq!(limits.run([1_514; 8]), 2);
    }

    #[test]
    fn a_rate_the_port_does_not_aggregate_carries_no_run() {
        let limits = AmpduLimits {
            rate: PhyRate::Legacy(LegacyRate::Ofdm54M),
            ..limits()
        };
        assert_eq!(limits.run([100; 8]), 0);
    }
}
