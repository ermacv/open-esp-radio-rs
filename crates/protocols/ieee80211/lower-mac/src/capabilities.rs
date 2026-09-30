//! What a lower-MAC backend supports and what it performs autonomously.

use oer_ieee80211_mac::{
    channel::{Band, Channel, ChannelWidth},
    phy::{FecCoding, HeMcs, HtMcs, PhyRate},
};

/// A set of bands.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BandSet(u8);

impl BandSet {
    pub const NONE: Self = Self(0);
    pub const GHZ2_4: Self = Self(1 << 0);
    pub const GHZ5: Self = Self(1 << 1);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, band: Band) -> bool {
        let bit = match band {
            Band::Ghz2_4 => Self::GHZ2_4.0,
            Band::Ghz5 => Self::GHZ5.0,
        };
        self.0 & bit != 0
    }
}

/// A set of channel widths.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct WidthSet(u8);

impl WidthSet {
    pub const NONE: Self = Self(0);
    pub const MHZ20: Self = Self(1 << 0);
    /// Both forty-megahertz geometries.
    pub const MHZ40: Self = Self(1 << 1);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, width: ChannelWidth) -> bool {
        let bit = match width {
            ChannelWidth::Mhz20 => Self::MHZ20.0,
            ChannelWidth::Mhz40Above | ChannelWidth::Mhz40Below => Self::MHZ40.0,
        };
        self.0 & bit != 0
    }
}

/// The rates a backend sends.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RateSupport {
    /// DSSS and CCK rates, in the 2.4 GHz band only.
    pub dsss_cck: bool,
    /// OFDM rates 6 through 54 Mb/s.
    pub ofdm: bool,
    /// The highest HT MCS; `None` without HT.
    pub ht_max_mcs: Option<HtMcs>,
    /// The highest HE SU MCS; `None` without HE.
    pub he_max_mcs: Option<HeMcs>,
    /// The widest HE PPDU in MHz.
    pub he_max_bandwidth_mhz: u16,
    /// HE dual carrier modulation.
    pub he_dcm: bool,
    /// LDPC coding of HE PPDUs.
    pub he_ldpc: bool,
    /// Spatial streams of HT and HE rates.
    pub spatial_streams: u8,
}

impl RateSupport {
    /// Whether the backend sends `rate`; bandwidth against the channel is
    /// checked by [`LowerMacCapabilities::supports_rate`].
    pub const fn supports(self, rate: PhyRate) -> bool {
        match rate {
            PhyRate::Legacy(rate) => {
                if rate.is_ofdm() {
                    self.ofdm
                } else {
                    self.dsss_cck
                }
            }
            PhyRate::Ht(rate) => match self.ht_max_mcs {
                Some(max) => {
                    rate.mcs().index() <= max.index()
                        && rate.mcs().spatial_streams() <= self.spatial_streams
                }
                None => false,
            },
            PhyRate::He(rate) => match self.he_max_mcs {
                Some(max) => {
                    rate.mcs().index() <= max.index()
                        && rate.spatial_streams().count() <= self.spatial_streams
                        && rate.bandwidth().mhz() <= self.he_max_bandwidth_mhz
                        && (!rate.dcm() || self.he_dcm)
                        && (matches!(rate.fec(), FecCoding::Bcc) || self.he_ldpc)
                }
                None => false,
            },
        }
    }
}

/// Work a backend performs without software on the air timeline.
///
/// The first six are below the port on every backend that has them; the
/// others are work portable code performs above the port unless the
/// backend reports it, in which case the portable owner delegates instead.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct HardwareServices(u16);

impl HardwareServices {
    pub const NONE: Self = Self(0);
    /// Append the FCS to every transmitted MPDU and check it on reception.
    pub const FCS: Self = Self(1 << 0);
    /// Send the ACK a received individually addressed frame solicits.
    pub const IMMEDIATE_ACK: Self = Self(1 << 1);
    /// Count down a chosen CSMA/CA backoff and arbitrate the medium.
    pub const BACKOFF_COUNTDOWN: Self = Self(1 << 2);
    /// Encrypt and authenticate protected frames with an installed key.
    pub const CIPHER_TRANSFORM: Self = Self(1 << 3);
    /// Match received A-MPDUs against receive Block Ack agreements and
    /// answer them with a BlockAck.
    pub const RX_BLOCK_ACK_MATCHING: Self = Self(1 << 4);
    /// Capture the BlockAck answering a transmitted A-MPDU.
    pub const TX_BLOCK_ACK_CAPTURE: Self = Self(1 << 5);
    /// Draw the CSMA/CA backoff.
    pub const BACKOFF_DRAW: Self = Self(1 << 6);
    /// Retry unacknowledged attempts and select retry rates.
    pub const RETRY_POLICY: Self = Self(1 << 7);
    /// Assign sequence numbers.
    pub const SEQUENCE_NUMBERS: Self = Self(1 << 8);
    /// Select the key of an outgoing frame.
    pub const KEY_SELECTION: Self = Self(1 << 9);
    /// Allocate packet numbers.
    pub const PACKET_NUMBERS: Self = Self(1 << 10);
    /// Reorder received MPDUs of a Block Ack agreement.
    pub const RX_REORDER: Self = Self(1 << 11);
    /// Select the MPDUs a following A-MPDU attempt retransmits.
    pub const AMPDU_RETRY_SELECTION: Self = Self(1 << 12);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// What a backend supports; it does not change while the port exists.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LowerMacCapabilities {
    pub bands: BandSet,
    pub widths: WidthSet,
    pub rates: RateSupport,
    pub services: HardwareServices,
    /// Virtual interfaces the backend configures at once.
    pub vifs: u8,
    /// EDCA transmit queues, one per access category when four.
    pub tx_queues: u8,
    /// Subframes of one A-MPDU attempt; zero without A-MPDU transmission.
    pub max_ampdu_subframes: u16,
    /// Installed keys at once.
    pub key_slots: u8,
    /// Receive Block Ack agreements at once.
    pub rx_block_ack_agreements: u8,
    /// Highest TID of a receive Block Ack agreement.
    pub rx_block_ack_max_tid: u8,
    /// Largest receive Block Ack window.
    pub rx_block_ack_max_window: u16,
}

impl LowerMacCapabilities {
    /// Whether the backend tunes to `channel`.
    pub const fn supports_channel(self, channel: Channel) -> bool {
        self.bands.contains(channel.band()) && self.widths.contains(channel.width())
    }

    /// Whether the backend sends `rate` on `channel`.
    pub const fn supports_rate(self, rate: PhyRate, channel: Channel) -> bool {
        let dsss_in_5ghz = matches!(channel.band(), Band::Ghz5)
            && matches!(rate, PhyRate::Legacy(legacy) if !legacy.is_ofdm());
        self.rates.supports(rate)
            && !dsss_in_5ghz
            && rate.bandwidth().mhz() <= channel.bandwidth_mhz()
    }
}
